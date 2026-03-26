//! Project-level manifest, lockfile, and materialization helpers.

use std::path::{Path, PathBuf};

use camino::Utf8PathBuf;
use chrono::Utc;
use cpm_types::{
    AssetKind, AssetOwnership, AssetSource, EnvSpec, EnvValue, GitMetadata, GitSourceKind,
    GlobalClaim, GlobalLockfile, LockedFile, Lockfile, Manifest, ManifestGroup, McpProtocol,
    McpRunnerKind, McpTransport, PackageMetadata, PartialSettings, PluginMeta, ResolvedAsset,
    Scope, SourceRule, SubAsset, SubAssetOwnership, WorkflowEngine,
};
use futures::{stream, StreamExt, TryStreamExt};
use indexmap::IndexMap;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info;

use crate::{
    config::{rewrite_source_url, EffectiveSettings},
    fetcher::{atomic_write, fetch_bytes, make_executable, sha256_hex, DownloadProgress},
    installer::{
        copilot_server_entry, install_asset, install_dir, remove_legacy_mcp_file,
        write_mcp_server_entry,
    },
    license::{detect_license, enforce_license_policy},
    resolver::detect_conflicts,
    source::{
        docker_image_pin, parse_github_source, resolve_package_transport_version,
        resolve_pinned_rev, GitHubSource, GitHubSourceMode,
    },
    CpmError,
};

const GITHUB_TREE_FETCH_CONCURRENCY: usize = 8;

/// Options controlling how a manifest application run behaves.
#[derive(Clone, Copy)]
pub struct ApplyOptions<'a> {
    /// Repository root used for relative local paths and local installs.
    pub repo_root: &'a Path,
    /// Whether to install the resolved assets on disk.
    pub install: bool,
    /// Optional extra named group to install in addition to `default`.
    pub install_group: Option<&'a str>,
    /// Optional scope filter applied during installation.
    pub install_scope: Option<Scope>,
    /// Effective runtime settings for policy enforcement.
    pub settings: &'a EffectiveSettings,
    /// User-configured source rewrite rules.
    pub source_rules: &'a IndexMap<String, SourceRule>,
    /// Existing lockfile consulted to carry forward per-asset ownership.
    ///
    /// When supplied, any asset that was previously locked with
    /// [`AssetOwnership::User`] retains that ownership in the new lockfile.
    /// Pass `None` when there is no prior state (e.g. first `cpm lock` run).
    pub existing_lock: Option<&'a Lockfile>,
    /// Optional progress sink used to surface streamed download progress.
    pub download_progress: Option<&'a dyn DownloadProgress>,
}

#[derive(Debug, Clone)]
struct PreparedAsset {
    lock: ResolvedAsset,
    files: Vec<PreparedFile>,
    /// Pre-computed Copilot server entry for MCP assets.
    ///
    /// Carries the entry built from the runtime-rewritten transport (after
    /// source-rule URL substitutions) so that `install_prepared_asset` can
    /// write the correct URL without re-applying rewrites.  Always `None` for
    /// non-MCP assets and for MCP assets taken from the reuse shortcut.
    mcp_entry: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct PreparedFile {
    relative_path: Utf8PathBuf,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct SourceFile {
    relative_path: Utf8PathBuf,
    bytes: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct GitHubContentEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockfileDocument {
    version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generated: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, rename = "plugin", skip_serializing_if = "Vec::is_empty")]
    plugins: Vec<LockfileRecord>,
    #[serde(default, rename = "skill", skip_serializing_if = "Vec::is_empty")]
    skills: Vec<LockfileRecord>,
    #[serde(default, rename = "agent", skip_serializing_if = "Vec::is_empty")]
    agents: Vec<LockfileRecord>,
    #[serde(default, rename = "mcp", skip_serializing_if = "Vec::is_empty")]
    mcps: Vec<LockfileRecord>,
    #[serde(default, rename = "hook", skip_serializing_if = "Vec::is_empty")]
    hooks: Vec<LockfileRecord>,
    #[serde(default, rename = "workflow", skip_serializing_if = "Vec::is_empty")]
    workflows: Vec<LockfileRecord>,
    #[serde(default, rename = "instruction", skip_serializing_if = "Vec::is_empty")]
    instructions: Vec<LockfileRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GlobalLockfileDocument {
    version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generated: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, rename = "claim", skip_serializing_if = "Vec::is_empty")]
    claims: Vec<GlobalClaimRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GlobalClaimRecord {
    claimed_by: Utf8PathBuf,
    kind: AssetKind,
    #[serde(flatten)]
    asset: LockfileRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum LockfileTransport {
    Name(String),
    Legacy(McpTransport),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockfileFileEntry {
    path: Utf8PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum LockfileFileRecord {
    LegacyPath(Utf8PathBuf),
    Entry(LockfileFileEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockfileRecord {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, alias = "date", skip_serializing)]
    legacy_date: Option<chrono::DateTime<chrono::Utc>>,
    hash: String,
    scope: Scope,
    #[serde(default = "default_group")]
    group: String,
    /// Ownership state for this asset entry.  Omitted from the lockfile when
    /// the value is the default (`upstream`) to keep diffs minimal.
    #[serde(default, skip_serializing_if = "asset_ownership_is_upstream")]
    ownership: AssetOwnership,
    /// Unified file list — written via `toml_edit` to produce inline tables.
    /// Deserialized from either bare strings (legacy) or `{ path, sha256 }`.
    #[serde(default, skip_serializing)]
    files: Vec<LockfileFileRecord>,
    #[serde(default, skip_serializing)]
    executable: Vec<Utf8PathBuf>,
    #[serde(default, skip_serializing)]
    file_hashes: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_repo: Option<String>,
    #[serde(default, rename = "git_ref", skip_serializing_if = "Option::is_none")]
    git_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_mode: Option<GitSourceKind>,
    #[serde(default, rename = "sub_asset", skip_serializing_if = "Vec::is_empty")]
    sub_assets: Vec<SubAsset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    license: Option<cpm_types::LicenseInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<Utf8PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<LockfileTransport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entrypoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    env: Vec<cpm_types::EnvSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    engine: Option<WorkflowEngine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bin_path: Option<Utf8PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compiled_path: Option<Utf8PathBuf>,
    // ── Plugin-specific fields (WS-3) ────────────────────────────────────────
    /// Registry that provided this plugin — only present for `[[plugin]]` entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin_registry: Option<String>,
    /// Version string returned by the registry at resolution time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin_version: Option<String>,
    /// Canonical source URL as reported by the registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin_source_url: Option<String>,
    /// `"sha256:<hex>"` hash of the plugin's `plugin.json` manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin_json_hash: Option<String>,
}

fn asset_ownership_is_upstream(ownership: &AssetOwnership) -> bool {
    *ownership == AssetOwnership::Upstream
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

fn strip_sha256_prefix(value: &str) -> String {
    value.strip_prefix("sha256:").unwrap_or(value).to_owned()
}

fn materialize_locked_files(
    files: &[LockfileFileRecord],
    legacy_hashes: &IndexMap<String, String>,
    legacy_executable: &[Utf8PathBuf],
) -> Vec<LockedFile> {
    files
        .iter()
        .map(|file| match file {
            LockfileFileRecord::LegacyPath(path) => LockedFile {
                path: path.clone(),
                sha256: legacy_hashes
                    .get(path.as_str())
                    .map(|hash| strip_sha256_prefix(hash)),
                executable: legacy_executable.contains(path),
            },
            LockfileFileRecord::Entry(entry) => LockedFile {
                path: entry.path.clone(),
                sha256: entry.sha256.clone(),
                executable: entry.executable,
            },
        })
        .collect()
}

fn default_group() -> String {
    "default".to_owned()
}

fn parse_table<'a>(value: &'a toml::Value, context: &str) -> Result<&'a toml::Table, CpmError> {
    value.as_table().ok_or_else(|| CpmError::Parse {
        file: "cpm.toml".to_owned(),
        msg: format!("{context} must be a TOML table"),
    })
}

fn parse_string(value: &toml::Value, context: &str) -> Result<String, CpmError> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| CpmError::Parse {
            file: "cpm.toml".to_owned(),
            msg: format!("{context} must be a string"),
        })
}

fn parse_string_array(value: &toml::Value, context: &str) -> Result<Vec<String>, CpmError> {
    let Some(items) = value.as_array() else {
        return Err(CpmError::Parse {
            file: "cpm.toml".to_owned(),
            msg: format!("{context} must be an array of strings"),
        });
    };

    items
        .iter()
        .map(|item| parse_string(item, context))
        .collect()
}

fn parse_env_value(value: &toml::Value, context: &str) -> Result<EnvValue, CpmError> {
    if let Some(raw) = value.as_str() {
        return Ok(EnvSpec::from_raw(context, raw).value);
    }

    let table = parse_table(value, context)?;
    if let Some(raw) = table.get("literal") {
        return Ok(EnvValue::Literal(parse_string(raw, context)?));
    }
    if let Some(raw) = table.get("from_env") {
        return Ok(EnvValue::FromEnv(parse_string(raw, context)?));
    }

    Err(CpmError::Parse {
        file: "cpm.toml".to_owned(),
        msg: format!("{context} must be a string or env value table"),
    })
}

fn parse_env_specs(value: &toml::Value, context: &str) -> Result<Vec<EnvSpec>, CpmError> {
    if let Some(table) = value.as_table() {
        return table
            .iter()
            .map(|(key, raw)| {
                Ok(EnvSpec {
                    key: key.clone(),
                    value: parse_env_value(raw, &format!("{context}.{key}"))?,
                })
            })
            .collect();
    }

    let Some(items) = value.as_array() else {
        return Err(CpmError::Parse {
            file: "cpm.toml".to_owned(),
            msg: format!("{context} must be a table or array"),
        });
    };

    let mut env = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let item_table = parse_table(item, &format!("{context}[{index}]"))?;
        let key = item_table
            .get("key")
            .ok_or_else(|| CpmError::Parse {
                file: "cpm.toml".to_owned(),
                msg: format!("{context}[{index}] is missing `key`"),
            })
            .and_then(|value| parse_string(value, &format!("{context}[{index}].key")))?;
        let raw_value = item_table.get("value").ok_or_else(|| CpmError::Parse {
            file: "cpm.toml".to_owned(),
            msg: format!("{context}[{index}] is missing `value`"),
        })?;
        env.push(EnvSpec {
            key,
            value: parse_env_value(raw_value, &format!("{context}[{index}].value"))?,
        });
    }
    Ok(env)
}

fn parse_scope(
    value: Option<&toml::Value>,
    default: Scope,
    context: &str,
) -> Result<Scope, CpmError> {
    let Some(raw) = value else {
        return Ok(default);
    };

    parse_string(raw, context)?
        .parse::<Scope>()
        .map_err(|reason| CpmError::Parse {
            file: "cpm.toml".to_owned(),
            msg: format!("{context}: {reason}"),
        })
}

fn parse_mcp_transport(
    table: &toml::Table,
    context: &str,
) -> Result<Option<McpTransport>, CpmError> {
    let protocol = table
        .get("type")
        .map(|value| parse_string(value, &format!("{context}.type")))
        .transpose()?;
    let runner = table
        .get("runner")
        .map(|value| parse_string(value, &format!("{context}.runner")))
        .transpose()?;

    if protocol.is_some() || runner.is_some() {
        let args = table
            .get("args")
            .map(|value| parse_string_array(value, &format!("{context}.args")))
            .transpose()?
            .unwrap_or_default();

        let infer_runner = || -> Option<String> {
            if table.contains_key("package") {
                let package = table.get("package")?.as_str()?;
                return Some(
                    if package.starts_with('@') || package.contains('/') {
                        "npx"
                    } else {
                        "uvx"
                    }
                    .to_owned(),
                );
            }
            if table.contains_key("image") {
                return Some("docker".to_owned());
            }
            if table.contains_key("bin") {
                return Some("binary".to_owned());
            }
            if table.contains_key("path") {
                return Some("local".to_owned());
            }
            if table.contains_key("command") || table.contains_key("script") {
                return Some("command".to_owned());
            }
            None
        };

        let build_stdio = |runner_name: &str| -> Result<McpTransport, CpmError> {
            match runner_name {
                "npx" => Ok(McpTransport::Npx {
                    package: parse_string(
                        table.get("package").ok_or_else(|| CpmError::Parse {
                            file: "cpm.toml".to_owned(),
                            msg: format!("{context} is missing `package`"),
                        })?,
                        &format!("{context}.package"),
                    )?,
                    entrypoint: table
                        .get("entrypoint")
                        .map(|value| parse_string(value, &format!("{context}.entrypoint")))
                        .transpose()?,
                    args,
                }),
                "uvx" => Ok(McpTransport::Uvx {
                    package: parse_string(
                        table.get("package").ok_or_else(|| CpmError::Parse {
                            file: "cpm.toml".to_owned(),
                            msg: format!("{context} is missing `package`"),
                        })?,
                        &format!("{context}.package"),
                    )?,
                    entrypoint: table
                        .get("entrypoint")
                        .map(|value| parse_string(value, &format!("{context}.entrypoint")))
                        .transpose()?,
                    args,
                }),
                "docker" => Ok(McpTransport::Docker {
                    image: parse_string(
                        table.get("image").ok_or_else(|| CpmError::Parse {
                            file: "cpm.toml".to_owned(),
                            msg: format!("{context} is missing `image`"),
                        })?,
                        &format!("{context}.image"),
                    )?,
                    args,
                }),
                "binary" => Ok(McpTransport::Binary {
                    url: parse_string(
                        table.get("url").ok_or_else(|| CpmError::Parse {
                            file: "cpm.toml".to_owned(),
                            msg: format!("{context} is missing `url`"),
                        })?,
                        &format!("{context}.url"),
                    )?,
                    bin: parse_string(
                        table.get("bin").ok_or_else(|| CpmError::Parse {
                            file: "cpm.toml".to_owned(),
                            msg: format!("{context} is missing `bin`"),
                        })?,
                        &format!("{context}.bin"),
                    )?,
                    args,
                }),
                "local" | "path" => Ok(McpTransport::Path {
                    path: parse_string(
                        table.get("path").ok_or_else(|| CpmError::Parse {
                            file: "cpm.toml".to_owned(),
                            msg: format!("{context} is missing `path`"),
                        })?,
                        &format!("{context}.path"),
                    )?
                    .into(),
                    args,
                }),
                "command" | "script" => Ok(McpTransport::Script {
                    command: parse_string(
                        table
                            .get("command")
                            .or_else(|| table.get("script"))
                            .ok_or_else(|| CpmError::Parse {
                                file: "cpm.toml".to_owned(),
                                msg: format!("{context} is missing `command`"),
                            })?,
                        &format!("{context}.command"),
                    )?,
                    args,
                }),
                other => Err(CpmError::Parse {
                    file: "cpm.toml".to_owned(),
                    msg: format!("{context} has unsupported runner `{other}`"),
                }),
            }
        };

        let protocol_name = protocol.as_deref().unwrap_or("stdio");
        return match protocol_name {
            "http" => Ok(Some(McpTransport::Http {
                url: parse_string(
                    table.get("url").ok_or_else(|| CpmError::Parse {
                        file: "cpm.toml".to_owned(),
                        msg: format!("{context} is missing `url`"),
                    })?,
                    &format!("{context}.url"),
                )?,
            })),
            "sse" => Ok(Some(McpTransport::Sse {
                url: parse_string(
                    table.get("url").ok_or_else(|| CpmError::Parse {
                        file: "cpm.toml".to_owned(),
                        msg: format!("{context} is missing `url`"),
                    })?,
                    &format!("{context}.url"),
                )?,
            })),
            "stdio" | "local" => {
                let runner_name =
                    runner
                        .clone()
                        .or_else(infer_runner)
                        .ok_or_else(|| CpmError::Parse {
                            file: "cpm.toml".to_owned(),
                            msg: format!("{context} is missing `runner`"),
                        })?;
                build_stdio(&runner_name).map(Some)
            }
            other => Err(CpmError::Parse {
                file: "cpm.toml".to_owned(),
                msg: format!("{context} has unsupported type `{other}`"),
            }),
        };
    }

    let Some(transport_value) = table.get("transport") else {
        return Ok(None);
    };

    if let Some(name) = transport_value.as_str() {
        let args = table
            .get("args")
            .map(|value| parse_string_array(value, &format!("{context}.args")))
            .transpose()?
            .unwrap_or_default();

        let transport = match name {
            "http" => McpTransport::Http {
                url: parse_string(
                    table.get("url").ok_or_else(|| CpmError::Parse {
                        file: "cpm.toml".to_owned(),
                        msg: format!("{context} is missing `url`"),
                    })?,
                    &format!("{context}.url"),
                )?,
            },
            "sse" => McpTransport::Sse {
                url: parse_string(
                    table.get("url").ok_or_else(|| CpmError::Parse {
                        file: "cpm.toml".to_owned(),
                        msg: format!("{context} is missing `url`"),
                    })?,
                    &format!("{context}.url"),
                )?,
            },
            "npx" => McpTransport::Npx {
                package: parse_string(
                    table.get("package").ok_or_else(|| CpmError::Parse {
                        file: "cpm.toml".to_owned(),
                        msg: format!("{context} is missing `package`"),
                    })?,
                    &format!("{context}.package"),
                )?,
                entrypoint: table
                    .get("entrypoint")
                    .map(|value| parse_string(value, &format!("{context}.entrypoint")))
                    .transpose()?,
                args,
            },
            "uvx" => McpTransport::Uvx {
                package: parse_string(
                    table.get("package").ok_or_else(|| CpmError::Parse {
                        file: "cpm.toml".to_owned(),
                        msg: format!("{context} is missing `package`"),
                    })?,
                    &format!("{context}.package"),
                )?,
                entrypoint: table
                    .get("entrypoint")
                    .map(|value| parse_string(value, &format!("{context}.entrypoint")))
                    .transpose()?,
                args,
            },
            "docker" => McpTransport::Docker {
                image: parse_string(
                    table.get("image").ok_or_else(|| CpmError::Parse {
                        file: "cpm.toml".to_owned(),
                        msg: format!("{context} is missing `image`"),
                    })?,
                    &format!("{context}.image"),
                )?,
                args,
            },
            "binary" => McpTransport::Binary {
                url: parse_string(
                    table.get("url").ok_or_else(|| CpmError::Parse {
                        file: "cpm.toml".to_owned(),
                        msg: format!("{context} is missing `url`"),
                    })?,
                    &format!("{context}.url"),
                )?,
                bin: parse_string(
                    table.get("bin").ok_or_else(|| CpmError::Parse {
                        file: "cpm.toml".to_owned(),
                        msg: format!("{context} is missing `bin`"),
                    })?,
                    &format!("{context}.bin"),
                )?,
                args,
            },
            "path" => McpTransport::Path {
                path: parse_string(
                    table.get("path").ok_or_else(|| CpmError::Parse {
                        file: "cpm.toml".to_owned(),
                        msg: format!("{context} is missing `path`"),
                    })?,
                    &format!("{context}.path"),
                )?
                .into(),
                args,
            },
            "script" => McpTransport::Script {
                command: parse_string(
                    table
                        .get("command")
                        .or_else(|| table.get("script"))
                        .ok_or_else(|| CpmError::Parse {
                            file: "cpm.toml".to_owned(),
                            msg: format!("{context} is missing `command`"),
                        })?,
                    &format!("{context}.command"),
                )?,
                args,
            },
            other => {
                return Err(CpmError::Parse {
                    file: "cpm.toml".to_owned(),
                    msg: format!("{context} has unsupported transport `{other}`"),
                });
            }
        };
        return Ok(Some(transport));
    }

    let legacy = parse_table(transport_value, &format!("{context}.transport"))?;
    if let Some(value) = legacy.get("http") {
        let inner = parse_table(value, &format!("{context}.transport.http"))?;
        return Ok(Some(McpTransport::Http {
            url: parse_string(
                inner.get("url").ok_or_else(|| CpmError::Parse {
                    file: "cpm.toml".to_owned(),
                    msg: format!("{context}.transport.http is missing `url`"),
                })?,
                &format!("{context}.transport.http.url"),
            )?,
        }));
    }
    if let Some(value) = legacy.get("sse") {
        let inner = parse_table(value, &format!("{context}.transport.sse"))?;
        return Ok(Some(McpTransport::Sse {
            url: parse_string(
                inner.get("url").ok_or_else(|| CpmError::Parse {
                    file: "cpm.toml".to_owned(),
                    msg: format!("{context}.transport.sse is missing `url`"),
                })?,
                &format!("{context}.transport.sse.url"),
            )?,
        }));
    }
    if let Some(value) = legacy.get("npx") {
        let inner = parse_table(value, &format!("{context}.transport.npx"))?;
        return Ok(Some(McpTransport::Npx {
            package: parse_string(
                inner.get("package").ok_or_else(|| CpmError::Parse {
                    file: "cpm.toml".to_owned(),
                    msg: format!("{context}.transport.npx is missing `package`"),
                })?,
                &format!("{context}.transport.npx.package"),
            )?,
            entrypoint: inner
                .get("entrypoint")
                .map(|value| parse_string(value, &format!("{context}.transport.npx.entrypoint")))
                .transpose()?,
            args: inner
                .get("args")
                .map(|value| parse_string_array(value, &format!("{context}.transport.npx.args")))
                .transpose()?
                .unwrap_or_default(),
        }));
    }
    if let Some(value) = legacy.get("uvx") {
        let inner = parse_table(value, &format!("{context}.transport.uvx"))?;
        return Ok(Some(McpTransport::Uvx {
            package: parse_string(
                inner.get("package").ok_or_else(|| CpmError::Parse {
                    file: "cpm.toml".to_owned(),
                    msg: format!("{context}.transport.uvx is missing `package`"),
                })?,
                &format!("{context}.transport.uvx.package"),
            )?,
            entrypoint: inner
                .get("entrypoint")
                .map(|value| parse_string(value, &format!("{context}.transport.uvx.entrypoint")))
                .transpose()?,
            args: inner
                .get("args")
                .map(|value| parse_string_array(value, &format!("{context}.transport.uvx.args")))
                .transpose()?
                .unwrap_or_default(),
        }));
    }
    if let Some(value) = legacy.get("docker") {
        let inner = parse_table(value, &format!("{context}.transport.docker"))?;
        return Ok(Some(McpTransport::Docker {
            image: parse_string(
                inner.get("image").ok_or_else(|| CpmError::Parse {
                    file: "cpm.toml".to_owned(),
                    msg: format!("{context}.transport.docker is missing `image`"),
                })?,
                &format!("{context}.transport.docker.image"),
            )?,
            args: inner
                .get("args")
                .map(|value| parse_string_array(value, &format!("{context}.transport.docker.args")))
                .transpose()?
                .unwrap_or_default(),
        }));
    }
    if let Some(value) = legacy.get("binary") {
        let inner = parse_table(value, &format!("{context}.transport.binary"))?;
        return Ok(Some(McpTransport::Binary {
            url: parse_string(
                inner.get("url").ok_or_else(|| CpmError::Parse {
                    file: "cpm.toml".to_owned(),
                    msg: format!("{context}.transport.binary is missing `url`"),
                })?,
                &format!("{context}.transport.binary.url"),
            )?,
            bin: parse_string(
                inner.get("bin").ok_or_else(|| CpmError::Parse {
                    file: "cpm.toml".to_owned(),
                    msg: format!("{context}.transport.binary is missing `bin`"),
                })?,
                &format!("{context}.transport.binary.bin"),
            )?,
            args: inner
                .get("args")
                .map(|value| parse_string_array(value, &format!("{context}.transport.binary.args")))
                .transpose()?
                .unwrap_or_default(),
        }));
    }
    if let Some(value) = legacy.get("path") {
        let inner = parse_table(value, &format!("{context}.transport.path"))?;
        return Ok(Some(McpTransport::Path {
            path: parse_string(
                inner.get("path").ok_or_else(|| CpmError::Parse {
                    file: "cpm.toml".to_owned(),
                    msg: format!("{context}.transport.path is missing `path`"),
                })?,
                &format!("{context}.transport.path.path"),
            )?
            .into(),
            args: inner
                .get("args")
                .map(|value| parse_string_array(value, &format!("{context}.transport.path.args")))
                .transpose()?
                .unwrap_or_default(),
        }));
    }
    if let Some(value) = legacy.get("script") {
        let inner = parse_table(value, &format!("{context}.transport.script"))?;
        return Ok(Some(McpTransport::Script {
            command: parse_string(
                inner
                    .get("command")
                    .or_else(|| inner.get("script"))
                    .ok_or_else(|| CpmError::Parse {
                        file: "cpm.toml".to_owned(),
                        msg: format!("{context}.transport.script is missing `command`"),
                    })?,
                &format!("{context}.transport.script.command"),
            )?,
            args: inner
                .get("args")
                .map(|value| parse_string_array(value, &format!("{context}.transport.script.args")))
                .transpose()?
                .unwrap_or_default(),
        }));
    }

    Err(CpmError::Parse {
        file: "cpm.toml".to_owned(),
        msg: format!("{context}.transport has no supported variant"),
    })
}

fn parse_asset_source(
    value: &toml::Value,
    kind: AssetKind,
    default_group_name: &str,
    context: &str,
) -> Result<AssetSource, CpmError> {
    if let Some(raw) = value.as_str() {
        return Ok(AssetSource {
            url: Some(raw.to_owned()),
            rev: None,
            path: None,
            group: default_group_name.to_owned(),
            scope: Scope::Local,
            transport: None,
            env: Vec::new(),
            args: Vec::new(),
            engine: None,
        });
    }

    let table = parse_table(value, context)?;
    let rev = table
        .get("rev")
        .map(|value| parse_string(value, &format!("{context}.rev")))
        .transpose()?;
    let path = table
        .get("path")
        .map(|value| parse_string(value, &format!("{context}.path")).map(Into::into))
        .transpose()?;
    let group = table
        .get("group")
        .map(|value| parse_string(value, &format!("{context}.group")))
        .transpose()?
        .unwrap_or_else(|| default_group_name.to_owned());
    let scope = parse_scope(
        table.get("scope"),
        Scope::Local,
        &format!("{context}.scope"),
    )?;
    let args = table
        .get("args")
        .map(|value| parse_string_array(value, &format!("{context}.args")))
        .transpose()?
        .unwrap_or_default();
    let env = table
        .get("env")
        .map(|value| parse_env_specs(value, &format!("{context}.env")))
        .transpose()?
        .unwrap_or_default();
    let engine = if kind == AssetKind::Workflow {
        table
            .get("engine")
            .map(|value| parse_string(value, &format!("{context}.engine")))
            .transpose()?
            .map(|value| match value.to_ascii_lowercase().as_str() {
                "copilot" => Ok(WorkflowEngine::Copilot),
                "claude" => Ok(WorkflowEngine::Claude),
                "codex" => Ok(WorkflowEngine::Codex),
                other => Err(CpmError::Parse {
                    file: "cpm.toml".to_owned(),
                    msg: format!("{context}.engine has unsupported value `{other}`"),
                }),
            })
            .transpose()?
    } else {
        None
    };

    let transport = if kind == AssetKind::Mcp {
        parse_mcp_transport(table, context)?
    } else {
        None
    };

    // When args are stored inside the transport (legacy nested form) but not
    // at the top level, mirror them into source.args so the manifest writer
    // can emit them correctly on the next write and preserve the roundtrip.
    let args = if args.is_empty() {
        match &transport {
            Some(
                McpTransport::Script { args: t_args, .. }
                | McpTransport::Npx { args: t_args, .. }
                | McpTransport::Uvx { args: t_args, .. }
                | McpTransport::Docker { args: t_args, .. }
                | McpTransport::Binary { args: t_args, .. }
                | McpTransport::Path { args: t_args, .. },
            ) if !t_args.is_empty() => t_args.clone(),
            _ => args,
        }
    } else {
        args
    };

    let url = table
        .get("url")
        .map(|value| parse_string(value, &format!("{context}.url")))
        .transpose()?;

    Ok(AssetSource {
        url,
        rev,
        path,
        group,
        scope,
        transport,
        env,
        args,
        engine,
    })
}

fn parse_section(
    table: Option<&toml::Value>,
    kind: AssetKind,
    default_group_name: &str,
    context: &str,
) -> Result<IndexMap<String, AssetSource>, CpmError> {
    let Some(value) = table else {
        return Ok(IndexMap::new());
    };
    let entries = parse_table(value, context)?;
    let mut section = IndexMap::new();
    for (name, entry) in entries {
        section.insert(
            name.clone(),
            parse_asset_source(
                entry,
                kind,
                default_group_name,
                &format!("{context}.{name}"),
            )?,
        );
    }
    Ok(section)
}

fn parse_groups(value: Option<&toml::Value>) -> Result<IndexMap<String, ManifestGroup>, CpmError> {
    let Some(value) = value else {
        return Ok(IndexMap::new());
    };

    let groups_table = parse_table(value, "groups")?;
    let mut groups = IndexMap::new();
    for (group_name, group_value) in groups_table {
        let group_table = parse_table(group_value, &format!("groups.{group_name}"))?;
        let description = group_table
            .get("description")
            .map(|value| parse_string(value, &format!("groups.{group_name}.description")))
            .transpose()?;
        let plugins = parse_section(
            group_table.get("plugins"),
            AssetKind::Plugin,
            group_name,
            &format!("groups.{group_name}.plugins"),
        )?;
        let skills = parse_section(
            group_table.get("skills"),
            AssetKind::Skill,
            group_name,
            &format!("groups.{group_name}.skills"),
        )?;
        let agents = parse_section(
            group_table.get("agents"),
            AssetKind::Agent,
            group_name,
            &format!("groups.{group_name}.agents"),
        )?;
        let mcps = parse_section(
            group_table.get("mcps"),
            AssetKind::Mcp,
            group_name,
            &format!("groups.{group_name}.mcps"),
        )?;
        let hooks = parse_section(
            group_table.get("hooks"),
            AssetKind::Hook,
            group_name,
            &format!("groups.{group_name}.hooks"),
        )?;
        let workflows = parse_section(
            group_table.get("workflows"),
            AssetKind::Workflow,
            group_name,
            &format!("groups.{group_name}.workflows"),
        )?;
        let instructions = parse_section(
            group_table.get("instructions"),
            AssetKind::Instruction,
            group_name,
            &format!("groups.{group_name}.instructions"),
        )?;

        groups.insert(
            group_name.clone(),
            ManifestGroup {
                description,
                plugins,
                skills,
                agents,
                mcps,
                hooks,
                workflows,
                instructions,
            },
        );
    }
    Ok(groups)
}

fn is_short_form_asset(source: &AssetSource, default_group_name: &str) -> bool {
    source.url.is_some()
        && source.rev.is_none()
        && source.path.is_none()
        && source.group == default_group_name
        && source.scope == Scope::Local
        && source.transport.is_none()
        && source.env.is_empty()
        && source.args.is_empty()
        && source.engine.is_none()
}

fn render_toml_value<T: Serialize>(value: T) -> Result<String, CpmError> {
    Ok(toml::Value::try_from(value)?.to_string())
}

fn render_env_inline(env: &[EnvSpec]) -> Result<String, CpmError> {
    let mut parts = Vec::new();
    for spec in env {
        let value = match &spec.value {
            EnvValue::Literal(raw) => render_toml_value(raw.clone())?,
            EnvValue::FromEnv(name) => render_toml_value(format!("${name}"))?,
        };
        parts.push(format!("{} = {}", spec.key, value));
    }
    Ok(format!("{{ {} }}", parts.join(", ")))
}

fn render_asset_source_inline(
    source: &AssetSource,
    kind: AssetKind,
    default_group_name: &str,
) -> Result<String, CpmError> {
    if kind != AssetKind::Mcp && is_short_form_asset(source, default_group_name) {
        if let Some(url) = &source.url {
            return render_toml_value(url.clone());
        }
    }

    let mut parts = Vec::new();
    if let Some(url) = &source.url {
        parts.push(format!("url = {}", render_toml_value(url.clone())?));
    }
    if let Some(rev) = &source.rev {
        parts.push(format!("rev = {}", render_toml_value(rev.clone())?));
    }
    if let Some(path) = &source.path {
        parts.push(format!("path = {}", render_toml_value(path.to_string())?));
    }
    if source.group != default_group_name {
        parts.push(format!(
            "group = {}",
            render_toml_value(source.group.clone())?
        ));
    }
    if source.scope != Scope::Local {
        parts.push(format!("scope = {}", render_toml_value(source.scope)?));
    }
    if !source.args.is_empty() {
        parts.push(format!(
            "args = {}",
            render_toml_value(source.args.clone())?
        ));
    }
    if !source.env.is_empty() {
        parts.push(format!("env = {}", render_env_inline(&source.env)?));
    }

    if let Some(transport) = &source.transport {
        parts.push(format!(
            "transport = {}",
            render_toml_value(transport.name().to_owned())?
        ));
        match transport {
            McpTransport::Http { url } | McpTransport::Sse { url } => {
                if source.url.as_deref() != Some(url.as_str()) {
                    parts.push(format!("url = {}", render_toml_value(url.clone())?));
                }
            }
            McpTransport::Npx { package, .. } | McpTransport::Uvx { package, .. } => {
                parts.push(format!("package = {}", render_toml_value(package.clone())?));
            }
            McpTransport::Docker { image, .. } => {
                parts.push(format!("image = {}", render_toml_value(image.clone())?));
            }
            McpTransport::Binary { url, bin, .. } => {
                if source.url.as_deref() != Some(url.as_str()) {
                    parts.push(format!("url = {}", render_toml_value(url.clone())?));
                }
                parts.push(format!("bin = {}", render_toml_value(bin.clone())?));
            }
            McpTransport::Path { path, .. } => {
                parts.push(format!(
                    "path = {}",
                    render_toml_value(path.to_string_lossy().into_owned())?
                ));
            }
            McpTransport::Script { command, .. } => {
                parts.push(format!("command = {}", render_toml_value(command.clone())?));
            }
        }
    }

    Ok(format!("{{ {} }}", parts.join(", ")))
}

fn canonical_mcp_type(transport: &McpTransport) -> &'static str {
    match transport.protocol() {
        McpProtocol::Stdio => "stdio",
        McpProtocol::Http => "http",
        McpProtocol::Sse => "sse",
    }
}

fn canonical_mcp_runner(transport: &McpTransport) -> Option<&'static str> {
    match transport.runner_kind() {
        Some(McpRunnerKind::Npx) => Some("npx"),
        Some(McpRunnerKind::Uvx) => Some("uvx"),
        Some(McpRunnerKind::Docker) => Some("docker"),
        Some(McpRunnerKind::Binary) => Some("binary"),
        Some(McpRunnerKind::Local) => Some("local"),
        Some(McpRunnerKind::Command) => Some("command"),
        None => None,
    }
}

fn render_mcp_source_parts(
    source: &AssetSource,
    default_group_name: &str,
) -> Result<Vec<String>, CpmError> {
    let mut parts = Vec::new();
    if let Some(rev) = &source.rev {
        parts.push(format!("rev = {}", render_toml_value(rev.clone())?));
    }
    if source.group != default_group_name {
        parts.push(format!(
            "group = {}",
            render_toml_value(source.group.clone())?
        ));
    }
    if source.scope != Scope::Local {
        parts.push(format!("scope = {}", render_toml_value(source.scope)?));
    }
    if let Some(transport) = &source.transport {
        parts.push(format!(
            "type = {}",
            render_toml_value(canonical_mcp_type(transport).to_owned())?
        ));
        if let Some(runner) = canonical_mcp_runner(transport) {
            parts.push(format!(
                "runner = {}",
                render_toml_value(runner.to_owned())?
            ));
        }
        match transport {
            McpTransport::Http { url } | McpTransport::Sse { url } => {
                if source.url.as_deref() != Some(url.as_str()) {
                    parts.push(format!("url = {}", render_toml_value(url.clone())?));
                }
            }
            McpTransport::Npx {
                package,
                entrypoint,
                ..
            }
            | McpTransport::Uvx {
                package,
                entrypoint,
                ..
            } => {
                parts.push(format!("package = {}", render_toml_value(package.clone())?));
                if let Some(entrypoint) = entrypoint {
                    parts.push(format!(
                        "entrypoint = {}",
                        render_toml_value(entrypoint.clone())?
                    ));
                }
            }
            McpTransport::Docker { image, .. } => {
                parts.push(format!("image = {}", render_toml_value(image.clone())?));
            }
            McpTransport::Binary { url, bin, .. } => {
                if source.url.as_deref() != Some(url.as_str()) {
                    parts.push(format!("url = {}", render_toml_value(url.clone())?));
                }
                parts.push(format!("bin = {}", render_toml_value(bin.clone())?));
            }
            McpTransport::Path { path, .. } => {
                parts.push(format!(
                    "path = {}",
                    render_toml_value(path.to_string_lossy().into_owned())?
                ));
            }
            McpTransport::Script { command, .. } => {
                parts.push(format!("command = {}", render_toml_value(command.clone())?));
            }
        }
    } else {
        if let Some(url) = &source.url {
            parts.push(format!("url = {}", render_toml_value(url.clone())?));
        }
        if let Some(path) = &source.path {
            parts.push(format!("path = {}", render_toml_value(path.to_string())?));
        }
    }
    if !source.args.is_empty() {
        parts.push(format!(
            "args = {}",
            render_toml_value(source.args.clone())?
        ));
    }
    Ok(parts)
}

fn render_mcp_source_inline(
    source: &AssetSource,
    default_group_name: &str,
) -> Result<String, CpmError> {
    Ok(format!(
        "{{ {} }}",
        render_mcp_source_parts(source, default_group_name)?.join(", ")
    ))
}

fn write_asset_section(
    output: &mut String,
    header: &str,
    section: &IndexMap<String, AssetSource>,
    kind: AssetKind,
    default_group_name: &str,
    always_emit_header: bool,
) -> Result<(), CpmError> {
    if !always_emit_header && section.is_empty() {
        return Ok(());
    }
    output.push_str(&format!("[{header}]\n"));
    for (name, source) in section {
        output.push_str(&format!(
            "{name} = {}\n",
            render_asset_source_inline(source, kind, default_group_name)?
        ));
    }
    output.push('\n');
    Ok(())
}

fn write_settings(output: &mut String, settings: &PartialSettings) -> Result<(), CpmError> {
    if settings.is_empty() {
        return Ok(());
    }
    output.push_str("[settings]\n");
    if let Some(default_scope) = settings.default_scope {
        output.push_str(&format!(
            "default_scope = {}\n",
            render_toml_value(default_scope)?
        ));
    }
    if let Some(update_policy) = settings.update_policy {
        output.push_str(&format!(
            "update_policy = {}\n",
            render_toml_value(update_policy)?
        ));
    }
    if let Some(license_policy) = settings.license_policy {
        output.push_str(&format!(
            "license_policy = {}\n",
            render_toml_value(license_policy)?
        ));
    }
    if let Some(allowed_licenses) = &settings.allowed_licenses {
        output.push_str(&format!(
            "allowed_licenses = {}\n",
            render_toml_value(allowed_licenses.clone())?
        ));
    }
    if let Some(cache_dir) = &settings.cache_dir {
        output.push_str(&format!(
            "cache_dir = {}\n",
            render_toml_value(cache_dir.clone())?
        ));
    }
    if let Some(network_timeout) = settings.network_timeout {
        output.push_str(&format!(
            "network_timeout = {}\n",
            render_toml_value(network_timeout)?
        ));
    }
    if let Some(auto_groups) = &settings.auto_groups {
        output.push_str(&format!(
            "auto_groups = {}\n",
            render_toml_value(auto_groups.clone())?
        ));
    }
    if let Some(verify_on_sync) = settings.verify_on_sync {
        output.push_str(&format!(
            "verify_on_sync = {}\n",
            render_toml_value(verify_on_sync)?
        ));
    }
    output.push('\n');
    Ok(())
}

fn write_package(output: &mut String, package: &PackageMetadata) -> Result<(), CpmError> {
    output.push_str("[package]\n");
    output.push_str(&format!(
        "name = {}\n",
        render_toml_value(package.name.clone())?
    ));
    if let Some(description) = &package.description {
        output.push_str(&format!(
            "description = {}\n",
            render_toml_value(description.clone())?
        ));
    }
    output.push_str(&format!(
        "version = {}\n",
        render_toml_value(package.version.clone())?
    ));
    if let Some(license) = &package.license {
        output.push_str(&format!(
            "license = {}\n",
            render_toml_value(license.clone())?
        ));
    }
    if let Some(authors) = &package.authors {
        output.push_str(&format!(
            "authors = {}\n",
            render_toml_value(authors.clone())?
        ));
    }
    if let Some(repository) = &package.repository {
        output.push_str(&format!(
            "repository = {}\n",
            render_toml_value(repository.clone())?
        ));
    }
    if let Some(created) = &package.created {
        output.push_str(&format!(
            "created = {}\n",
            render_toml_value(created.clone())?
        ));
    }
    output.push('\n');
    Ok(())
}

fn write_sources(
    output: &mut String,
    sources: &IndexMap<String, SourceRule>,
) -> Result<(), CpmError> {
    for (name, rule) in sources {
        output.push_str(&format!("[sources.{name}]\n"));
        output.push_str(&format!("url = {}\n", render_toml_value(rule.url.clone())?));
        if let Some(token_env) = &rule.token_env {
            output.push_str(&format!(
                "token_env = {}\n",
                render_toml_value(token_env.clone())?
            ));
        }
        if let Some(replace) = &rule.replace {
            output.push_str(&format!(
                "replace = {}\n",
                render_toml_value(replace.clone())?
            ));
        }
        output.push('\n');
    }
    Ok(())
}

fn write_mcp_sections(
    output: &mut String,
    header_prefix: &str,
    section: &IndexMap<String, AssetSource>,
    default_group_name: &str,
) -> Result<(), CpmError> {
    if section.is_empty() {
        return Ok(());
    }

    let inline_entries: Vec<_> = section
        .iter()
        .filter(|(_, source)| source.env.is_empty())
        .collect();
    let nested_entries: Vec<_> = section
        .iter()
        .filter(|(_, source)| !source.env.is_empty())
        .collect();

    if !inline_entries.is_empty() {
        output.push_str(&format!("[{header_prefix}]\n"));
        for (name, source) in inline_entries {
            output.push_str(&format!(
                "{name} = {}\n",
                render_mcp_source_inline(source, default_group_name)?
            ));
        }
        output.push('\n');
    }

    if nested_entries.is_empty() {
        return Ok(());
    }

    for (name, source) in nested_entries {
        output.push_str(&format!("[{header_prefix}.{name}]\n"));
        for part in render_mcp_source_parts(source, default_group_name)? {
            output.push_str(&part);
            output.push('\n');
        }
        output.push('\n');
        if !source.env.is_empty() {
            output.push_str(&format!("[{header_prefix}.{name}.env]\n"));
            for spec in &source.env {
                let value = match &spec.value {
                    EnvValue::Literal(raw) => render_toml_value(raw.clone())?,
                    EnvValue::FromEnv(var) => render_toml_value(format!("${var}"))?,
                };
                output.push_str(&format!("{} = {}\n", spec.key, value));
            }
            output.push('\n');
        }
    }
    Ok(())
}

/// Load `cpm.toml` from disk.
pub fn load_manifest(path: &Path) -> Result<Manifest, CpmError> {
    if !path.exists() {
        return Ok(Manifest::default());
    }

    let contents = std::fs::read_to_string(path)?;
    let document: toml::Value = toml::from_str(&contents).map_err(|err| CpmError::Parse {
        file: path.display().to_string(),
        msg: err.to_string(),
    })?;
    let root = parse_table(&document, "root")?;

    let package = root
        .get("package")
        .map(|value| value.clone().try_into::<PackageMetadata>())
        .transpose()
        .map_err(|err| CpmError::Parse {
            file: path.display().to_string(),
            msg: err.to_string(),
        })?;
    let settings = root
        .get("settings")
        .map(|value| value.clone().try_into::<PartialSettings>())
        .transpose()
        .map_err(|err| CpmError::Parse {
            file: path.display().to_string(),
            msg: err.to_string(),
        })?
        .unwrap_or_default();
    let sources = root
        .get("sources")
        .map(|value| value.clone().try_into::<IndexMap<String, SourceRule>>())
        .transpose()
        .map_err(|err| CpmError::Parse {
            file: path.display().to_string(),
            msg: err.to_string(),
        })?
        .unwrap_or_default();

    Ok(Manifest {
        package,
        settings,
        sources,
        plugins: parse_section(root.get("plugins"), AssetKind::Plugin, "default", "plugins")?,
        skills: parse_section(root.get("skills"), AssetKind::Skill, "default", "skills")?,
        agents: parse_section(root.get("agents"), AssetKind::Agent, "default", "agents")?,
        mcps: parse_section(root.get("mcps"), AssetKind::Mcp, "default", "mcps")?,
        hooks: parse_section(root.get("hooks"), AssetKind::Hook, "default", "hooks")?,
        workflows: parse_section(
            root.get("workflows"),
            AssetKind::Workflow,
            "default",
            "workflows",
        )?,
        instructions: parse_section(
            root.get("instructions"),
            AssetKind::Instruction,
            "default",
            "instructions",
        )?,
        groups: parse_groups(root.get("groups"))?,
    })
}

/// Persist `cpm.toml`.
pub fn write_manifest(path: &Path, manifest: &Manifest) -> Result<(), CpmError> {
    let mut serialized = String::new();
    if let Some(package) = &manifest.package {
        write_package(&mut serialized, package)?;
    }
    write_settings(&mut serialized, &manifest.settings)?;
    write_sources(&mut serialized, &manifest.sources)?;
    write_asset_section(
        &mut serialized,
        "plugins",
        &manifest.plugins,
        AssetKind::Plugin,
        "default",
        true,
    )?;
    write_asset_section(
        &mut serialized,
        "skills",
        &manifest.skills,
        AssetKind::Skill,
        "default",
        true,
    )?;
    write_asset_section(
        &mut serialized,
        "agents",
        &manifest.agents,
        AssetKind::Agent,
        "default",
        true,
    )?;
    if manifest.mcps.is_empty() {
        serialized.push_str("[mcps]\n\n");
    } else {
        write_mcp_sections(&mut serialized, "mcps", &manifest.mcps, "default")?;
    }
    write_asset_section(
        &mut serialized,
        "hooks",
        &manifest.hooks,
        AssetKind::Hook,
        "default",
        true,
    )?;
    write_asset_section(
        &mut serialized,
        "workflows",
        &manifest.workflows,
        AssetKind::Workflow,
        "default",
        true,
    )?;
    write_asset_section(
        &mut serialized,
        "instructions",
        &manifest.instructions,
        AssetKind::Instruction,
        "default",
        true,
    )?;
    for (group_name, group) in &manifest.groups {
        serialized.push_str(&format!("[groups.{group_name}]\n"));
        if let Some(description) = &group.description {
            serialized.push_str(&format!(
                "description = {}\n",
                render_toml_value(description.clone())?
            ));
        }
        serialized.push('\n');
        write_asset_section(
            &mut serialized,
            &format!("groups.{group_name}.plugins"),
            &group.plugins,
            AssetKind::Plugin,
            group_name,
            false,
        )?;
        write_asset_section(
            &mut serialized,
            &format!("groups.{group_name}.skills"),
            &group.skills,
            AssetKind::Skill,
            group_name,
            false,
        )?;
        write_asset_section(
            &mut serialized,
            &format!("groups.{group_name}.agents"),
            &group.agents,
            AssetKind::Agent,
            group_name,
            false,
        )?;
        write_mcp_sections(
            &mut serialized,
            &format!("groups.{group_name}.mcps"),
            &group.mcps,
            group_name,
        )?;
        write_asset_section(
            &mut serialized,
            &format!("groups.{group_name}.hooks"),
            &group.hooks,
            AssetKind::Hook,
            group_name,
            false,
        )?;
        write_asset_section(
            &mut serialized,
            &format!("groups.{group_name}.workflows"),
            &group.workflows,
            AssetKind::Workflow,
            group_name,
            false,
        )?;
        write_asset_section(
            &mut serialized,
            &format!("groups.{group_name}.instructions"),
            &group.instructions,
            AssetKind::Instruction,
            group_name,
            false,
        )?;
    }
    if !serialized.ends_with('\n') {
        serialized.push('\n');
    }
    std::fs::write(path, serialized)?;
    Ok(())
}

/// Load `cpm.lock` from disk.
pub fn load_lockfile(path: &Path) -> Result<Lockfile, CpmError> {
    if !path.exists() {
        return Ok(Lockfile::new());
    }

    let contents = std::fs::read_to_string(path)?;
    let document: LockfileDocument = toml::from_str(&contents).map_err(|err| CpmError::Parse {
        file: path.display().to_string(),
        msg: err.to_string(),
    })?;

    let generated = document
        .generated
        .or_else(|| {
            document
                .plugins
                .iter()
                .chain(document.skills.iter())
                .chain(document.agents.iter())
                .chain(document.mcps.iter())
                .chain(document.hooks.iter())
                .chain(document.workflows.iter())
                .filter_map(LockfileRecord::resolved_at)
                .max()
        })
        .unwrap_or_else(Utc::now);

    Ok(Lockfile {
        version: document.version,
        generated,
        plugins: document
            .plugins
            .into_iter()
            .map(|record| record.into_resolved_asset(AssetKind::Plugin))
            .collect(),
        skills: document
            .skills
            .into_iter()
            .map(|record| record.into_resolved_asset(AssetKind::Skill))
            .collect(),
        agents: document
            .agents
            .into_iter()
            .map(|record| record.into_resolved_asset(AssetKind::Agent))
            .collect(),
        mcps: document
            .mcps
            .into_iter()
            .map(|record| record.into_resolved_asset(AssetKind::Mcp))
            .collect(),
        hooks: document
            .hooks
            .into_iter()
            .map(|record| record.into_resolved_asset(AssetKind::Hook))
            .collect(),
        workflows: document
            .workflows
            .into_iter()
            .map(|record| record.into_resolved_asset(AssetKind::Workflow))
            .collect(),
        instructions: document
            .instructions
            .into_iter()
            .map(|record| record.into_resolved_asset(AssetKind::Instruction))
            .collect(),
    })
}

/// Persist `cpm.lock`.
pub fn write_lockfile(path: &Path, lockfile: &Lockfile) -> Result<(), CpmError> {
    let plugin_records: Vec<LockfileRecord> =
        lockfile.plugins.iter().map(LockfileRecord::from).collect();
    let skill_records: Vec<LockfileRecord> =
        lockfile.skills.iter().map(LockfileRecord::from).collect();
    let agent_records: Vec<LockfileRecord> =
        lockfile.agents.iter().map(LockfileRecord::from).collect();
    let mcp_records: Vec<LockfileRecord> = lockfile.mcps.iter().map(LockfileRecord::from).collect();
    let hook_records: Vec<LockfileRecord> =
        lockfile.hooks.iter().map(LockfileRecord::from).collect();
    let workflow_records: Vec<LockfileRecord> = lockfile
        .workflows
        .iter()
        .map(LockfileRecord::from)
        .collect();
    let instruction_records: Vec<LockfileRecord> = lockfile
        .instructions
        .iter()
        .map(LockfileRecord::from)
        .collect();

    let document = LockfileDocument {
        version: lockfile.version,
        generated: Some(lockfile.generated),
        plugins: plugin_records.clone(),
        skills: skill_records.clone(),
        agents: agent_records.clone(),
        mcps: mcp_records.clone(),
        hooks: hook_records.clone(),
        workflows: workflow_records.clone(),
        instructions: instruction_records.clone(),
    };
    // Serialize the base document.  The `files` field is `skip_serializing`
    // so it is omitted here — we inject it below with `toml_edit` to
    // guarantee inline-table formatting.
    let base_toml = toml::to_string_pretty(&document)?;

    let mut doc = base_toml
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| CpmError::Parse {
            file: path.display().to_string(),
            msg: e.to_string(),
        })?;

    inject_files_arrays(&mut doc, "plugin", &plugin_records);
    inject_files_arrays(&mut doc, "skill", &skill_records);
    inject_files_arrays(&mut doc, "agent", &agent_records);
    inject_files_arrays(&mut doc, "mcp", &mcp_records);
    inject_files_arrays(&mut doc, "hook", &hook_records);
    inject_files_arrays(&mut doc, "workflow", &workflow_records);
    inject_files_arrays(&mut doc, "instruction", &instruction_records);

    let mut output = doc.to_string();
    if !output.ends_with('\n') {
        output.push('\n');
    }
    std::fs::write(path, output)?;
    Ok(())
}

/// Inject `files` arrays as inline-table entries into a `toml_edit` document.
///
/// `toml::to_string_pretty` would serialize `Vec<LockfileFileEntry>` as nested
/// arrays of tables (`[[kind.files]]`).  Instead we skip `files` from serde
/// and inject them here as `files = [{ path = "…", sha256 = "…" }, …]`.
fn inject_files_arrays(doc: &mut toml_edit::DocumentMut, kind: &str, records: &[LockfileRecord]) {
    let Some(item) = doc.get_mut(kind) else {
        return;
    };
    let Some(array) = item.as_array_of_tables_mut() else {
        return;
    };

    for (i, record) in records.iter().enumerate() {
        if i >= array.len() || record.files.is_empty() {
            continue;
        }
        let Some(table) = array.get_mut(i) else {
            continue;
        };
        let files_array = build_files_inline_array(&record.files);
        // Insert `files` right after `hash` for lockfile readability.  The
        // `toml_edit` table preserves insertion order so placing it before
        // other fields (git_owner, url, etc.) keeps the diff small.
        table.insert("files", toml_edit::Item::Value(files_array));
    }
}

/// Build a `toml_edit::Value::Array` of inline tables from the record's files.
fn build_files_inline_array(files: &[LockfileFileRecord]) -> toml_edit::Value {
    let mut array = toml_edit::Array::new();
    for entry in files {
        match entry {
            LockfileFileRecord::Entry(file) => {
                let mut inline = toml_edit::InlineTable::new();
                inline.insert("path", toml_edit::Value::from(file.path.as_str()));
                if let Some(sha256) = &file.sha256 {
                    inline.insert("sha256", toml_edit::Value::from(sha256.as_str()));
                }
                if file.executable {
                    inline.insert("executable", toml_edit::Value::from(true));
                }
                let mut value = toml_edit::Value::InlineTable(inline);
                value.decor_mut().set_prefix("\n    ");
                array.push(value);
            }
            LockfileFileRecord::LegacyPath(path) => {
                // Shouldn't occur on write, but handle gracefully.
                let mut value = toml_edit::Value::from(path.as_str());
                value.decor_mut().set_prefix("\n    ");
                array.push(value);
            }
        }
    }
    array.set_trailing_comma(true);
    array.set_trailing("\n");
    toml_edit::Value::Array(array)
}

/// Remove all lock entries that match `(kind, name)`, optionally filtered by
/// scope, without touching any other section of the lockfile.
///
/// This is the correct lockfile update for `cpm remove` and `cpm reset`:
/// entries that stay in the manifest are already resolved and must not be
/// re-fetched from their remote sources.
pub fn drop_asset_from_lockfile(
    lockfile: &mut Lockfile,
    kind: AssetKind,
    name: &str,
    scope: Option<Scope>,
) {
    let section: &mut Vec<ResolvedAsset> = match kind {
        AssetKind::Plugin => &mut lockfile.plugins,
        AssetKind::Skill => &mut lockfile.skills,
        AssetKind::Agent => &mut lockfile.agents,
        AssetKind::Mcp => &mut lockfile.mcps,
        AssetKind::Hook => &mut lockfile.hooks,
        AssetKind::Workflow => &mut lockfile.workflows,
        AssetKind::Instruction => &mut lockfile.instructions,
    };
    section.retain(|e| !(e.name == name && scope.is_none_or(|s| e.scope == s)));
}

/// Return the default machine-global cpm lockfile path in `~/.copilot`.
pub fn default_global_lockfile_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".copilot")
        .join("cpm.lock")
}

/// Load the machine-local global lockfile from the default path.
pub fn load_global_lockfile() -> Result<GlobalLockfile, CpmError> {
    load_global_lockfile_from(&default_global_lockfile_path())
}

/// Load a machine-local global lockfile from an explicit path.
pub fn load_global_lockfile_from(path: &Path) -> Result<GlobalLockfile, CpmError> {
    if !path.exists() {
        return Ok(GlobalLockfile::new());
    }

    let contents = std::fs::read_to_string(path)?;
    let document: GlobalLockfileDocument =
        toml::from_str(&contents).map_err(|err| CpmError::Parse {
            file: path.display().to_string(),
            msg: err.to_string(),
        })?;

    let generated = document
        .generated
        .or_else(|| {
            document
                .claims
                .iter()
                .filter_map(GlobalClaimRecord::resolved_at)
                .max()
        })
        .unwrap_or_else(Utc::now);

    Ok(GlobalLockfile {
        version: document.version,
        generated,
        claims: document
            .claims
            .into_iter()
            .map(GlobalClaimRecord::into_claim)
            .collect(),
    })
}

/// Persist a machine-local global lockfile to the default path.
pub fn write_global_lockfile(lockfile: &GlobalLockfile) -> Result<(), CpmError> {
    write_global_lockfile_to(&default_global_lockfile_path(), lockfile)
}

/// Persist a machine-local global lockfile to an explicit path.
pub fn write_global_lockfile_to(path: &Path, lockfile: &GlobalLockfile) -> Result<(), CpmError> {
    let claim_records: Vec<GlobalClaimRecord> = lockfile
        .claims
        .iter()
        .map(GlobalClaimRecord::from)
        .collect();
    let document = GlobalLockfileDocument {
        version: lockfile.version,
        generated: Some(lockfile.generated),
        claims: claim_records.clone(),
    };
    let base_toml = toml::to_string_pretty(&document)?;

    let mut doc = base_toml
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| CpmError::Parse {
            file: path.display().to_string(),
            msg: e.to_string(),
        })?;

    // Global lockfile uses `[[claim]]` with a flattened `LockfileRecord`.
    if let Some(item) = doc.get_mut("claim") {
        if let Some(array) = item.as_array_of_tables_mut() {
            for (i, claim) in claim_records.iter().enumerate() {
                if i >= array.len() || claim.asset.files.is_empty() {
                    continue;
                }
                let Some(table) = array.get_mut(i) else {
                    continue;
                };
                let files_array = build_files_inline_array(&claim.asset.files);
                table.insert("files", toml_edit::Item::Value(files_array));
            }
        }
    }

    let mut output = doc.to_string();
    if !output.ends_with('\n') {
        output.push('\n');
    }
    atomic_write(path, output.as_bytes())?;
    Ok(())
}

/// Add or update a **single** named asset without re-resolving the rest of the
/// manifest.
///
/// `apply_manifest` iterates every entry in every section, which means adding
/// an MCP backed by PyPI would needlessly call the GitHub API for every
/// existing GitHub-hosted plugin in the manifest.  This function only prepares
/// the one asset being added so that unrelated entries are never touched.
///
/// Steps:
/// 1. Prepare only the named asset (resolve its source, generate config/files).
/// 2. Enforce the configured license policy for that single entry.
/// 3. Upsert the resulting lock entry into a copy of `options.existing_lock`
///    (or a fresh `Lockfile` when `existing_lock` is `None`).
/// 4. If `options.install` is `true`, materialize the asset files on disk.
pub async fn add_single_asset(
    kind: AssetKind,
    name: &str,
    source: &AssetSource,
    client: &reqwest::Client,
    token: Option<&str>,
    options: ApplyOptions<'_>,
) -> Result<Lockfile, CpmError> {
    let generated = Utc::now();

    let prepared = prepare_asset(
        kind,
        name,
        source,
        client,
        token,
        options.repo_root,
        None,
        generated,
        options.download_progress,
        options.source_rules,
    )
    .await?;

    enforce_license_policy(&prepared.lock, options.settings)?;

    // Build the updated lockfile from the existing one (or a blank slate).
    let mut lockfile = options
        .existing_lock
        .cloned()
        .unwrap_or_else(|| Lockfile::with_generated(generated));

    // Upsert: replace any existing entry with the same name, otherwise append.
    let section: &mut Vec<ResolvedAsset> = match kind {
        AssetKind::Plugin => &mut lockfile.plugins,
        AssetKind::Skill => &mut lockfile.skills,
        AssetKind::Agent => &mut lockfile.agents,
        AssetKind::Mcp => &mut lockfile.mcps,
        AssetKind::Hook => &mut lockfile.hooks,
        AssetKind::Workflow => &mut lockfile.workflows,
        AssetKind::Instruction => &mut lockfile.instructions,
    };
    if let Some(pos) = section.iter().position(|e| e.name == name) {
        section[pos] = prepared.lock.clone();
    } else {
        section.push(prepared.lock.clone());
    }

    if options.install {
        install_prepared_asset(
            &prepared,
            options.repo_root,
            options.settings.auto_compile_workflows,
        )?;
    }

    Ok(lockfile)
}

/// Resolve the manifest into a lockfile and optionally materialize the selected assets.
pub async fn apply_manifest(
    manifest: &Manifest,
    client: &reqwest::Client,
    token: Option<&str>,
    options: ApplyOptions<'_>,
) -> Result<Lockfile, CpmError> {
    let mut prepared_assets = Vec::new();
    let generated = Utc::now();
    let mut lockfile = Lockfile::with_generated(generated);
    let plugins = manifest.effective_section(AssetKind::Plugin);
    let skills = manifest.effective_section(AssetKind::Skill);
    let agents = manifest.effective_section(AssetKind::Agent);
    let mcps = manifest.effective_section(AssetKind::Mcp);
    let hooks = manifest.effective_section(AssetKind::Hook);
    let workflows = manifest.effective_section(AssetKind::Workflow);
    let instructions = manifest.effective_section(AssetKind::Instruction);

    prepare_section(
        &plugins,
        AssetKind::Plugin,
        &mut prepared_assets,
        client,
        token,
        options,
        generated,
    )
    .await?;
    prepare_section(
        &skills,
        AssetKind::Skill,
        &mut prepared_assets,
        client,
        token,
        options,
        generated,
    )
    .await?;
    prepare_section(
        &agents,
        AssetKind::Agent,
        &mut prepared_assets,
        client,
        token,
        options,
        generated,
    )
    .await?;
    prepare_section(
        &mcps,
        AssetKind::Mcp,
        &mut prepared_assets,
        client,
        token,
        options,
        generated,
    )
    .await?;
    prepare_section(
        &hooks,
        AssetKind::Hook,
        &mut prepared_assets,
        client,
        token,
        options,
        generated,
    )
    .await?;
    prepare_section(
        &workflows,
        AssetKind::Workflow,
        &mut prepared_assets,
        client,
        token,
        options,
        generated,
    )
    .await?;
    prepare_section(
        &instructions,
        AssetKind::Instruction,
        &mut prepared_assets,
        client,
        token,
        options,
        generated,
    )
    .await?;

    for prepared in &prepared_assets {
        match prepared.lock.kind {
            AssetKind::Plugin => lockfile.plugins.push(prepared.lock.clone()),
            AssetKind::Skill => lockfile.skills.push(prepared.lock.clone()),
            AssetKind::Agent => lockfile.agents.push(prepared.lock.clone()),
            AssetKind::Mcp => lockfile.mcps.push(prepared.lock.clone()),
            AssetKind::Hook => lockfile.hooks.push(prepared.lock.clone()),
            AssetKind::Workflow => lockfile.workflows.push(prepared.lock.clone()),
            AssetKind::Instruction => lockfile.instructions.push(prepared.lock.clone()),
        }
    }

    // Carry forward ownership overrides from the existing lockfile so that
    // assets previously marked `user` are not silently reset to `upstream`
    // on every sync run.
    if let Some(existing) = options.existing_lock {
        carry_forward_ownership(&mut lockfile, existing);
    }

    detect_conflicts(&lockfile)?;
    for asset in lockfile.all_assets() {
        enforce_license_policy(asset, options.settings)?;
    }

    if options.install {
        // Re-derive prepared assets with merged ownership so
        // install_prepared_asset sees the correct ownership value.
        let mut owned_prepared = prepared_assets.clone();
        if options.existing_lock.is_some() {
            for pa in &mut owned_prepared {
                if let Some(locked) = lockfile
                    .all_assets()
                    .find(|a| a.kind == pa.lock.kind && a.name == pa.lock.name)
                {
                    pa.lock.ownership = locked.ownership;
                }
            }
        }
        for prepared in &owned_prepared {
            if should_install(
                &prepared.lock.source,
                options.install_group,
                options.install_scope,
            ) {
                install_prepared_asset(
                    prepared,
                    options.repo_root,
                    options.settings.auto_compile_workflows,
                )?;
            }
        }
    }

    Ok(lockfile)
}

/// Carry forward [`AssetOwnership::User`] (and [`AssetOwnership::Generated`])
/// from an existing lockfile into a newly resolved lockfile.
///
/// Only non-default ownership states are propagated; assets that were
/// `upstream` in the old lock (or not present) keep the freshly resolved
/// default.
fn carry_forward_ownership(new_lock: &mut Lockfile, existing: &Lockfile) {
    // Build a lookup map: (kind, name) → ownership for any non-upstream entry
    // from the existing lock.
    let overrides: std::collections::HashMap<(AssetKind, &str), AssetOwnership> = existing
        .all_assets()
        .filter(|a| a.ownership != AssetOwnership::Upstream)
        .map(|a| ((a.kind, a.name.as_str()), a.ownership))
        .collect();

    if overrides.is_empty() {
        return;
    }

    let update = |assets: &mut Vec<ResolvedAsset>| {
        for asset in assets.iter_mut() {
            if let Some(&ownership) = overrides.get(&(asset.kind, asset.name.as_str())) {
                asset.ownership = ownership;
            }
        }
    };

    update(&mut new_lock.plugins);
    update(&mut new_lock.skills);
    update(&mut new_lock.agents);
    update(&mut new_lock.mcps);
    update(&mut new_lock.hooks);
    update(&mut new_lock.workflows);
}

async fn prepare_section(
    section: &indexmap::IndexMap<String, AssetSource>,
    kind: AssetKind,
    destination: &mut Vec<PreparedAsset>,
    client: &reqwest::Client,
    token: Option<&str>,
    options: ApplyOptions<'_>,
    resolved_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), CpmError> {
    for (name, source) in section {
        // Short-circuit: when an existing lock entry has the same source and
        // we can prove the content is still valid, reuse it without any
        // network call.  This is safe in three situations:
        //
        //   1. `install = false` – the caller only needs the lock record, not
        //      the file bytes (e.g. `cpm remove`, `cpm reset`).
        //   2. MCP assets – they are pure config; `prepare_mcp_asset` would
        //      only regenerate the same JSON from the same transport spec.
        //   3. Pinned to an immutable full commit SHA – the content at that
        //      rev can never change, so re-fetching wastes bandwidth/quota.
        if let Some(existing) = options.existing_lock {
            if let Some(existing_entry) = existing
                .all_assets()
                .find(|a| a.kind == kind && a.name == name.as_str())
            {
                if existing_entry.source == *source
                    && can_reuse_without_refetch(
                        kind,
                        &existing_entry.resolved_rev,
                        options.install,
                    )
                {
                    destination.push(PreparedAsset {
                        lock: existing_entry.clone(),
                        // No file bytes needed: either install=false (caller
                        // won't materialize files) or the asset is an MCP
                        // (config is regenerated at install time by the
                        // installer, not from these bytes) or the rev is a
                        // pinned SHA whose on-disk files are already correct.
                        files: vec![],
                        mcp_entry: None,
                    });
                    continue;
                }
            }
        }

        let prepared = prepare_asset(
            kind,
            name,
            source,
            client,
            token,
            options.repo_root,
            None,
            resolved_at,
            options.download_progress,
            options.source_rules,
        )
        .await?;
        destination.push(prepared);
    }

    Ok(())
}

/// Return `true` when a prepared asset from the existing lockfile can be
/// reused as-is without calling `prepare_asset` again.
///
/// Rules:
/// - When `install` is `false` the caller never materializes files, so any
///   matching locked entry is sufficient.
/// - MCP assets carry no remote file content; `prepare_mcp_asset` only
///   derives the `.mcp.json` config from the transport spec, which the
///   installer regenerates on demand.
/// - A fully-pinned 40-hex commit SHA is content-addressable and immutable.
fn can_reuse_without_refetch(kind: AssetKind, resolved_rev: &str, install: bool) -> bool {
    if !install {
        return true;
    }
    if kind == AssetKind::Mcp {
        return true;
    }
    // Full 40-character hex SHA → content is immutable.
    resolved_rev.len() == 40 && resolved_rev.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Install a previously locked asset by re-materializing it from its pinned source.
///
/// The [`ResolvedAsset::ownership`] field is respected: assets with
/// [`AssetOwnership::User`] ownership will not overwrite files that are
/// already present on disk.
pub async fn install_resolved_asset(
    asset: &ResolvedAsset,
    client: &reqwest::Client,
    token: Option<&str>,
    repo_root: &Path,
    download_progress: Option<&dyn DownloadProgress>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<(), CpmError> {
    let mut prepared = prepare_asset(
        asset.kind,
        &asset.name,
        &asset.source,
        client,
        token,
        repo_root,
        Some(asset.resolved_rev.as_str()),
        asset.resolved_date,
        download_progress,
        source_rules,
    )
    .await?;
    // Propagate the ownership from the lock entry so the installer can make
    // the correct overwrite decision.
    prepared.lock.ownership = asset.ownership;
    install_prepared_asset(&prepared, repo_root, false)
}

fn should_install(
    source: &AssetSource,
    install_group: Option<&str>,
    install_scope: Option<Scope>,
) -> bool {
    let group_matches = source.group == "default"
        || install_group
            .map(|group| source.group == group)
            .unwrap_or(false);
    let scope_matches = install_scope
        .map(|scope| source.scope == scope)
        .unwrap_or(true);
    group_matches && scope_matches
}

#[allow(clippy::too_many_arguments)]
async fn prepare_asset(
    kind: AssetKind,
    name: &str,
    source: &AssetSource,
    client: &reqwest::Client,
    token: Option<&str>,
    repo_root: &Path,
    pinned_rev_override: Option<&str>,
    resolved_at: chrono::DateTime<chrono::Utc>,
    download_progress: Option<&dyn DownloadProgress>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<PreparedAsset, CpmError> {
    validate_asset_scope(kind, source.scope)?;
    if kind == AssetKind::Mcp {
        return prepare_mcp_asset(
            name,
            source,
            client,
            token,
            repo_root,
            pinned_rev_override,
            resolved_at,
            source_rules,
        )
        .await;
    }

    let resolved_rev = match pinned_rev_override {
        Some(rev) => rev.to_owned(),
        None => resolve_pinned_rev(
            client,
            token,
            source.url.as_deref(),
            source.rev.as_deref(),
            source_rules,
        )
        .await?
        .or_else(|| source.rev.clone())
        .unwrap_or_default(),
    };

    let source_files = if let Some(path) = &source.path {
        collect_local_source_files(path, repo_root)?
    } else if let Some(url) = &source.url {
        collect_remote_source_files(
            kind,
            url,
            &resolved_rev,
            client,
            token,
            download_progress,
            source_rules,
        )
        .await?
    } else {
        return Err(CpmError::InvalidSource {
            input: name.to_owned(),
            reason: "asset source must define either `url` or `path`".to_owned(),
        });
    };

    if source_files.is_empty() {
        return Err(CpmError::InvalidSource {
            input: name.to_owned(),
            reason: "asset source resolved to no installable files".to_owned(),
        });
    }

    let files: Vec<PreparedFile> = source_files
        .into_iter()
        .map(|file| PreparedFile {
            relative_path: install_relative_path(kind, name, &file.relative_path),
            bytes: file.bytes,
        })
        .collect();
    let sub_assets = detect_sub_assets(kind, &files);

    let (hash, locked_files) = hash_materialized_files(kind, &files);
    let legacy_executable: Vec<_> = locked_files
        .iter()
        .filter(|file| file.executable)
        .map(|file| file.path.clone())
        .collect();
    let legacy_file_hashes: IndexMap<_, _> = locked_files
        .iter()
        .filter_map(|file| {
            file.sha256
                .as_ref()
                .map(|sha256| (file.path.clone(), format!("sha256:{sha256}")))
        })
        .collect();
    let license = detect_license(
        source,
        &resolved_rev,
        repo_root,
        client,
        token,
        source_rules,
    )
    .await?;
    let lock = ResolvedAsset {
        name: name.to_owned(),
        kind,
        source: source.clone(),
        resolved_rev,
        resolved_date: resolved_at,
        hash,
        scope: source.scope,
        ownership: AssetOwnership::Upstream,
        files: locked_files,
        executable: legacy_executable,
        file_hashes: legacy_file_hashes,
        git: extract_git_metadata(source),
        sub_assets,
        license,
        bin_path: None,
        compiled_path: expected_compiled_path(kind, &files),
        plugin_meta: None,
    };

    Ok(PreparedAsset {
        lock,
        files,
        mcp_entry: None,
    })
}

fn validate_asset_scope(kind: AssetKind, scope: Scope) -> Result<(), CpmError> {
    if kind == AssetKind::Workflow && scope == Scope::Global {
        return Err(CpmError::InvalidConfig {
            key: "scope".to_owned(),
            reason: "workflow assets are local-only and cannot be installed in global scope"
                .to_owned(),
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn prepare_mcp_asset(
    name: &str,
    source: &AssetSource,
    client: &reqwest::Client,
    token: Option<&str>,
    repo_root: &Path,
    pinned_rev_override: Option<&str>,
    resolved_at: chrono::DateTime<chrono::Utc>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<PreparedAsset, CpmError> {
    let resolved_rev = match pinned_rev_override {
        Some(rev) => rev.to_owned(),
        None => match source.transport.as_ref() {
            Some(transport @ (McpTransport::Npx { .. } | McpTransport::Uvx { .. })) => {
                if let Some(rev) = source.rev.clone() {
                    rev
                } else {
                    resolve_package_transport_version(client, transport, source_rules)
                        .await?
                        .unwrap_or_default()
                }
            }
            Some(McpTransport::Docker { image, .. }) => source
                .rev
                .clone()
                .or_else(|| docker_image_pin(image))
                .unwrap_or_default(),
            _ => resolve_pinned_rev(
                client,
                token,
                source.url.as_deref(),
                source.rev.as_deref(),
                source_rules,
            )
            .await?
            .or_else(|| source.rev.clone())
            .unwrap_or_default(),
        },
    };

    let license = detect_license(
        source,
        &resolved_rev,
        repo_root,
        client,
        token,
        source_rules,
    )
    .await?;

    // Build the Copilot server entry using the runtime-rewritten transport
    // (source-rule URL substitutions applied).  This is what gets written to
    // the aggregate Copilot MCP config at install time.
    let mut runtime_lock = ResolvedAsset {
        name: name.to_owned(),
        kind: AssetKind::Mcp,
        source: source.clone(),
        resolved_rev: resolved_rev.clone(),
        resolved_date: resolved_at,
        hash: String::new(),
        scope: source.scope,
        ownership: AssetOwnership::Upstream,
        files: vec![],
        executable: vec![],
        file_hashes: IndexMap::new(),
        git: None,
        sub_assets: vec![],
        license: license.clone(),
        bin_path: None,
        compiled_path: None,
        plugin_meta: None,
    };
    runtime_lock.source = rewrite_mcp_source(source, source_rules);

    // Distinguish "Binary transport missing bin_path" from "no transport at all"
    // to give a clearer error when someone has a binary entry without a resolved
    // executable path (e.g. a manifest entry left over before --release was rejected).
    if matches!(
        runtime_lock.source.transport,
        Some(McpTransport::Binary { .. })
    ) && runtime_lock.bin_path.is_none()
    {
        return Err(CpmError::InvalidSource {
            input: name.to_owned(),
            reason: "binary MCP transport requires a resolved executable path (bin_path); \
                     --release is not yet implemented — use --uvx, --npx, --docker, --script, \
                     or --path instead"
                .to_owned(),
        });
    }

    let mcp_entry = copilot_server_entry(&runtime_lock).ok_or_else(|| CpmError::InvalidSource {
        input: name.to_owned(),
        reason: "MCP entries must define a transport".to_owned(),
    })?;
    let entry_bytes = serde_json::to_vec_pretty(&mcp_entry)?;

    // The lock stored in the lockfile uses the ORIGINAL source (not rewritten)
    // so that source-rule rewrites are always applied fresh at install time.
    // `files` is empty — MCP config lives in the aggregate Copilot config, not
    // as a per-server file tracked by the lockfile.
    let mut lock = ResolvedAsset {
        name: name.to_owned(),
        kind: AssetKind::Mcp,
        source: source.clone(),
        resolved_rev,
        resolved_date: resolved_at,
        hash: sha256_hex(&entry_bytes),
        scope: source.scope,
        ownership: AssetOwnership::Upstream,
        files: vec![],
        executable: vec![],
        file_hashes: IndexMap::new(),
        git: None,
        sub_assets: vec![],
        license,
        bin_path: None,
        compiled_path: None,
        plugin_meta: None,
    };
    lock.git = extract_git_metadata(source);

    Ok(PreparedAsset {
        lock,
        files: vec![],
        mcp_entry: Some(mcp_entry),
    })
}

fn install_prepared_asset(
    prepared: &PreparedAsset,
    repo_root: &Path,
    auto_compile_workflows: bool,
) -> Result<(), CpmError> {
    // MCP assets use the aggregate Copilot config, not per-server files.
    if prepared.lock.kind == AssetKind::Mcp {
        // Always clean up any legacy per-server file written by older cpm.
        remove_legacy_mcp_file(&prepared.lock.name, prepared.lock.scope, repo_root)?;

        if let Some(entry) = &prepared.mcp_entry {
            // Fresh prepare: write the pre-computed entry (from the runtime-
            // rewritten source) directly into the aggregate config.
            write_mcp_server_entry(
                &prepared.lock.name,
                entry.clone(),
                prepared.lock.scope,
                repo_root,
            )?;
        } else {
            // Reuse path (can_reuse_without_refetch): fall back to install_asset
            // which recomputes the entry from the lock's stored source.
            install_asset(&prepared.lock, repo_root)?;
        }
        return Ok(());
    }

    let base = install_dir(prepared.lock.kind, prepared.lock.scope, repo_root);
    std::fs::create_dir_all(&base)?;

    for file in &prepared.files {
        let destination = base.join(file.relative_path.as_std_path());

        // User-owned assets must never be silently overwritten by sync or
        // install.  If the file already exists on disk, skip it and leave the
        // developer's version intact.  A first-time install (file absent) is
        // always allowed so the asset is seeded even for user-owned entries.
        if prepared.lock.ownership == AssetOwnership::User && destination.exists() {
            tracing::debug!(
                "skipping user-owned file {} (ownership = user)",
                destination.display()
            );
            continue;
        }

        atomic_write(&destination, &file.bytes)?;
        if prepared
            .lock
            .files
            .iter()
            .find(|entry| entry.path == file.relative_path)
            .is_some_and(|entry| entry.executable)
        {
            make_executable(&destination)?;
        }
        info!("wrote {}", destination.display());
    }

    if prepared.lock.kind == AssetKind::Workflow && auto_compile_workflows {
        if let Some(relative_path) = prepared.lock.files.first().map(|file| &file.path) {
            let workflow_path = base.join(relative_path.as_std_path());
            let output = std::process::Command::new("gh")
                .args(["aw", "compile"])
                .arg(&workflow_path)
                .output()
                .map_err(|err| CpmError::WorkflowCompileFailed {
                    msg: err.to_string(),
                })?;
            if !output.status.success() {
                return Err(CpmError::WorkflowCompileFailed {
                    msg: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                });
            }
            if let Some(compiled_path) = prepared.lock.compiled_path.as_ref() {
                let compiled = base.join(compiled_path.as_std_path());
                if !compiled.exists() {
                    return Err(CpmError::WorkflowCompileFailed {
                        msg: format!("`gh aw compile` did not produce {}", compiled.display()),
                    });
                }
            }
        }
    }

    Ok(())
}

fn hash_materialized_files(kind: AssetKind, files: &[PreparedFile]) -> (String, Vec<LockedFile>) {
    let mut sorted: Vec<_> = files
        .iter()
        .map(|file| {
            (
                file.relative_path.as_str().to_owned(),
                file.bytes.as_slice(),
            )
        })
        .collect();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));

    let mut hasher = Sha256::new();
    let mut locked_files = Vec::with_capacity(sorted.len());
    for (path, bytes) in sorted {
        let file_hash = sha256_hex(bytes);
        hasher.update(file_hash.as_bytes());
        locked_files.push(LockedFile {
            path: Utf8PathBuf::from(path.clone()),
            sha256: Some(strip_sha256_prefix(&file_hash)),
            executable: kind == AssetKind::Hook && path.ends_with(".sh"),
        });
    }
    (
        format!("sha256:{}", hex::encode(hasher.finalize())),
        locked_files,
    )
}

fn expected_compiled_path(kind: AssetKind, files: &[PreparedFile]) -> Option<Utf8PathBuf> {
    if kind != AssetKind::Workflow {
        return None;
    }

    let workflow = files.first()?.relative_path.clone();
    let stem = workflow.file_stem()?;
    let parent = workflow.parent().map(Utf8PathBuf::from).unwrap_or_default();
    Some(parent.join(format!("{stem}.lock.yml")))
}

#[derive(Debug, Default, Deserialize)]
struct PluginBundleManifest {
    #[serde(default)]
    agents: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
}

fn extract_git_metadata(source: &AssetSource) -> Option<GitMetadata> {
    let github = parse_github_source(source.url.as_deref()?)?;
    Some(GitMetadata {
        owner: github.owner,
        repo: github.repo,
        reference: source.rev.clone().unwrap_or(github.git_ref),
        path: Utf8PathBuf::from(github.path),
        mode: match github.mode {
            GitHubSourceMode::Blob => GitSourceKind::Blob,
            GitHubSourceMode::Tree => GitSourceKind::Tree,
        },
    })
}

fn detect_sub_assets(kind: AssetKind, files: &[PreparedFile]) -> Vec<SubAsset> {
    if kind != AssetKind::Plugin || files.is_empty() {
        return Vec::new();
    }

    let Some(plugin_name) = files[0]
        .relative_path
        .components()
        .next()
        .map(|component| component.as_str().to_owned())
    else {
        return Vec::new();
    };

    let manifest = parse_plugin_bundle_manifest(files);
    let mut agent_roots = vec!["agents".to_owned()];
    let mut skill_roots = vec!["skills".to_owned()];
    if let Some(manifest) = manifest {
        agent_roots.extend(
            manifest
                .agents
                .into_iter()
                .map(normalize_bundle_declared_path),
        );
        skill_roots.extend(
            manifest
                .skills
                .into_iter()
                .map(normalize_bundle_declared_path),
        );
    }

    let mut detected = Vec::new();
    for file in files {
        let path = file.relative_path.as_str();
        let Some(remainder) = path
            .strip_prefix(&plugin_name)
            .and_then(|rest| rest.strip_prefix('/'))
        else {
            continue;
        };

        if matches_declared_sub_asset(remainder, &agent_roots) && is_nested_agent_path(remainder) {
            let file_name = file
                .relative_path
                .file_name()
                .map(str::to_owned)
                .unwrap_or_else(|| remainder.to_owned());
            let name = file_name
                .strip_suffix(".agent.md")
                .or_else(|| file_name.strip_suffix(".md"))
                .unwrap_or(file_name.as_str())
                .to_owned();
            detected.push(SubAsset {
                name,
                kind: AssetKind::Agent,
                path: file.relative_path.clone(),
                ownership: SubAssetOwnership::Parent,
            });
            continue;
        }

        if matches_declared_sub_asset(remainder, &skill_roots) && remainder.ends_with("/SKILL.md") {
            let Some(skill_root) = file.relative_path.parent() else {
                continue;
            };
            let Some(name) = skill_root.file_name().map(str::to_owned) else {
                continue;
            };
            detected.push(SubAsset {
                name,
                kind: AssetKind::Skill,
                path: skill_root.to_owned(),
                ownership: SubAssetOwnership::Parent,
            });
        }
    }

    detected.sort_by(|left, right| {
        left.kind
            .to_string()
            .cmp(&right.kind.to_string())
            .then_with(|| left.path.cmp(&right.path))
    });
    detected.dedup_by(|left, right| left.kind == right.kind && left.path == right.path);
    detected
}

fn parse_plugin_bundle_manifest(files: &[PreparedFile]) -> Option<PluginBundleManifest> {
    let manifest = files.iter().find(|file| {
        file.relative_path
            .as_str()
            .ends_with("/.github/plugin/plugin.json")
    })?;
    serde_json::from_slice(&manifest.bytes).ok()
}

fn normalize_bundle_declared_path(path: String) -> String {
    path.trim()
        .trim_start_matches("./")
        .trim_matches('/')
        .to_owned()
}

fn matches_declared_sub_asset(path: &str, roots: &[String]) -> bool {
    roots
        .iter()
        .filter(|root| !root.is_empty())
        .any(|root| path == root || path.starts_with(&format!("{root}/")))
}

fn is_nested_agent_path(path: &str) -> bool {
    path.ends_with(".md")
        && path.rsplit('/').next().is_some_and(|file_name| {
            !file_name.eq_ignore_ascii_case("README.md")
                && !file_name.eq_ignore_ascii_case("SKILL.md")
        })
}

fn install_relative_path(
    kind: AssetKind,
    name: &str,
    source_relative_path: &Utf8PathBuf,
) -> Utf8PathBuf {
    match kind {
        AssetKind::Plugin | AssetKind::Skill | AssetKind::Hook => {
            Utf8PathBuf::from(name).join(source_relative_path)
        }
        AssetKind::Agent | AssetKind::Mcp | AssetKind::Workflow => source_relative_path.clone(),
        AssetKind::Instruction => {
            let file_name = source_relative_path
                .file_name()
                .map(normalize_instruction_file_name)
                .unwrap_or_else(|| Utf8PathBuf::from(format!("{name}.instructions.md")));
            source_relative_path
                .parent()
                .map(Utf8PathBuf::from)
                .unwrap_or_default()
                .join(file_name)
        }
    }
}

fn normalize_instruction_file_name(file_name: &str) -> Utf8PathBuf {
    if let Some(stem) = file_name.strip_suffix(".instructions.md") {
        return Utf8PathBuf::from(format!("{stem}.instructions.md"));
    }
    if let Some(stem) = file_name.strip_suffix(".md") {
        return Utf8PathBuf::from(format!("{stem}.instructions.md"));
    }
    Utf8PathBuf::from(format!("{file_name}.instructions.md"))
}

fn collect_local_source_files(
    source_path: &Utf8PathBuf,
    repo_root: &Path,
) -> Result<Vec<SourceFile>, CpmError> {
    let root = resolve_local_source_path(source_path, repo_root);
    if root.is_dir() {
        let mut files = Vec::new();
        collect_local_directory_files(&root, &root, &mut files)?;
        return Ok(files);
    }

    if root.is_file() {
        return Ok(vec![SourceFile {
            relative_path: utf8_file_name(&root)?,
            bytes: std::fs::read(&root)?,
        }]);
    }

    Err(CpmError::InvalidSource {
        input: source_path.to_string(),
        reason: "local asset path no longer exists".to_owned(),
    })
}

fn resolve_local_source_path(source_path: &Utf8PathBuf, repo_root: &Path) -> PathBuf {
    let path = PathBuf::from(source_path.as_str());
    if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    }
}

fn collect_local_directory_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<SourceFile>,
) -> Result<(), CpmError> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_local_directory_files(root, &path, files)?;
            continue;
        }

        let relative = path.strip_prefix(root).map_err(|err| CpmError::Parse {
            file: path.display().to_string(),
            msg: err.to_string(),
        })?;
        let relative = Utf8PathBuf::from_path_buf(relative.to_path_buf()).map_err(|_| {
            CpmError::InvalidSource {
                input: path.display().to_string(),
                reason: "local asset paths must be valid UTF-8".to_owned(),
            }
        })?;

        files.push(SourceFile {
            relative_path: relative,
            bytes: std::fs::read(&path)?,
        });
    }

    Ok(())
}

async fn collect_remote_source_files(
    kind: AssetKind,
    source_url: &str,
    resolved_rev: &str,
    client: &reqwest::Client,
    token: Option<&str>,
    download_progress: Option<&dyn DownloadProgress>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<Vec<SourceFile>, CpmError> {
    if let Some(github) = parse_github_source(source_url) {
        return collect_github_source_files(
            kind,
            &github,
            resolved_rev,
            client,
            token,
            download_progress,
            source_rules,
        )
        .await;
    }

    let target = rewrite_source_url(source_url, source_rules);
    let url = Url::parse(&target.url).map_err(|_| CpmError::UnsupportedUrl {
        url: source_url.to_owned(),
    })?;
    let bytes = fetch_bytes(
        client,
        &target.url,
        target.token.as_deref().or(token),
        download_progress,
    )
    .await?;
    let file_name = url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
        .unwrap_or("asset");

    Ok(vec![SourceFile {
        relative_path: Utf8PathBuf::from(file_name),
        bytes,
    }])
}

async fn collect_github_source_files(
    kind: AssetKind,
    github: &GitHubSource,
    resolved_rev: &str,
    client: &reqwest::Client,
    token: Option<&str>,
    download_progress: Option<&dyn DownloadProgress>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<Vec<SourceFile>, CpmError> {
    match github.mode {
        GitHubSourceMode::Blob => {
            let bytes = fetch_github_bytes(
                client,
                &github_raw_url(&github.owner, &github.repo, resolved_rev, &github.path),
                token,
                download_progress,
                source_rules,
            )
            .await?;
            Ok(vec![SourceFile {
                relative_path: Utf8PathBuf::from(
                    github
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or(github.path.as_str()),
                ),
                bytes,
            }])
        }
        GitHubSourceMode::Tree => {
            if matches!(kind, AssetKind::Agent | AssetKind::Workflow) {
                return Err(CpmError::InvalidSource {
                    input: format!(
                        "https://github.com/{}/{}/tree/{}/{}",
                        github.owner, github.repo, github.git_ref, github.path
                    ),
                    reason: format!("{kind} sources must point to a single file"),
                });
            }
            collect_github_directory_files(
                github,
                resolved_rev,
                client,
                token,
                download_progress,
                source_rules,
            )
            .await
        }
    }
}

async fn collect_github_directory_files(
    github: &GitHubSource,
    resolved_rev: &str,
    client: &reqwest::Client,
    token: Option<&str>,
    download_progress: Option<&dyn DownloadProgress>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<Vec<SourceFile>, CpmError> {
    let mut pending = vec![github.path.clone()];
    let mut file_paths = Vec::new();

    while let Some(path) = pending.pop() {
        let api_url = format!(
            "https://api.github.com/repos/{owner}/{repo}/contents/{path}?ref={resolved_rev}",
            owner = github.owner,
            repo = github.repo,
        );
        let target = rewrite_source_url(&api_url, source_rules);
        let mut request = client.get(&target.url);
        if let Some(token) = target.token.as_deref().or(token) {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(CpmError::AuthRequired {
                url: target.url.clone(),
            });
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(CpmError::InvalidSource {
                input: format!(
                    "https://github.com/{}/{}/tree/{}/{}",
                    github.owner, github.repo, github.git_ref, github.path
                ),
                reason: "GitHub asset directory could not be fetched".to_owned(),
            });
        }

        let value: serde_json::Value = response.error_for_status()?.json().await?;
        let entries = value.as_array().ok_or_else(|| CpmError::InvalidSource {
            input: target.url.clone(),
            reason: "expected a GitHub directory listing".to_owned(),
        })?;

        for entry in entries {
            let entry: GitHubContentEntry = serde_json::from_value(entry.clone())?;
            match entry.kind.as_str() {
                "dir" => pending.push(entry.path),
                "file" => file_paths.push(entry.path),
                _ => {}
            }
        }
    }

    file_paths.sort();

    let owner = github.owner.clone();
    let repo = github.repo.clone();
    let root_path = github.path.clone();
    let resolved_rev = resolved_rev.to_owned();
    let mut files: Vec<SourceFile> = stream::iter(file_paths.into_iter().map(|entry_path| {
        let owner = owner.clone();
        let repo = repo.clone();
        let root_path = root_path.clone();
        let resolved_rev = resolved_rev.clone();
        async move {
            let bytes = fetch_github_bytes(
                client,
                &github_raw_url(&owner, &repo, &resolved_rev, &entry_path),
                token,
                download_progress,
                source_rules,
            )
            .await?;
            let relative = entry_path
                .strip_prefix(root_path.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .unwrap_or(entry_path.as_str())
                .to_owned();
            Ok::<SourceFile, CpmError>(SourceFile {
                relative_path: Utf8PathBuf::from(relative),
                bytes,
            })
        }
    }))
    .buffer_unordered(GITHUB_TREE_FETCH_CONCURRENCY)
    .try_collect()
    .await?;

    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn github_raw_url(owner: &str, repo: &str, rev: &str, path: &str) -> String {
    format!("https://raw.githubusercontent.com/{owner}/{repo}/{rev}/{path}")
}

fn rewrite_mcp_source(
    source: &AssetSource,
    source_rules: &IndexMap<String, SourceRule>,
) -> AssetSource {
    let mut rewritten = source.clone();
    rewritten.transport = rewritten.transport.map(|transport| match transport {
        McpTransport::Http { url } => McpTransport::Http {
            url: rewrite_source_url(&url, source_rules).url,
        },
        McpTransport::Sse { url } => McpTransport::Sse {
            url: rewrite_source_url(&url, source_rules).url,
        },
        McpTransport::Binary { url, bin, args } => McpTransport::Binary {
            url: rewrite_source_url(&url, source_rules).url,
            bin,
            args,
        },
        other => other,
    });
    rewritten
}

async fn fetch_github_bytes(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    download_progress: Option<&dyn DownloadProgress>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<Vec<u8>, CpmError> {
    let target = rewrite_source_url(url, source_rules);
    fetch_bytes(
        client,
        &target.url,
        target.token.as_deref().or(token),
        download_progress,
    )
    .await
}

fn utf8_file_name(path: &Path) -> Result<Utf8PathBuf, CpmError> {
    let file_name = path.file_name().ok_or_else(|| CpmError::InvalidSource {
        input: path.display().to_string(),
        reason: "expected a file name".to_owned(),
    })?;
    let file_name = file_name.to_str().ok_or_else(|| CpmError::InvalidSource {
        input: path.display().to_string(),
        reason: "file names must be valid UTF-8".to_owned(),
    })?;
    Ok(Utf8PathBuf::from(file_name))
}

impl From<&ResolvedAsset> for LockfileRecord {
    fn from(asset: &ResolvedAsset) -> Self {
        let (transport, package, entrypoint, image, bin, command, args, url, path) =
            match asset.source.transport.as_ref() {
                Some(McpTransport::Http { .. }) => (
                    Some(LockfileTransport::Name("http".into())),
                    None,
                    None,
                    None,
                    None,
                    None,
                    asset.source.args.clone(),
                    asset.source.url.clone(),
                    asset.source.path.clone(),
                ),
                Some(McpTransport::Sse { .. }) => (
                    Some(LockfileTransport::Name("sse".into())),
                    None,
                    None,
                    None,
                    None,
                    None,
                    asset.source.args.clone(),
                    asset.source.url.clone(),
                    asset.source.path.clone(),
                ),
                Some(McpTransport::Npx {
                    package,
                    entrypoint,
                    args,
                }) => (
                    Some(LockfileTransport::Name("npx".into())),
                    Some(package.clone()),
                    entrypoint.clone(),
                    None,
                    None,
                    None,
                    args.clone(),
                    asset.source.url.clone(),
                    asset.source.path.clone(),
                ),
                Some(McpTransport::Uvx {
                    package,
                    entrypoint,
                    args,
                }) => (
                    Some(LockfileTransport::Name("uvx".into())),
                    Some(package.clone()),
                    entrypoint.clone(),
                    None,
                    None,
                    None,
                    args.clone(),
                    asset.source.url.clone(),
                    asset.source.path.clone(),
                ),
                Some(McpTransport::Docker { image, args }) => (
                    Some(LockfileTransport::Name("docker".into())),
                    None,
                    None,
                    Some(image.clone()),
                    None,
                    None,
                    args.clone(),
                    asset.source.url.clone(),
                    asset.source.path.clone(),
                ),
                Some(McpTransport::Binary { url, bin, args }) => (
                    Some(LockfileTransport::Name("binary".into())),
                    None,
                    None,
                    None,
                    Some(bin.clone()),
                    None,
                    args.clone(),
                    Some(url.clone()),
                    asset.source.path.clone(),
                ),
                Some(McpTransport::Path { path, args }) => (
                    Some(LockfileTransport::Name("path".into())),
                    None,
                    None,
                    None,
                    None,
                    None,
                    args.clone(),
                    asset.source.url.clone(),
                    Some(
                        Utf8PathBuf::from_path_buf(path.clone()).unwrap_or_else(|path| {
                            tracing::warn!(
                                path = %path.display(),
                                "non-UTF-8 MCP path was lossy-converted before writing the lockfile"
                            );
                            Utf8PathBuf::from(path.to_string_lossy().into_owned())
                        }),
                    ),
                ),
                Some(McpTransport::Script { command, args }) => (
                    Some(LockfileTransport::Name("script".into())),
                    None,
                    None,
                    None,
                    None,
                    Some(command.clone()),
                    args.clone(),
                    asset.source.url.clone(),
                    asset.source.path.clone(),
                ),
                None => (
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    asset.source.args.clone(),
                    asset.source.url.clone(),
                    asset.source.path.clone(),
                ),
            };

        Self {
            name: asset.name.clone(),
            rev: (!asset.resolved_rev.is_empty()).then(|| asset.resolved_rev.clone()),
            resolved: Some(asset.resolved_date),
            legacy_date: None,
            hash: asset.hash.clone(),
            scope: asset.scope,
            group: asset.source.group.clone(),
            ownership: asset.ownership,
            files: asset
                .files
                .iter()
                .map(|file| {
                    LockfileFileRecord::Entry(LockfileFileEntry {
                        path: file.path.clone(),
                        sha256: file.sha256.clone(),
                        executable: file.executable,
                    })
                })
                .collect(),
            executable: Vec::new(),
            file_hashes: IndexMap::new(),
            git_owner: asset.git.as_ref().map(|git| git.owner.clone()),
            git_repo: asset.git.as_ref().map(|git| git.repo.clone()),
            git_reference: asset.git.as_ref().map(|git| git.reference.clone()),
            git_path: asset.git.as_ref().map(|git| git.path.to_string()),
            git_mode: asset.git.as_ref().map(|git| git.mode),
            sub_assets: asset.sub_assets.clone(),
            license: asset.license.clone(),
            url,
            path,
            transport,
            package,
            entrypoint,
            image,
            bin,
            command,
            env: asset.source.env.clone(),
            args,
            engine: asset.source.engine,
            bin_path: asset.bin_path.clone(),
            compiled_path: asset.compiled_path.clone(),
            plugin_registry: asset.plugin_meta.as_ref().and_then(|m| m.registry.clone()),
            plugin_version: asset
                .plugin_meta
                .as_ref()
                .and_then(|m| m.plugin_version.clone()),
            plugin_source_url: asset
                .plugin_meta
                .as_ref()
                .and_then(|m| m.source_url.clone()),
            plugin_json_hash: asset
                .plugin_meta
                .as_ref()
                .and_then(|m| m.plugin_json_hash.clone()),
        }
    }
}

impl LockfileRecord {
    fn resolved_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.resolved.or(self.legacy_date)
    }

    fn into_resolved_asset(self, kind: AssetKind) -> ResolvedAsset {
        let resolved_date = self.resolved_at().unwrap_or_else(Utc::now);
        let transport =
            match self.transport {
                Some(LockfileTransport::Legacy(transport)) => Some(transport),
                Some(LockfileTransport::Name(name)) => match name.as_str() {
                    "http" => self.url.clone().map(|url| McpTransport::Http { url }),
                    "sse" => self.url.clone().map(|url| McpTransport::Sse { url }),
                    "npx" => self.package.clone().map(|package| McpTransport::Npx {
                        package,
                        entrypoint: self.entrypoint.clone(),
                        args: self.args.clone(),
                    }),
                    "uvx" => self.package.clone().map(|package| McpTransport::Uvx {
                        package,
                        entrypoint: self.entrypoint.clone(),
                        args: self.args.clone(),
                    }),
                    "docker" => self.image.clone().map(|image| McpTransport::Docker {
                        image,
                        args: self.args.clone(),
                    }),
                    "binary" => self.url.clone().zip(self.bin.clone()).map(|(url, bin)| {
                        McpTransport::Binary {
                            url,
                            bin,
                            args: self.args.clone(),
                        }
                    }),
                    "path" => self.path.clone().map(|path| McpTransport::Path {
                        path: path.into_std_path_buf(),
                        args: self.args.clone(),
                    }),
                    "script" => self.command.clone().map(|command| McpTransport::Script {
                        command,
                        args: self.args.clone(),
                    }),
                    _ => None,
                },
                None => None,
            };
        let source = AssetSource {
            url: self.url,
            rev: self.rev.clone(),
            path: self.path,
            group: self.group,
            scope: self.scope,
            transport,
            env: self.env,
            args: self.args,
            engine: self.engine,
        };
        let git = match (
            self.git_owner,
            self.git_repo,
            self.git_reference,
            self.git_path,
            self.git_mode,
        ) {
            (Some(owner), Some(repo), Some(reference), Some(path), Some(mode)) => {
                Some(GitMetadata {
                    owner,
                    repo,
                    reference,
                    path: Utf8PathBuf::from(path),
                    mode,
                })
            }
            _ => None,
        };
        let files = materialize_locked_files(&self.files, &self.file_hashes, &self.executable);
        let executable: Vec<_> = files
            .iter()
            .filter(|file| file.executable)
            .map(|file| file.path.clone())
            .collect();
        let file_hashes: IndexMap<_, _> = files
            .iter()
            .filter_map(|file| {
                file.sha256
                    .as_ref()
                    .map(|sha256| (file.path.clone(), format!("sha256:{sha256}")))
            })
            .collect();

        ResolvedAsset {
            name: self.name,
            kind,
            source,
            resolved_rev: self.rev.unwrap_or_default(),
            resolved_date,
            hash: self.hash,
            scope: self.scope,
            ownership: self.ownership,
            files,
            executable,
            file_hashes,
            git,
            sub_assets: self.sub_assets,
            license: self.license,
            bin_path: self.bin_path,
            compiled_path: self.compiled_path,
            plugin_meta: {
                let meta = PluginMeta {
                    registry: self.plugin_registry,
                    plugin_version: self.plugin_version,
                    source_url: self.plugin_source_url,
                    plugin_json_hash: self.plugin_json_hash,
                };
                if meta.is_empty() {
                    None
                } else {
                    Some(meta)
                }
            },
        }
    }
}

impl From<&GlobalClaim> for GlobalClaimRecord {
    fn from(claim: &GlobalClaim) -> Self {
        Self {
            claimed_by: claim.claimed_by.clone(),
            kind: claim.asset.kind,
            asset: LockfileRecord::from(&claim.asset),
        }
    }
}

impl GlobalClaimRecord {
    fn resolved_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.asset.resolved_at()
    }

    fn into_claim(self) -> GlobalClaim {
        GlobalClaim {
            claimed_by: self.claimed_by,
            asset: self.asset.into_resolved_asset(self.kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn apply_manifest_materializes_local_skill_and_populates_lock() {
        let repo = TempDir::new().expect("tempdir");
        let skill_dir = repo.path().join("skills/pdf");
        std::fs::create_dir_all(&skill_dir).expect("mkdir");
        std::fs::write(skill_dir.join("SKILL.md"), "# PDF\n").expect("write skill");
        std::fs::write(skill_dir.join("helper.txt"), "helper").expect("write helper");
        std::fs::write(repo.path().join("LICENSE"), "MIT License\n").expect("write license");

        let mut manifest = Manifest::default();
        manifest.skills.insert(
            "pdf".into(),
            AssetSource {
                url: None,
                rev: None,
                path: Some(Utf8PathBuf::from("skills/pdf")),
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
        );

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let lock = apply_manifest(
            &manifest,
            &client,
            None,
            ApplyOptions {
                repo_root: repo.path(),
                install: true,
                install_group: None,
                install_scope: None,
                settings: &crate::config::EffectiveSettings::default(),
                source_rules: &source_rules,
                existing_lock: None,
                download_progress: None,
            },
        )
        .await
        .expect("apply");

        assert_eq!(lock.skills.len(), 1);
        let file_paths: Vec<_> = lock.skills[0]
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect();
        assert_eq!(
            file_paths,
            vec![
                Utf8PathBuf::from("pdf/SKILL.md"),
                Utf8PathBuf::from("pdf/helper.txt")
            ]
        );
        assert!(repo.path().join(".github/skills/pdf/SKILL.md").exists());
        assert!(repo.path().join(".github/skills/pdf/helper.txt").exists());
        assert!(lock.skills[0].hash.starts_with("sha256:"));
        assert_eq!(
            lock.skills[0]
                .license
                .as_ref()
                .map(|license| license.spdx.as_str()),
            Some("MIT")
        );
    }

    #[tokio::test]
    async fn apply_manifest_detects_plugin_bundle_sub_assets() {
        let repo = TempDir::new().expect("tempdir");
        let plugin_dir = repo.path().join("plugins/partners");
        std::fs::create_dir_all(plugin_dir.join(".github/plugin")).expect("create plugin manifest");
        std::fs::create_dir_all(plugin_dir.join("agents")).expect("create agents");
        std::fs::create_dir_all(plugin_dir.join("skills/prompt-lib/docs")).expect("create skills");
        std::fs::write(
            plugin_dir.join(".github/plugin/plugin.json"),
            r#"{
  "name": "partners",
  "agents": ["./agents"],
  "skills": ["./skills"]
}"#,
        )
        .expect("write plugin manifest");
        std::fs::write(plugin_dir.join("agents/terraform.md"), "# Terraform\n")
            .expect("write agent");
        std::fs::write(
            plugin_dir.join("skills/prompt-lib/SKILL.md"),
            "# Prompt Lib\n",
        )
        .expect("write skill");
        std::fs::write(
            plugin_dir.join("skills/prompt-lib/docs/guide.md"),
            "# Guide\n",
        )
        .expect("write helper");

        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "partners".into(),
            AssetSource {
                url: None,
                rev: None,
                path: Some(Utf8PathBuf::from("plugins/partners")),
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
        );

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let lock = apply_manifest(
            &manifest,
            &client,
            None,
            ApplyOptions {
                repo_root: repo.path(),
                install: false,
                install_group: None,
                install_scope: None,
                settings: &crate::config::EffectiveSettings::default(),
                source_rules: &source_rules,
                existing_lock: None,
                download_progress: None,
            },
        )
        .await
        .expect("apply");

        assert_eq!(lock.plugins.len(), 1);
        assert_eq!(
            lock.plugins[0].sub_assets,
            vec![
                SubAsset {
                    name: "terraform".into(),
                    kind: AssetKind::Agent,
                    path: Utf8PathBuf::from("partners/agents/terraform.md"),
                    ownership: SubAssetOwnership::Parent,
                },
                SubAsset {
                    name: "prompt-lib".into(),
                    kind: AssetKind::Skill,
                    path: Utf8PathBuf::from("partners/skills/prompt-lib"),
                    ownership: SubAssetOwnership::Parent,
                },
            ]
        );
    }

    #[tokio::test]
    async fn apply_manifest_rejects_disallowed_license() {
        let repo = TempDir::new().expect("tempdir");
        let skill_dir = repo.path().join("skills/gpl-skill");
        std::fs::create_dir_all(&skill_dir).expect("mkdir");
        std::fs::write(skill_dir.join("SKILL.md"), "# GPL\n").expect("write skill");
        std::fs::write(
            repo.path().join("LICENSE"),
            "GNU GENERAL PUBLIC LICENSE\nVersion 3, 29 June 2007\n",
        )
        .expect("write license");

        let mut manifest = Manifest::default();
        manifest.skills.insert(
            "gpl-skill".into(),
            AssetSource {
                url: None,
                rev: None,
                path: Some(Utf8PathBuf::from("skills/gpl-skill")),
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
        );

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let settings = crate::config::EffectiveSettings {
            license_policy: cpm_types::LicensePolicy::DenyCopyleft,
            ..crate::config::EffectiveSettings::default()
        };

        let err = apply_manifest(
            &manifest,
            &client,
            None,
            ApplyOptions {
                repo_root: repo.path(),
                install: false,
                install_group: None,
                install_scope: None,
                settings: &settings,
                source_rules: &source_rules,
                existing_lock: None,
                download_progress: None,
            },
        )
        .await
        .expect_err("copyleft license should be rejected");

        assert!(matches!(err, CpmError::LicenseViolation { .. }));
    }

    #[tokio::test]
    async fn apply_manifest_installs_grouped_asset_when_group_requested() {
        let repo = TempDir::new().expect("tempdir");
        let skill_dir = repo.path().join("skills/research-pdf");
        std::fs::create_dir_all(&skill_dir).expect("mkdir");
        std::fs::write(skill_dir.join("SKILL.md"), "# Research PDF\n").expect("write skill");

        let mut manifest = Manifest::default();
        let mut research = ManifestGroup {
            description: Some("Research assets".into()),
            ..ManifestGroup::default()
        };
        research.skills.insert(
            "research-pdf".into(),
            AssetSource {
                url: None,
                rev: None,
                path: Some(Utf8PathBuf::from("skills/research-pdf")),
                group: "research".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
        );
        manifest.groups.insert("research".into(), research);

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let lock = apply_manifest(
            &manifest,
            &client,
            None,
            ApplyOptions {
                repo_root: repo.path(),
                install: true,
                install_group: Some("research"),
                install_scope: None,
                settings: &crate::config::EffectiveSettings::default(),
                source_rules: &source_rules,
                existing_lock: None,
                download_progress: None,
            },
        )
        .await
        .expect("apply");

        assert_eq!(lock.skills.len(), 1);
        assert_eq!(lock.skills[0].source.group, "research");
        assert!(repo
            .path()
            .join(".github/skills/research-pdf/SKILL.md")
            .exists());
    }

    /// Regression: `cpm add <pypi-url> --uvx --scope local` used to call
    /// `apply_manifest` over the full manifest, which would invoke the GitHub
    /// API for every existing plugin even though only the new MCP was being
    /// added.  `add_single_asset` must succeed without hitting any external
    /// API when the new asset itself has no GitHub-backed source.
    #[tokio::test]
    async fn add_single_mcp_does_not_resolve_unrelated_plugin() {
        let repo = TempDir::new().expect("tempdir");

        // Simulate an existing lockfile that already has a GitHub-hosted plugin.
        // Re-resolving it would call the GitHub API and fail in this test
        // environment (no token, no network assumed).
        let mut existing_lockfile = Lockfile::new();
        existing_lockfile.plugins.push(ResolvedAsset {
            name: "edge-ai-tasks".to_owned(),
            kind: AssetKind::Plugin,
            source: AssetSource {
                url: Some(
                    "https://github.com/awesome-copilot/plugins/tree/main/edge-ai-tasks".to_owned(),
                ),
                rev: Some("abc123deadbeef".to_owned()),
                path: None,
                group: "default".to_owned(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "abc123deadbeef".to_owned(),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:dummy".to_owned(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        });

        let mcp_source = AssetSource {
            url: None,
            rev: None,
            path: None,
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: Some(McpTransport::Uvx {
                package: "mcp-zen-of-docs".to_owned(),
                entrypoint: None,
                args: vec![],
            }),
            env: vec![],
            args: vec![],
            engine: None,
        };

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        // Pass `None` as token — if the GitHub API were called for the
        // unrelated plugin this would produce an authentication error.
        let lockfile = add_single_asset(
            AssetKind::Mcp,
            "mcp-zen-of-docs",
            &mcp_source,
            &client,
            None,
            ApplyOptions {
                repo_root: repo.path(),
                install: false,
                install_group: None,
                install_scope: None,
                settings: &crate::config::EffectiveSettings::default(),
                source_rules: &source_rules,
                existing_lock: Some(&existing_lockfile),
                download_progress: None,
            },
        )
        .await
        .expect("add_single_asset must not call GitHub API for unrelated plugin");

        // The new MCP must appear.
        assert_eq!(lockfile.mcps.len(), 1);
        assert_eq!(lockfile.mcps[0].name, "mcp-zen-of-docs");
        // The existing plugin must be carried forward unchanged.
        assert_eq!(lockfile.plugins.len(), 1);
        assert_eq!(lockfile.plugins[0].name, "edge-ai-tasks");
        assert_eq!(
            lockfile.plugins[0].resolved_rev, "abc123deadbeef",
            "existing plugin rev must not be altered"
        );
    }

    #[tokio::test]
    async fn add_single_package_mcp_preserves_explicit_rev_pin() {
        let repo = TempDir::new().expect("tempdir");
        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let mcp_source = AssetSource {
            url: None,
            rev: Some("1.2.3".to_owned()),
            path: None,
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: Some(McpTransport::Uvx {
                package: "mcp-zen-of-docs".to_owned(),
                entrypoint: None,
                args: vec![],
            }),
            env: vec![],
            args: vec![],
            engine: None,
        };

        let lockfile = add_single_asset(
            AssetKind::Mcp,
            "mcp-zen-of-docs",
            &mcp_source,
            &client,
            None,
            ApplyOptions {
                repo_root: repo.path(),
                install: false,
                install_group: None,
                install_scope: None,
                settings: &crate::config::EffectiveSettings::default(),
                source_rules: &source_rules,
                existing_lock: None,
                download_progress: None,
            },
        )
        .await
        .expect("package MCP add should preserve explicit rev");

        assert_eq!(lockfile.mcps.len(), 1);
        assert_eq!(lockfile.mcps[0].resolved_rev, "1.2.3");
        assert_eq!(lockfile.mcps[0].source.rev.as_deref(), Some("1.2.3"));
    }

    #[tokio::test]
    async fn add_single_docker_mcp_uses_image_tag_as_resolved_rev() {
        let repo = TempDir::new().expect("tempdir");
        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let mcp_source = AssetSource {
            url: None,
            rev: None,
            path: None,
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: Some(McpTransport::Docker {
                image: "ghcr.io/github/github-mcp-server:1.2.3".to_owned(),
                args: vec![],
            }),
            env: vec![],
            args: vec![],
            engine: None,
        };

        let lockfile = add_single_asset(
            AssetKind::Mcp,
            "github-mcp-server",
            &mcp_source,
            &client,
            None,
            ApplyOptions {
                repo_root: repo.path(),
                install: false,
                install_group: None,
                install_scope: None,
                settings: &crate::config::EffectiveSettings::default(),
                source_rules: &source_rules,
                existing_lock: None,
                download_progress: None,
            },
        )
        .await
        .expect("docker MCP add should preserve image tag");

        assert_eq!(lockfile.mcps.len(), 1);
        assert_eq!(lockfile.mcps[0].resolved_rev, "1.2.3");
    }

    // ── drop_asset_from_lockfile ──────────────────────────────────────────────

    #[test]
    fn drop_asset_removes_only_named_entry() {
        let mut lockfile = Lockfile::new();
        lockfile
            .skills
            .push(make_dummy_asset("alpha", AssetKind::Skill, Scope::Local));
        lockfile
            .skills
            .push(make_dummy_asset("beta", AssetKind::Skill, Scope::Local));

        drop_asset_from_lockfile(&mut lockfile, AssetKind::Skill, "alpha", None);

        assert_eq!(lockfile.skills.len(), 1);
        assert_eq!(lockfile.skills[0].name, "beta");
    }

    #[test]
    fn drop_asset_respects_scope_filter() {
        let mut lockfile = Lockfile::new();
        lockfile
            .skills
            .push(make_dummy_asset("pdf", AssetKind::Skill, Scope::Local));
        lockfile
            .skills
            .push(make_dummy_asset("pdf", AssetKind::Skill, Scope::Global));

        drop_asset_from_lockfile(&mut lockfile, AssetKind::Skill, "pdf", Some(Scope::Local));

        assert_eq!(lockfile.skills.len(), 1);
        assert_eq!(lockfile.skills[0].scope, Scope::Global);
    }

    #[test]
    fn drop_asset_is_noop_when_name_absent() {
        let mut lockfile = Lockfile::new();
        lockfile
            .mcps
            .push(make_dummy_asset("zen", AssetKind::Mcp, Scope::Local));

        drop_asset_from_lockfile(&mut lockfile, AssetKind::Mcp, "missing", None);

        assert_eq!(lockfile.mcps.len(), 1, "no entry should be removed");
    }

    // ── prepare_section short-circuit (via apply_manifest) ───────────────────

    /// When an existing lockfile is supplied and a manifest asset has the same
    /// source as the locked entry, `apply_manifest` with `install=false` must
    /// reuse the existing lock entry without calling any GitHub API.
    ///
    /// The test places a GitHub-backed plugin in the manifest.  Without the
    /// short-circuit, `prepare_section` would call `resolve_pinned_rev` for
    /// that plugin, which would fail here because there is no token and no
    /// network.
    #[tokio::test]
    async fn apply_manifest_short_circuits_unchanged_entries_when_install_false() {
        let repo = TempDir::new().expect("tempdir");

        // Existing plugin in the manifest.
        let github_source = AssetSource {
            url: Some(
                "https://github.com/awesome-copilot/plugins/tree/main/edge-ai-tasks".to_owned(),
            ),
            rev: Some("abc123deadbeef".to_owned()),
            path: None,
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        };
        let mut manifest = Manifest::default();
        manifest
            .plugins
            .insert("edge-ai-tasks".into(), github_source.clone());

        // Pre-existing lock with the same source already resolved.
        let mut existing_lockfile = Lockfile::new();
        existing_lockfile.plugins.push(ResolvedAsset {
            name: "edge-ai-tasks".to_owned(),
            kind: AssetKind::Plugin,
            source: github_source.clone(),
            resolved_rev: "abc123deadbeef".to_owned(),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:dummy".to_owned(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        });

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        // No token – would cause a GitHub API auth error without the short-circuit.
        let lockfile = apply_manifest(
            &manifest,
            &client,
            None,
            ApplyOptions {
                repo_root: repo.path(),
                install: false,
                install_group: None,
                install_scope: None,
                settings: &crate::config::EffectiveSettings::default(),
                source_rules: &source_rules,
                existing_lock: Some(&existing_lockfile),
                download_progress: None,
            },
        )
        .await
        .expect("apply_manifest must not call GitHub API for unchanged entries");

        assert_eq!(lockfile.plugins.len(), 1);
        assert_eq!(lockfile.plugins[0].name, "edge-ai-tasks");
        assert_eq!(lockfile.plugins[0].resolved_rev, "abc123deadbeef");
    }

    /// Unchanged MCP entries are short-circuited even with `install=true` since
    /// MCPs are pure config (no remote file content to re-fetch).
    #[tokio::test]
    async fn apply_manifest_short_circuits_mcp_with_install_true() {
        let repo = TempDir::new().expect("tempdir");

        let mcp_source = AssetSource {
            url: None,
            rev: None,
            path: None,
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: Some(McpTransport::Uvx {
                package: "mcp-zen-of-docs".to_owned(),
                entrypoint: None,
                args: vec![],
            }),
            env: vec![],
            args: vec![],
            engine: None,
        };
        let mut manifest = Manifest::default();
        manifest
            .mcps
            .insert("mcp-zen-of-docs".into(), mcp_source.clone());

        let mut existing_lockfile = Lockfile::new();
        existing_lockfile.mcps.push(ResolvedAsset {
            name: "mcp-zen-of-docs".to_owned(),
            kind: AssetKind::Mcp,
            source: mcp_source.clone(),
            resolved_rev: String::new(),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:dummy".to_owned(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![Utf8PathBuf::from("mcp-zen-of-docs.mcp.json").into()],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        });

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let lockfile = apply_manifest(
            &manifest,
            &client,
            None,
            ApplyOptions {
                repo_root: repo.path(),
                install: true,
                install_group: None,
                install_scope: None,
                settings: &crate::config::EffectiveSettings::default(),
                source_rules: &source_rules,
                existing_lock: Some(&existing_lockfile),
                download_progress: None,
            },
        )
        .await
        .expect("unchanged MCP with install=true should not error");

        assert_eq!(lockfile.mcps.len(), 1);
        assert_eq!(lockfile.mcps[0].name, "mcp-zen-of-docs");
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn make_dummy_asset(name: &str, kind: AssetKind, scope: Scope) -> ResolvedAsset {
        ResolvedAsset {
            name: name.to_owned(),
            kind,
            source: AssetSource {
                url: None,
                rev: None,
                path: None,
                group: "default".to_owned(),
                scope,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: String::new(),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:dummy".to_owned(),
            scope,
            ownership: AssetOwnership::Upstream,
            files: vec![],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        }
    }

    #[test]
    fn manifest_writer_emits_canonical_arch2_shape() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.toml");
        let mut manifest = Manifest {
            package: Some(PackageMetadata {
                name: "demo-project".into(),
                description: Some("Demo".into()),
                version: "0.1.0".into(),
                license: Some("MIT".into()),
                authors: Some(vec!["Example <example@example.com>".into()]),
                repository: Some("https://github.com/example/demo".into()),
                created: Some("2026-03-22".into()),
            }),
            settings: PartialSettings {
                default_scope: Some(Scope::Global),
                ..PartialSettings::default()
            },
            ..Manifest::default()
        };
        manifest.sources.insert(
            "internal".into(),
            SourceRule {
                url: "https://mirror.example.com".into(),
                token_env: Some("CORP_TOKEN".into()),
                replace: Some("https://github.com/acme".into()),
            },
        );
        manifest.plugins.insert(
            "partners".into(),
            AssetSource {
                url: Some(
                    "https://github.com/github/awesome-copilot/tree/main/plugins/partners".into(),
                ),
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
        );
        let mut research = ManifestGroup {
            description: Some("Research assets".into()),
            ..ManifestGroup::default()
        };
        research.skills.insert(
            "arxiv-search".into(),
            AssetSource {
                url: Some("https://github.com/example/arxiv-search".into()),
                rev: None,
                path: None,
                group: "research".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
        );
        manifest.groups.insert("research".into(), research);

        write_manifest(&path, &manifest).expect("write manifest");
        let text = std::fs::read_to_string(&path).expect("read manifest");
        let loaded = load_manifest(&path).expect("load manifest");

        assert!(text.contains("[package]"));
        assert!(text.contains("[sources.internal]"));
        assert!(text.contains("[plugins]"));
        assert!(text.contains(
            "partners = \"https://github.com/github/awesome-copilot/tree/main/plugins/partners\""
        ));
        assert!(text.contains("[groups.research]"));
        assert!(text.contains("[groups.research.skills]"));
        assert!(!text.contains("[plugins.partners]"));
        assert_eq!(
            loaded.package.as_ref().map(|pkg| pkg.name.as_str()),
            Some("demo-project")
        );
        assert!(loaded.sources.contains_key("internal"));
        assert!(loaded.groups.contains_key("research"));
        assert!(loaded.plugins.contains_key("partners"));
    }

    #[test]
    fn load_manifest_accepts_legacy_nested_asset_tables() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.toml");
        std::fs::write(
            &path,
            r#"
[plugins.partners]
url = "https://github.com/github/awesome-copilot/tree/main/plugins/partners"
rev = "1234567890abcdef1234567890abcdef12345678"
group = "default"
scope = "local"
env = []
args = []
"#,
        )
        .expect("write legacy manifest");

        let manifest = load_manifest(&path).expect("load legacy manifest");
        let partners = manifest.plugins.get("partners").expect("partners entry");
        assert_eq!(
            partners.url.as_deref(),
            Some("https://github.com/github/awesome-copilot/tree/main/plugins/partners")
        );
        assert_eq!(
            partners.rev.as_deref(),
            Some("1234567890abcdef1234567890abcdef12345678")
        );
    }

    #[test]
    fn lockfile_roundtrip() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.lock");
        let mut lock = Lockfile::new();
        lock.plugins.push(ResolvedAsset {
            name: "demo".into(),
            kind: AssetKind::Plugin,
            source: AssetSource {
                url: Some("https://example.com/plugin".into()),
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "a".repeat(40),
            resolved_date: Utc::now(),
            hash: "sha256:abc".into(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![LockedFile {
                path: Utf8PathBuf::from("demo/file.txt"),
                sha256: Some("file".into()),
                executable: false,
            }],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: Some(GitMetadata {
                owner: "example".into(),
                repo: "plugin".into(),
                reference: "main".into(),
                path: Utf8PathBuf::from("plugin"),
                mode: GitSourceKind::Tree,
            }),
            sub_assets: vec![
                SubAsset {
                    name: "terraform".into(),
                    kind: AssetKind::Agent,
                    path: Utf8PathBuf::from("demo/agents/terraform.md"),
                    ownership: SubAssetOwnership::Parent,
                },
                SubAsset {
                    name: "prompt-lib".into(),
                    kind: AssetKind::Skill,
                    path: Utf8PathBuf::from("demo/skills/prompt-lib"),
                    ownership: SubAssetOwnership::Standalone,
                },
            ],
            license: Some(cpm_types::LicenseInfo {
                spdx: "MIT".into(),
                url: Some("https://example.com/plugin/LICENSE".into()),
                verified: true,
            }),
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        });

        write_lockfile(&path, &lock).expect("write");
        let text = std::fs::read_to_string(&path).expect("read text");
        let loaded = load_lockfile(&path).expect("load");
        assert_eq!(loaded.plugins.len(), 1);
        assert_eq!(loaded.plugins[0].name, "demo");
        assert_eq!(loaded.generated, lock.generated);
        assert_eq!(
            loaded.plugins[0].files,
            vec![LockedFile {
                path: Utf8PathBuf::from("demo/file.txt"),
                sha256: Some("file".into()),
                executable: false,
            }]
        );
        // Legacy file_hashes are reconstituted from the unified files list.
        assert_eq!(
            loaded.plugins[0].file_hashes,
            IndexMap::from([(
                Utf8PathBuf::from("demo/file.txt"),
                String::from("sha256:file"),
            )])
        );
        assert_eq!(
            loaded.plugins[0].git,
            Some(GitMetadata {
                owner: "example".into(),
                repo: "plugin".into(),
                reference: "main".into(),
                path: Utf8PathBuf::from("plugin"),
                mode: GitSourceKind::Tree,
            })
        );
        assert_eq!(loaded.plugins[0].sub_assets, lock.plugins[0].sub_assets);
        assert!(text.contains("[[plugin]]"));
        // Files are now inline tables, not a separate `[plugin.file_hashes]`.
        assert!(text.contains("files = ["));
        assert!(text.contains(r#"path = "demo/file.txt""#));
        assert!(text.contains(r#"sha256 = "file""#));
        assert!(!text.contains("[plugin.file_hashes]"));
        assert!(text.contains("[[plugin.sub_asset]]"));
        assert!(text.contains("generated = "));
        assert!(text.contains("git_owner = \"example\""));
        assert!(text.contains("git_repo = \"plugin\""));
        assert!(text.contains("git_ref = \"main\""));
        assert!(text.contains("git_path = \"plugin\""));
        assert!(text.contains("git_mode = \"tree\""));
        assert!(text.contains("resolved = "));
        assert!(text.contains("rev = "));
        assert!(text.contains("spdx = \"MIT\""));
        assert!(text.contains("ownership = \"standalone\""));
        assert!(!text.contains("ownership = \"parent\""));
        assert!(!text.contains("date = "));
        assert!(!text.contains("resolved_date"));
        assert!(!text.contains("resolved_rev"));
        assert!(!text.contains(".source]"));
    }

    #[test]
    fn lockfile_writer_emits_canonical_mcp_transport_shape() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.lock");
        let generated = chrono::DateTime::parse_from_rfc3339("2026-03-22T14:05:31Z")
            .expect("timestamp")
            .with_timezone(&Utc);
        let mut lock = Lockfile::with_generated(generated);
        lock.mcps.push(ResolvedAsset {
            name: "github".into(),
            kind: AssetKind::Mcp,
            source: AssetSource {
                url: None,
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Global,
                transport: Some(McpTransport::Npx {
                    package: "@modelcontextprotocol/server-github".into(),
                    entrypoint: None,
                    args: vec!["stdio".into()],
                }),
                env: vec![cpm_types::EnvSpec::from_raw(
                    "GITHUB_TOKEN",
                    "$GITHUB_TOKEN",
                )],
                args: vec![],
                engine: None,
            },
            resolved_rev: "1.4.2".into(),
            resolved_date: generated,
            hash: "sha256:def".into(),
            scope: Scope::Global,
            ownership: AssetOwnership::Upstream,
            files: vec![LockedFile {
                path: Utf8PathBuf::from("github.mcp.json"),
                sha256: Some("def".into()),
                executable: false,
            }],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        });

        write_lockfile(&path, &lock).expect("write");
        let text = std::fs::read_to_string(&path).expect("read text");

        assert!(text.contains("generated = "));
        assert!(text.contains("[[mcp]]"));
        assert!(text.contains("transport = \"npx\""));
        assert!(text.contains("package = \"@modelcontextprotocol/server-github\""));
        assert!(text.contains("resolved = "));
        assert!(!text.contains("transport = { npx"));
    }

    #[test]
    fn global_lockfile_roundtrip() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join(".copilot").join("cpm.lock");
        let generated = chrono::DateTime::parse_from_rfc3339("2026-03-22T14:05:31Z")
            .expect("timestamp")
            .with_timezone(&Utc);
        let mut lock = GlobalLockfile::with_generated(generated);
        lock.claims.push(GlobalClaim::new(
            Utf8PathBuf::from("/tmp/demo-repo"),
            ResolvedAsset {
                name: "demo".into(),
                kind: AssetKind::Plugin,
                source: AssetSource {
                    url: Some("https://example.com/plugin".into()),
                    rev: None,
                    path: None,
                    group: "default".into(),
                    scope: Scope::Global,
                    transport: None,
                    env: vec![],
                    args: vec![],
                    engine: None,
                },
                resolved_rev: "a".repeat(40),
                resolved_date: generated,
                hash: "sha256:abc".into(),
                scope: Scope::Global,
                ownership: AssetOwnership::Upstream,
                files: vec![LockedFile {
                    path: Utf8PathBuf::from("demo/file.txt"),
                    sha256: Some("file".into()),
                    executable: false,
                }],
                executable: vec![],
                file_hashes: IndexMap::new(),
                git: Some(GitMetadata {
                    owner: "example".into(),
                    repo: "plugin".into(),
                    reference: "main".into(),
                    path: Utf8PathBuf::from("plugin"),
                    mode: GitSourceKind::Tree,
                }),
                sub_assets: vec![],
                license: None,
                bin_path: None,
                compiled_path: None,
                plugin_meta: None,
            },
        ));

        write_global_lockfile_to(&path, &lock).expect("write");
        let text = std::fs::read_to_string(&path).expect("read text");
        let loaded = load_global_lockfile_from(&path).expect("load");

        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.generated, generated);
        assert_eq!(loaded.claims.len(), 1);
        assert_eq!(
            loaded.claims[0].claimed_by,
            Utf8PathBuf::from("/tmp/demo-repo")
        );
        assert_eq!(loaded.claims[0].asset.name, "demo");
        assert_eq!(loaded.claims[0].asset.kind, AssetKind::Plugin);
        assert!(text.contains("[[claim]]"));
        assert!(text.contains("claimed_by = \"/tmp/demo-repo\""));
        assert!(text.contains("kind = \"plugin\""));
        assert!(text.contains("generated = "));
        assert!(text.contains("resolved = "));
        assert!(!text.contains(".asset]"));
    }

    #[test]
    fn load_global_lockfile_returns_empty_when_missing() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join(".copilot").join("cpm.lock");

        let loaded = load_global_lockfile_from(&path).expect("load");

        assert_eq!(loaded.version, 1);
        assert!(loaded.claims.is_empty());
    }

    #[test]
    fn extract_git_metadata_prefers_explicit_rev_for_github_sources() {
        let source = AssetSource {
            url: Some("https://github.com/example/skills/tree/main/skills/pdf".into()),
            rev: Some("feature/refactor".into()),
            path: None,
            group: "default".into(),
            scope: Scope::Local,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        };

        assert_eq!(
            extract_git_metadata(&source),
            Some(GitMetadata {
                owner: "example".into(),
                repo: "skills".into(),
                reference: "feature/refactor".into(),
                path: Utf8PathBuf::from("skills/pdf"),
                mode: GitSourceKind::Tree,
            })
        );
    }

    // ── ownership serialization ────────────────────────────────────────────────

    #[test]
    fn lockfile_omits_upstream_ownership_field() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.lock");
        let mut lock = Lockfile::new();
        lock.skills.push(ResolvedAsset {
            name: "upstream-skill".into(),
            kind: AssetKind::Skill,
            source: AssetSource {
                url: None,
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "a".repeat(40),
            resolved_date: Utc::now(),
            hash: "sha256:abc".into(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![Utf8PathBuf::from("upstream-skill/SKILL.md").into()],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        });

        write_lockfile(&path, &lock).expect("write");
        let text = std::fs::read_to_string(&path).expect("read text");

        // The ownership field must be omitted for upstream (the default).
        assert!(
            !text.contains("ownership = \"upstream\""),
            "upstream ownership must not be written to lock: {text}"
        );

        // Roundtrip: the loaded asset should deserialise back as Upstream.
        let loaded = load_lockfile(&path).expect("load");
        assert_eq!(loaded.skills[0].ownership, AssetOwnership::Upstream);
    }

    #[test]
    fn lockfile_roundtrip_user_ownership() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.lock");
        let mut lock = Lockfile::new();
        lock.skills.push(ResolvedAsset {
            name: "user-skill".into(),
            kind: AssetKind::Skill,
            source: AssetSource {
                url: None,
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "a".repeat(40),
            resolved_date: Utc::now(),
            hash: "sha256:abc".into(),
            scope: Scope::Local,
            ownership: AssetOwnership::User,
            files: vec![Utf8PathBuf::from("user-skill/SKILL.md").into()],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        });

        write_lockfile(&path, &lock).expect("write");
        let text = std::fs::read_to_string(&path).expect("read text");

        // The ownership field must be present and set to "user".
        assert!(
            text.contains("ownership = \"user\""),
            "user ownership must be written to lock: {text}"
        );

        // Roundtrip: the loaded asset should deserialise as User.
        let loaded = load_lockfile(&path).expect("load");
        assert_eq!(loaded.skills[0].ownership, AssetOwnership::User);
    }

    // ── install_prepared_asset ownership semantics ─────────────────────────────

    fn make_prepared_skill(
        name: &str,
        content: &[u8],
        ownership: AssetOwnership,
        scope: Scope,
    ) -> PreparedAsset {
        let relative = Utf8PathBuf::from(format!("{name}/SKILL.md"));
        let (hash, files) = hash_materialized_files(
            AssetKind::Skill,
            &[PreparedFile {
                relative_path: relative.clone(),
                bytes: content.to_vec(),
            }],
        );
        let executable = files
            .iter()
            .filter(|file| file.executable)
            .map(|file| file.path.clone())
            .collect();
        let file_hashes = files
            .iter()
            .filter_map(|file| {
                file.sha256
                    .as_ref()
                    .map(|sha256| (file.path.clone(), format!("sha256:{sha256}")))
            })
            .collect();
        PreparedAsset {
            lock: ResolvedAsset {
                name: name.to_owned(),
                kind: AssetKind::Skill,
                source: AssetSource {
                    url: None,
                    rev: None,
                    path: None,
                    group: "default".into(),
                    scope,
                    transport: None,
                    env: vec![],
                    args: vec![],
                    engine: None,
                },
                resolved_rev: "a".repeat(40),
                resolved_date: Utc::now(),
                hash,
                scope,
                ownership,
                files,
                executable,
                file_hashes,
                git: None,
                sub_assets: vec![],
                license: None,
                bin_path: None,
                compiled_path: None,
                plugin_meta: None,
            },
            files: vec![PreparedFile {
                relative_path: relative,
                bytes: content.to_vec(),
            }],
            mcp_entry: None,
        }
    }

    #[test]
    fn install_prepared_skips_user_owned_existing_file() {
        let dir = TempDir::new().expect("tempdir");
        // Pre-write the file so it already exists on disk.
        let dest = dir.path().join(".github/skills/my-skill/SKILL.md");
        std::fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
        std::fs::write(&dest, b"original").expect("pre-write");

        let prepared = make_prepared_skill(
            "my-skill",
            b"replacement",
            AssetOwnership::User,
            Scope::Local,
        );
        install_prepared_asset(&prepared, dir.path(), false).expect("install");

        // The original content must be preserved.
        let on_disk = std::fs::read(&dest).expect("read");
        assert_eq!(
            on_disk, b"original",
            "user-owned file must not be overwritten"
        );
    }

    #[test]
    fn install_prepared_writes_user_owned_file_on_first_install() {
        let dir = TempDir::new().expect("tempdir");
        // The file does NOT exist yet — first install must succeed.
        let dest = dir.path().join(".github/skills/new-skill/SKILL.md");

        let prepared = make_prepared_skill(
            "new-skill",
            b"initial content",
            AssetOwnership::User,
            Scope::Local,
        );
        install_prepared_asset(&prepared, dir.path(), false).expect("install");

        assert!(dest.exists(), "file must be created on first install");
        let on_disk = std::fs::read(&dest).expect("read");
        assert_eq!(on_disk, b"initial content");
    }

    #[test]
    fn install_prepared_overwrites_upstream_owned_existing_file() {
        let dir = TempDir::new().expect("tempdir");
        let dest = dir.path().join(".github/skills/up-skill/SKILL.md");
        std::fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
        std::fs::write(&dest, b"old content").expect("pre-write");

        let prepared = make_prepared_skill(
            "up-skill",
            b"updated content",
            AssetOwnership::Upstream,
            Scope::Local,
        );
        install_prepared_asset(&prepared, dir.path(), false).expect("install");

        let on_disk = std::fs::read(&dest).expect("read");
        assert_eq!(
            on_disk, b"updated content",
            "upstream-owned file must be overwritten"
        );
    }

    // ── carry_forward_ownership ────────────────────────────────────────────────

    fn make_skill_asset(name: &str, ownership: AssetOwnership) -> ResolvedAsset {
        ResolvedAsset {
            name: name.to_owned(),
            kind: AssetKind::Skill,
            source: AssetSource {
                url: None,
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "a".repeat(40),
            resolved_date: Utc::now(),
            hash: "sha256:abc".into(),
            scope: Scope::Local,
            ownership,
            files: vec![Utf8PathBuf::from(format!("{name}/SKILL.md")).into()],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        }
    }

    #[test]
    fn carry_forward_preserves_user_ownership() {
        let mut existing = Lockfile::new();
        existing
            .skills
            .push(make_skill_asset("alpha", AssetOwnership::User));

        let mut new_lock = Lockfile::new();
        // Same skill re-resolved with default (Upstream) ownership.
        new_lock
            .skills
            .push(make_skill_asset("alpha", AssetOwnership::Upstream));

        carry_forward_ownership(&mut new_lock, &existing);

        assert_eq!(
            new_lock.skills[0].ownership,
            AssetOwnership::User,
            "carry_forward must promote upstream → user when existing lock marks it as user"
        );
    }

    #[test]
    fn carry_forward_new_assets_remain_upstream() {
        let mut existing = Lockfile::new();
        existing
            .skills
            .push(make_skill_asset("old-skill", AssetOwnership::User));

        let mut new_lock = Lockfile::new();
        // New skill not present in existing lock at all.
        new_lock
            .skills
            .push(make_skill_asset("brand-new", AssetOwnership::Upstream));

        carry_forward_ownership(&mut new_lock, &existing);

        assert_eq!(
            new_lock.skills[0].ownership,
            AssetOwnership::Upstream,
            "newly-resolved assets not in existing lock must remain upstream"
        );
    }

    #[test]
    fn carry_forward_does_not_promote_generated_to_user() {
        let mut existing = Lockfile::new();
        existing
            .skills
            .push(make_skill_asset("gen-skill", AssetOwnership::Generated));

        let mut new_lock = Lockfile::new();
        new_lock
            .skills
            .push(make_skill_asset("gen-skill", AssetOwnership::Upstream));

        carry_forward_ownership(&mut new_lock, &existing);

        assert_eq!(
            new_lock.skills[0].ownership,
            AssetOwnership::Generated,
            "generated ownership must be carried forward just like user"
        );
    }

    #[test]
    fn carry_forward_only_affects_matching_kind_and_name() {
        let mut existing = Lockfile::new();
        // A plugin named "shared" is user-owned in the existing lock.
        existing.plugins.push(ResolvedAsset {
            name: "shared".into(),
            kind: AssetKind::Plugin,
            source: AssetSource {
                url: None,
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "a".repeat(40),
            resolved_date: Utc::now(),
            hash: "sha256:abc".into(),
            scope: Scope::Local,
            ownership: AssetOwnership::User,
            files: vec![],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        });

        let mut new_lock = Lockfile::new();
        // A *skill* named "shared" (different kind) must not inherit the plugin's ownership.
        new_lock
            .skills
            .push(make_skill_asset("shared", AssetOwnership::Upstream));

        carry_forward_ownership(&mut new_lock, &existing);

        assert_eq!(
            new_lock.skills[0].ownership,
            AssetOwnership::Upstream,
            "ownership lookup must be kind-sensitive"
        );
    }

    #[test]
    fn load_lockfile_accepts_legacy_date_and_transport_object() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.lock");
        std::fs::write(
            &path,
            r#"
version = 1

[[mcp]]
name = "github"
rev = "1.4.2"
date = "2026-03-22T14:05:31Z"
hash = "sha256:def"
scope = "global"
group = "default"
files = ["github.mcp.json"]
transport = { npx = { package = "@modelcontextprotocol/server-github", args = ["stdio"] } }
"#,
        )
        .expect("write");

        let loaded = load_lockfile(&path).expect("load");

        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.mcps.len(), 1);
        assert_eq!(loaded.generated, loaded.mcps[0].resolved_date);
        assert_eq!(loaded.mcps[0].resolved_rev, "1.4.2");
        assert!(matches!(
            loaded.mcps[0].source.transport.as_ref(),
            Some(McpTransport::Npx {
                package,
                entrypoint: None,
                args,
            })
                if package == "@modelcontextprotocol/server-github" && args == &vec!["stdio".to_owned()]
        ));
    }

    // ── WS-3: Plugin lock schema extension ───────────────────────────────────

    /// A `[[plugin]]` entry with all four new `plugin_meta` fields serialises
    /// them into the lockfile and deserialises them back without loss.
    #[test]
    fn plugin_meta_roundtrip_full() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.lock");
        let mut lock = Lockfile::new();
        lock.plugins.push(ResolvedAsset {
            name: "edge-ai-tasks".into(),
            kind: AssetKind::Plugin,
            source: AssetSource {
                url: Some(
                    "https://github.com/github/awesome-copilot/tree/main/plugins/edge-ai-tasks"
                        .into(),
                ),
                rev: Some("c6a75d7e0923ec0a754e5554b1c52ef76f0d75f8".into()),
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "c6a75d7e0923ec0a754e5554b1c52ef76f0d75f8".into(),
            resolved_date: Utc::now(),
            hash: "sha256:deadbeef".into(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![Utf8PathBuf::from("edge-ai-tasks/.github/plugin/plugin.json").into()],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: Some(PluginMeta {
                registry: Some("copilot".into()),
                plugin_version: Some("1.2.3".into()),
                source_url: Some(
                    "https://github.com/github/awesome-copilot/tree/main/plugins/edge-ai-tasks"
                        .into(),
                ),
                plugin_json_hash: Some("sha256:cafebabe".into()),
            }),
        });

        write_lockfile(&path, &lock).expect("write");
        let text = std::fs::read_to_string(&path).expect("read text");
        let loaded = load_lockfile(&path).expect("load");

        // Serialised TOML must contain the four plugin_meta keys.
        assert!(
            text.contains("plugin_registry = \"copilot\""),
            "registry missing: {text}"
        );
        assert!(
            text.contains("plugin_version = \"1.2.3\""),
            "version missing: {text}"
        );
        assert!(
            text.contains("plugin_source_url ="),
            "source_url missing: {text}"
        );
        assert!(
            text.contains("plugin_json_hash = \"sha256:cafebabe\""),
            "plugin_json_hash missing: {text}"
        );

        // Deserialised value round-trips without loss.
        let meta = loaded.plugins[0]
            .plugin_meta
            .as_ref()
            .expect("plugin_meta should be present after roundtrip");
        assert_eq!(meta.registry.as_deref(), Some("copilot"));
        assert_eq!(meta.plugin_version.as_deref(), Some("1.2.3"));
        assert_eq!(
            meta.source_url.as_deref(),
            Some("https://github.com/github/awesome-copilot/tree/main/plugins/edge-ai-tasks")
        );
        assert_eq!(meta.plugin_json_hash.as_deref(), Some("sha256:cafebabe"));
    }

    /// A `[[plugin]]` entry without any `plugin_meta` fields deserialises with
    /// `plugin_meta == None` and does not emit any `plugin_` keys to TOML.
    #[test]
    fn plugin_meta_absent_when_none() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.lock");
        let mut lock = Lockfile::new();
        lock.plugins.push(ResolvedAsset {
            name: "no-meta-plugin".into(),
            kind: AssetKind::Plugin,
            source: AssetSource {
                url: Some("https://example.com/plugin".into()),
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "a".repeat(40),
            resolved_date: Utc::now(),
            hash: "sha256:000".into(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        });

        write_lockfile(&path, &lock).expect("write");
        let text = std::fs::read_to_string(&path).expect("read text");
        let loaded = load_lockfile(&path).expect("load");

        // No plugin_meta keys should appear in the serialised TOML.
        assert!(
            !text.contains("plugin_registry"),
            "unexpected plugin_registry in lockfile: {text}"
        );
        assert!(
            !text.contains("plugin_version"),
            "unexpected plugin_version in lockfile: {text}"
        );
        assert!(
            !text.contains("plugin_source_url"),
            "unexpected plugin_source_url in lockfile: {text}"
        );
        assert!(
            !text.contains("plugin_json_hash"),
            "unexpected plugin_json_hash in lockfile: {text}"
        );

        // Deserialised value must be None.
        assert!(
            loaded.plugins[0].plugin_meta.is_none(),
            "plugin_meta should be None when no keys present"
        );
    }

    /// A `[[plugin]]` entry with only some `plugin_meta` fields (partial fill)
    /// round-trips and the missing fields remain `None`.
    #[test]
    fn plugin_meta_roundtrip_partial_fields() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.lock");
        let mut lock = Lockfile::new();
        lock.plugins.push(ResolvedAsset {
            name: "partial-plugin".into(),
            kind: AssetKind::Plugin,
            source: AssetSource {
                url: None,
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: String::new(),
            resolved_date: Utc::now(),
            hash: "sha256:111".into(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: Some(PluginMeta {
                registry: Some("custom-registry".into()),
                plugin_version: None,
                source_url: None,
                plugin_json_hash: None,
            }),
        });

        write_lockfile(&path, &lock).expect("write");
        let text = std::fs::read_to_string(&path).expect("read text");
        let loaded = load_lockfile(&path).expect("load");

        assert!(
            text.contains("plugin_registry = \"custom-registry\""),
            "registry missing: {text}"
        );
        // Absent fields must NOT be emitted.
        assert!(
            !text.contains("plugin_version"),
            "unexpected plugin_version: {text}"
        );
        assert!(
            !text.contains("plugin_source_url"),
            "unexpected plugin_source_url: {text}"
        );
        assert!(
            !text.contains("plugin_json_hash"),
            "unexpected plugin_json_hash: {text}"
        );

        let meta = loaded.plugins[0]
            .plugin_meta
            .as_ref()
            .expect("plugin_meta should be present");
        assert_eq!(meta.registry.as_deref(), Some("custom-registry"));
        assert!(meta.plugin_version.is_none());
        assert!(meta.source_url.is_none());
        assert!(meta.plugin_json_hash.is_none());
    }

    /// Backward-compatibility: a lockfile written without `plugin_meta` keys
    /// (simulating an older lockfile on disk) must load without errors and
    /// produce `plugin_meta == None`.
    #[test]
    fn plugin_meta_backward_compat_old_lockfile() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.lock");

        // Write a lockfile as if produced by a pre-WS3 cpm binary (no plugin_* keys).
        std::fs::write(
            &path,
            r#"
version = 1
generated = "2026-01-01T00:00:00Z"

[[plugin]]
name = "legacy-plugin"
rev = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
resolved = "2026-01-01T00:00:00Z"
hash = "sha256:legacy"
scope = "local"
group = "default"
files = ["legacy-plugin/plugin.json"]
url = "https://example.com/legacy"
"#,
        )
        .expect("write legacy lockfile");

        let loaded = load_lockfile(&path).expect("load");

        assert_eq!(loaded.plugins.len(), 1);
        assert_eq!(loaded.plugins[0].name, "legacy-plugin");
        assert!(
            loaded.plugins[0].plugin_meta.is_none(),
            "legacy lockfile without plugin_meta keys must deserialise to None"
        );
    }

    /// Non-plugin assets (`[[skill]]`) must never have `plugin_meta` emitted
    /// to TOML even if the field is accidentally set.
    #[test]
    fn plugin_meta_not_emitted_for_non_plugin_assets() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cpm.lock");
        let mut lock = Lockfile::new();
        // Skills, agents, MCPs, hooks, and workflows must never carry plugin_meta.
        lock.skills.push(ResolvedAsset {
            name: "my-skill".into(),
            kind: AssetKind::Skill,
            source: AssetSource {
                url: None,
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: String::new(),
            resolved_date: Utc::now(),
            hash: "sha256:222".into(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            // Intentionally set to confirm the field is None after roundtrip.
            plugin_meta: None,
        });

        write_lockfile(&path, &lock).expect("write");
        let text = std::fs::read_to_string(&path).expect("read text");
        let loaded = load_lockfile(&path).expect("load");

        assert!(
            !text.contains("plugin_registry"),
            "plugin_registry must not appear in skill entry: {text}"
        );
        assert!(loaded.skills[0].plugin_meta.is_none());
    }

    // ── can_reuse_without_refetch ─────────────────────────────────────────────

    #[test]
    fn reuse_always_allowed_when_install_false() {
        // For every kind and any rev string, install=false means the caller
        // only needs the lock record — no file bytes are required.
        for kind in [
            AssetKind::Skill,
            AssetKind::Agent,
            AssetKind::Hook,
            AssetKind::Workflow,
            AssetKind::Mcp,
            AssetKind::Plugin,
        ] {
            assert!(
                can_reuse_without_refetch(kind, "branch-name", false),
                "{kind:?}: reuse should be allowed for install=false regardless of rev"
            );
            assert!(
                can_reuse_without_refetch(kind, "", false),
                "{kind:?}: reuse should be allowed for install=false with empty rev"
            );
        }
    }

    #[test]
    fn reuse_always_allowed_for_mcp_even_when_install_true() {
        // MCP assets carry no remote file content; their config is derived
        // entirely from the transport spec already stored in the lock entry.
        assert!(can_reuse_without_refetch(
            AssetKind::Mcp,
            "branch-ref",
            true
        ));
        assert!(can_reuse_without_refetch(AssetKind::Mcp, "", true));
        assert!(can_reuse_without_refetch(
            AssetKind::Mcp,
            "abc123deadbeefabc123deadbeefabc123deadbeef",
            true
        ));
    }

    #[test]
    fn reuse_allowed_for_full_40_char_sha_with_install_true() {
        let full_sha = "a".repeat(40);
        for kind in [AssetKind::Skill, AssetKind::Agent, AssetKind::Plugin] {
            assert!(
                can_reuse_without_refetch(kind, &full_sha, true),
                "{kind:?}: full 40-char SHA is content-addressable and must allow reuse"
            );
        }
    }

    #[test]
    fn reuse_denied_for_non_sha_rev_with_install_true() {
        // Short hashes, branch names, and tags can't prove immutability.
        let cases = [
            "main",
            "v1.2.3",
            "abc123",                // only 6 chars
            &"a".repeat(39),         // 39 chars — one short
            &("a".repeat(39) + "G"), // 40 chars but non-hex
        ];
        for rev in cases {
            for kind in [AssetKind::Skill, AssetKind::Hook] {
                assert!(
                    !can_reuse_without_refetch(kind, rev, true),
                    "{kind:?}: rev={rev:?} is not a full SHA, reuse must be denied for install=true"
                );
            }
        }
    }

    // ── prepare_section short-circuit: changed source must re-resolve ─────────

    /// If the manifest source changes (different URL / rev pin), the short-
    /// circuit must NOT fire — `apply_manifest` must call `prepare_asset` to
    /// get the new resolution.  We model this with a local-path asset so that
    /// the re-resolve happens without any network access; what we're asserting
    /// is that the *new* lock entry (from the changed source) replaces the old
    /// one rather than the old one being carried forward unchanged.
    #[tokio::test]
    async fn apply_manifest_reprocesses_when_source_path_changes() {
        let repo = TempDir::new().expect("tempdir");

        // Create two distinct skill directories.
        let skill_v1 = repo.path().join("skills/v1");
        let skill_v2 = repo.path().join("skills/v2");
        std::fs::create_dir_all(&skill_v1).expect("mkdir v1");
        std::fs::create_dir_all(&skill_v2).expect("mkdir v2");
        std::fs::write(skill_v1.join("SKILL.md"), "# V1\n").expect("write v1");
        std::fs::write(skill_v2.join("SKILL.md"), "# V2\n").expect("write v2");

        let source_v1 = AssetSource {
            url: None,
            rev: None,
            path: Some(camino::Utf8PathBuf::from_path_buf(skill_v1).expect("utf8")),
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        };
        let source_v2 = AssetSource {
            path: Some(camino::Utf8PathBuf::from_path_buf(skill_v2).expect("utf8")),
            ..source_v1.clone()
        };

        // Existing lock has v1.
        let mut existing_lock = Lockfile::new();
        existing_lock.skills.push(ResolvedAsset {
            name: "my-skill".to_owned(),
            kind: AssetKind::Skill,
            source: source_v1.clone(),
            resolved_rev: "v1-hash".to_owned(),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:v1".to_owned(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![camino::Utf8PathBuf::from("my-skill/SKILL.md").into()],
            executable: vec![],
            file_hashes: IndexMap::new(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        });

        // Manifest now points to v2 — a different path.
        let mut manifest = Manifest::default();
        manifest.skills.insert("my-skill".into(), source_v2.clone());

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let new_lock = apply_manifest(
            &manifest,
            &client,
            None,
            ApplyOptions {
                repo_root: repo.path(),
                install: false,
                install_group: None,
                install_scope: None,
                settings: &crate::config::EffectiveSettings::default(),
                source_rules: &source_rules,
                existing_lock: Some(&existing_lock),
                download_progress: None,
            },
        )
        .await
        .expect("apply_manifest with changed source");

        assert_eq!(new_lock.skills.len(), 1);
        // The resolved_rev for a local path is the file hash, not the old "v1-hash".
        assert_ne!(
            new_lock.skills[0].resolved_rev, "v1-hash",
            "changed source must be re-resolved, not carried forward from old lock"
        );
        // The new lock entry should reference the v2 source path.
        assert_eq!(new_lock.skills[0].source.path, source_v2.path);
    }
}
