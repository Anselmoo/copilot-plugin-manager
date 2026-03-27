//! Asset installer — writes files into the local or global install paths.
//!
//! # Install paths
//!
//! | Kind   | Local                 | Global                        |
//! |--------|-----------------------|-------------------------------|
//! | plugin | `.github/plugins/`    | Copilot-managed delegated install (legacy markers in `~/.copilot/plugins/`, modern bundles in `~/.copilot/installed-plugins/`) |
//! | skill  | `.github/skills/`     | `~/.copilot/skills/`          |
//! | agent  | `.github/agents/`     | `~/.copilot/agents/`          |
//! | mcp    | `.vscode/mcp.json`    | `~/.copilot/mcp-config.json`  |
//! | hook   | `.github/hooks/`      | `~/.copilot/hooks/`           |
//! | workflow | `.github/workflows/`| local-only                    |
//! | instruction | `.github/instructions/` | `~/.copilot/instructions/` |
//!
//! MCP assets are written into an **aggregate** Copilot-readable config rather
//! than per-server loose files.  Local scope writes (or updates) the workspace
//! file `.vscode/mcp.json`; global scope writes (or updates) the user file
//! `~/.copilot/mcp-config.json`.  Both files share the same `{ "servers": {
//! … } }` top-level shape that GitHub Copilot consumes directly.
//!
//! Secret env values (`EnvValue::FromEnv`) are **not** written to disk.
//!
//! All writes are atomic: write to `<target>.tmp` then `rename`.

use std::path::{Path, PathBuf};

use cpm_types::{AssetKind, EnvValue, McpTransport, ResolvedAsset, Scope};
use serde_json::{json, Value};
use tracing::{debug, info};

use crate::fetcher::atomic_write;
use crate::paths::{copilot_home_dir, copilot_state_dir};
use crate::CpmError;

/// Return the base install directory for the given `kind` and `scope`.
///
/// `repo_root` is only used for `Scope::Local`.
pub fn install_dir(kind: AssetKind, scope: Scope, repo_root: &Path) -> PathBuf {
    match scope {
        Scope::Local => {
            let sub = match kind {
                AssetKind::Plugin => "plugins",
                AssetKind::Skill => "skills",
                AssetKind::Agent => "agents",
                AssetKind::Mcp => "mcp",
                AssetKind::Hook => "hooks",
                AssetKind::Workflow => "workflows",
                AssetKind::Instruction => "instructions",
            };
            repo_root.join(".github").join(sub)
        }
        Scope::Global => {
            let sub = match kind {
                AssetKind::Plugin => "plugins",
                AssetKind::Skill => "skills",
                AssetKind::Agent => "agents",
                AssetKind::Mcp => "mcp",
                AssetKind::Hook => "hooks",
                AssetKind::Workflow => "workflows",
                AssetKind::Instruction => "instructions",
            };
            copilot_state_dir().join(sub)
        }
    }
}

/// Return the path to the aggregate Copilot-readable MCP config file.
///
/// - **Local** scope → `{repo_root}/.vscode/mcp.json`
/// - **Global** scope → `~/.copilot/mcp-config.json`
///
/// Both files share a top-level server map. Local files typically use
/// `{ "servers": { … } }`, while global Copilot config may use
/// `{ "mcpServers": { … } }`.
pub fn copilot_mcp_config_path(scope: Scope, repo_root: &Path) -> PathBuf {
    match scope {
        Scope::Local => repo_root.join(".vscode").join("mcp.json"),
        Scope::Global => copilot_state_dir().join("mcp-config.json"),
    }
}

/// Build the Copilot server entry for one MCP asset (the value inside
/// `servers`).
///
/// Maps cpm transport variants to the Copilot server config shape:
///
/// | cpm transport | Copilot type | notes |
/// |---|---|---|
/// | `http` | `http` | remote URL |
/// | `sse`  | `sse`  | remote URL |
/// | `npx`  | `stdio` | `command = "npx"`, `args = ["-y", package, …]` |
/// | `uvx`  | `stdio` | `command = "uvx"`, `args = [package, …]` |
/// | `docker` | `stdio` | `command = "docker"`, `args = ["run","--rm","-i",image,…]` |
/// | `binary` | `stdio` | `command = bin_path` (requires resolved `bin_path`) |
/// | `path`   | `stdio` | `command = path` |
/// | `script` (no extra args) | `stdio` | `command = "sh"`, `args = ["-c", cmd]` |
/// | `script` (with extra args) | `stdio` | `command = cmd`, `args = args` |
///
/// `EnvValue::FromEnv` variables are written as `${env:VAR_NAME}` references so
/// the Copilot/VS Code runtime expands them at launch time.
///
/// Returns `None` when the asset has no transport (non-MCP assets) or when the
/// transport cannot produce a valid runtime config (e.g. a `binary` transport
/// whose binary has not yet been downloaded).
pub fn copilot_server_entry(asset: &ResolvedAsset) -> Option<Value> {
    let transport = asset.source.transport.as_ref()?;

    // Build env map: literals are written verbatim; FromEnv references are
    // serialized as "${env:VAR_NAME}" for VS Code / Copilot runtime expansion.
    let env_obj: serde_json::Map<String, Value> = asset
        .source
        .env
        .iter()
        .map(|spec| {
            let v = match &spec.value {
                EnvValue::Literal(v) => v.clone(),
                EnvValue::FromEnv(var) => format!("${{env:{var}}}"),
            };
            (spec.key.clone(), Value::String(v))
        })
        .collect();

    let entry = match transport {
        McpTransport::Http { url } => {
            if env_obj.is_empty() {
                json!({ "type": "http", "url": url })
            } else {
                json!({ "type": "http", "url": url, "env": env_obj })
            }
        }
        McpTransport::Sse { url } => {
            if env_obj.is_empty() {
                json!({ "type": "sse", "url": url })
            } else {
                json!({ "type": "sse", "url": url, "env": env_obj })
            }
        }
        McpTransport::Npx {
            package,
            entrypoint,
            args,
        } => {
            let mut full_args: Vec<Value> = vec![Value::String("-y".into())];
            if let Some(entrypoint) = entrypoint {
                full_args.push(Value::String("--package".into()));
                full_args.push(Value::String(package.clone()));
                full_args.push(Value::String(entrypoint.clone()));
            } else {
                full_args.push(Value::String(package.clone()));
            }
            full_args.extend(args.iter().map(|a| Value::String(a.clone())));
            json!({
                "type": "stdio",
                "command": "npx",
                "args": full_args,
                "env": env_obj,
            })
        }
        McpTransport::Uvx {
            package,
            entrypoint,
            args,
        } => {
            let mut full_args: Vec<Value> = if let Some(entrypoint) = entrypoint {
                vec![
                    Value::String("--from".into()),
                    Value::String(package.clone()),
                    Value::String(entrypoint.clone()),
                ]
            } else {
                vec![Value::String(package.clone())]
            };
            full_args.extend(args.iter().map(|a| Value::String(a.clone())));
            json!({
                "type": "stdio",
                "command": "uvx",
                "args": full_args,
                "env": env_obj,
            })
        }
        McpTransport::Docker { image, args } => {
            let mut full_args: Vec<Value> = vec![
                Value::String("run".into()),
                Value::String("--rm".into()),
                Value::String("-i".into()),
                Value::String(image.clone()),
            ];
            full_args.extend(args.iter().map(|a| Value::String(a.clone())));
            json!({
                "type": "stdio",
                "command": "docker",
                "args": full_args,
                "env": env_obj,
            })
        }
        McpTransport::Binary { args, .. } => {
            // Binary MCP transports require a resolved bin_path set by a prior
            // download/materialization step.  Without it, any config we'd emit
            // would point at a nonexistent executable, which is worse than no
            // config at all.  Return None so callers can error cleanly.
            let cmd = asset.bin_path.as_deref()?;
            json!({
                "type": "stdio",
                "command": cmd,
                "args": args,
                "env": env_obj,
            })
        }
        McpTransport::Path { path, args } => {
            let cmd = path.to_string_lossy().into_owned();
            json!({
                "type": "stdio",
                "command": cmd,
                "args": args,
                "env": env_obj,
            })
        }
        McpTransport::Script { command, args } => {
            if args.is_empty() {
                // No extra args: wrap command string in sh for shell evaluation.
                json!({
                    "type": "stdio",
                    "command": "sh",
                    "args": ["-c", command],
                    "env": env_obj,
                })
            } else {
                // Explicit args: execute the command directly.
                json!({
                    "type": "stdio",
                    "command": command,
                    "args": args,
                    "env": env_obj,
                })
            }
        }
    };

    Some(entry)
}

/// Build the `.mcp.json` payload for a given resolved MCP asset.
///
/// `EnvValue::FromEnv` variables are written as `${env:VAR_NAME}` references.
///
/// Returns `None` when the asset has no transport (non-MCP assets).
pub fn mcp_json(asset: &ResolvedAsset) -> Option<Value> {
    let transport = asset.source.transport.as_ref()?;

    // Build env map: literals are written verbatim; FromEnv references are
    // serialized as "${env:VAR_NAME}" for VS Code / Copilot runtime expansion.
    let env_obj: serde_json::Map<String, Value> = asset
        .source
        .env
        .iter()
        .map(|spec| {
            let v = match &spec.value {
                EnvValue::Literal(v) => v.clone(),
                EnvValue::FromEnv(var) => format!("${{env:{var}}}"),
            };
            (spec.key.clone(), Value::String(v))
        })
        .collect();

    let json = match transport {
        McpTransport::Http { url } => json!({
            "type": "http",
            "url": url,
            "env": env_obj,
        }),
        McpTransport::Sse { url } => json!({
            "type": "sse",
            "url": url,
            "env": env_obj,
        }),
        McpTransport::Npx {
            package,
            entrypoint,
            args,
        } => json!({
            "type": "npx",
            "package": package,
            "entrypoint": entrypoint,
            "args": args,
            "env": env_obj,
        }),
        McpTransport::Uvx {
            package,
            entrypoint,
            args,
        } => json!({
            "type": "uvx",
            "package": package,
            "entrypoint": entrypoint,
            "args": args,
            "env": env_obj,
        }),
        McpTransport::Docker { image, args } => json!({
            "type": "docker",
            "image": image,
            "args": args,
            "env": env_obj,
        }),
        McpTransport::Binary { url, bin, args } => json!({
            "type": "binary",
            "url": url,
            "bin": bin,
            "args": args,
            "bin_path": asset.bin_path.as_ref().map(|p| p.as_str()),
            "env": env_obj,
        }),
        McpTransport::Path { path, args } => json!({
            "type": "path",
            "path": path,
            "args": args,
            "env": env_obj,
        }),
        McpTransport::Script { command, args } => json!({
            "type": "script",
            "command": command,
            "args": args,
            "env": env_obj,
        }),
    };

    Some(json)
}

/// Install a single asset at the given `install_root`.
///
/// For MCP assets the aggregate Copilot config file is updated:
/// - **local** → `.vscode/mcp.json`
/// - **global** → `~/.copilot/mcp-config.json`
///
/// All writes are atomic.
///
/// # Errors
/// Returns [`CpmError::Io`] or [`CpmError::Json`] on failure.
pub fn install_asset(asset: &ResolvedAsset, repo_root: &Path) -> Result<(), CpmError> {
    if asset.kind == AssetKind::Mcp {
        upsert_mcp_server(asset, repo_root)?;
        debug!("installed mcp '{}' into Copilot config", asset.name);
        return Ok(());
    }

    let dir = install_dir(asset.kind, asset.scope, repo_root);
    std::fs::create_dir_all(&dir)?;

    debug!("installed asset '{}' to {}", asset.name, dir.display());
    Ok(())
}

/// Write a pre-computed Copilot server entry directly into the aggregate config.
///
/// Called by the project materialization path when the entry has already been
/// computed using the runtime-rewritten transport (after source-rule URL
/// substitutions).
///
/// # Errors
/// Returns [`CpmError::Io`] or [`CpmError::Json`] on failure.
pub fn write_mcp_server_entry(
    name: &str,
    entry: serde_json::Value,
    scope: Scope,
    repo_root: &Path,
) -> Result<(), CpmError> {
    let config_path = copilot_mcp_config_path(scope, repo_root);
    write_mcp_server_entry_at_path(name, entry, &config_path, preferred_mcp_servers_key(scope))
}

/// Remove a legacy per-server `.mcp.json` file left by older cpm versions.
///
/// Older cpm wrote per-server files to `.github/mcp/{name}.mcp.json` (local)
/// or `~/.copilot/mcp/{name}.mcp.json` (global).  This helper silently removes
/// that file when found so that stale files do not confuse Copilot or fill up
/// directories.
pub fn remove_legacy_mcp_file(name: &str, scope: Scope, repo_root: &Path) -> Result<(), CpmError> {
    let legacy_dir = install_dir(AssetKind::Mcp, scope, repo_root);
    let legacy_file = legacy_dir.join(format!("{name}.mcp.json"));
    if legacy_file.exists() {
        std::fs::remove_file(&legacy_file)?;
        prune_empty_dirs(legacy_file.parent(), prune_stop(scope, repo_root).as_path())?;
        info!("removed legacy MCP file {}", legacy_file.display());
    }
    Ok(())
}

/// Remove installed files for an asset.
///
/// For MCP assets the server entry is removed from the aggregate Copilot
/// config file.  The config file itself is **not** deleted even when all
/// entries are gone; it is left as `{ "servers": {} }`.
///
/// # Errors
/// Returns [`CpmError::Io`] or [`CpmError::Json`] if any operation fails.
pub fn remove_asset(asset: &ResolvedAsset, repo_root: &Path) -> Result<(), CpmError> {
    if asset.kind == AssetKind::Mcp {
        remove_mcp_server(asset, repo_root)?;
        return Ok(());
    }

    let dir = install_dir(asset.kind, asset.scope, repo_root);

    for rel_path in &asset.files {
        let full = dir.join(rel_path.path.as_std_path());
        if full.exists() {
            std::fs::remove_file(&full)?;
            info!("removed {}", full.display());
            prune_empty_dirs(full.parent(), prune_stop(asset.scope, repo_root).as_path())?;
        }
    }
    Ok(())
}

// ── Private MCP aggregate-config helpers ─────────────────────────────────────

/// Read the aggregate Copilot MCP config at `path`.
///
/// Returns `{ "servers": {} }` if the file does not exist.  Returns the
/// on-disk JSON if the file exists, even if `servers` is absent (we add it
/// lazily on write).
fn read_copilot_mcp_config(
    path: &Path,
    preferred_key: &'static str,
) -> Result<(Value, &'static str), CpmError> {
    if !path.exists() {
        return Ok((json!({ preferred_key: {} }), preferred_key));
    }
    let bytes = std::fs::read(path)?;
    let v: Value = serde_json::from_slice(&bytes)?;
    // If the top-level is not an object (corrupted), start fresh.
    if !v.is_object() {
        return Ok((json!({ preferred_key: {} }), preferred_key));
    }
    let key = if v.get("mcpServers").is_some() {
        "mcpServers"
    } else if v.get("servers").is_some() {
        "servers"
    } else {
        preferred_key
    };
    Ok((v, key))
}

/// Insert or update one server entry in the aggregate Copilot MCP config.
fn upsert_mcp_server(asset: &ResolvedAsset, repo_root: &Path) -> Result<(), CpmError> {
    let Some(entry) = copilot_server_entry(asset) else {
        // No transport — nothing to write.
        return Ok(());
    };

    let config_path = copilot_mcp_config_path(asset.scope, repo_root);
    write_mcp_server_entry_at_path(
        &asset.name,
        entry,
        &config_path,
        preferred_mcp_servers_key(asset.scope),
    )
}

fn write_mcp_server_entry_at_path(
    name: &str,
    entry: serde_json::Value,
    config_path: &Path,
    preferred_key: &'static str,
) -> Result<(), CpmError> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let (mut config, servers_key) = read_copilot_mcp_config(config_path, preferred_key)?;

    // config is guaranteed to be an Object at this point.
    let top = config.as_object_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "MCP config root is not a JSON object",
        )
    })?;
    let servers_val = top
        .entry(servers_key.to_owned())
        .or_insert_with(|| json!({}));
    let servers = servers_val.as_object_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("`{servers_key}` field is not a JSON object"),
        )
    })?;
    servers.insert(name.to_owned(), entry);

    let bytes = serde_json::to_vec_pretty(&config)?;
    atomic_write(config_path, &bytes)?;
    info!("updated {} (upserted '{}')", config_path.display(), name);
    Ok(())
}

/// Remove one server entry from the aggregate Copilot MCP config.
///
/// Also removes any legacy per-server `.mcp.json` file written by older cpm
/// versions.  If the aggregate config file does not exist nothing is done.
/// The file is retained even when `servers` becomes empty.
fn remove_mcp_server(asset: &ResolvedAsset, repo_root: &Path) -> Result<(), CpmError> {
    // Clean up any legacy per-server file left by older cpm.
    remove_legacy_mcp_file(&asset.name, asset.scope, repo_root)?;

    remove_copilot_mcp_server_by_name(&asset.name, asset.scope, repo_root)
}

/// Remove a single MCP server entry by name from the aggregate Copilot config.
///
/// This is used when cpm needs to clean up an unmanaged MCP entry that is
/// represented only by its config key rather than a full [`ResolvedAsset`].
pub fn remove_copilot_mcp_server_by_name(
    name: &str,
    scope: Scope,
    repo_root: &Path,
) -> Result<(), CpmError> {
    let config_path = copilot_mcp_config_path(scope, repo_root);
    remove_mcp_server_by_name_at_path(name, &config_path, preferred_mcp_servers_key(scope))
}

fn remove_mcp_server_by_name_at_path(
    name: &str,
    config_path: &Path,
    preferred_key: &'static str,
) -> Result<(), CpmError> {
    if !config_path.exists() {
        return Ok(());
    }

    let (mut config, servers_key) = read_copilot_mcp_config(config_path, preferred_key)?;

    if let Some(servers) = config
        .as_object_mut()
        .and_then(|o| o.get_mut(servers_key))
        .and_then(|s| s.as_object_mut())
    {
        servers.remove(name);
    }

    let bytes = serde_json::to_vec_pretty(&config)?;
    atomic_write(config_path, &bytes)?;
    info!("updated {} (removed '{}')", config_path.display(), name);
    Ok(())
}

/// Read the configured MCP server names from the aggregate Copilot config.
///
/// Supports both local `"servers"` and global `"mcpServers"` compatibility.
pub fn read_copilot_mcp_server_names(
    scope: Scope,
    repo_root: &Path,
) -> Result<Vec<String>, CpmError> {
    let config_path = copilot_mcp_config_path(scope, repo_root);
    read_copilot_mcp_server_names_from(&config_path, preferred_mcp_servers_key(scope))
}

fn read_copilot_mcp_server_names_from(
    path: &Path,
    preferred_key: &'static str,
) -> Result<Vec<String>, CpmError> {
    let (config, servers_key) = read_copilot_mcp_config(path, preferred_key)?;
    let mut names = config
        .as_object()
        .and_then(|o| o.get(servers_key))
        .and_then(|s| s.as_object())
        .map(|servers| servers.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    names.sort();
    Ok(names)
}

fn preferred_mcp_servers_key(scope: Scope) -> &'static str {
    match scope {
        Scope::Local => "servers",
        Scope::Global => "mcpServers",
    }
}

fn prune_stop(scope: Scope, repo_root: &Path) -> PathBuf {
    match scope {
        Scope::Local => repo_root.to_path_buf(),
        Scope::Global => copilot_home_dir().unwrap_or_else(std::env::temp_dir),
    }
}

fn prune_empty_dirs(current: Option<&Path>, stop_at: &Path) -> Result<(), CpmError> {
    let Some(current) = current else {
        return Ok(());
    };
    if current == stop_at || !current.starts_with(stop_at) {
        return Ok(());
    }
    if !current.exists() {
        return prune_empty_dirs(current.parent(), stop_at);
    }
    if current.read_dir()?.next().is_none() {
        std::fs::remove_dir(current)?;
        prune_empty_dirs(current.parent(), stop_at)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::join_portable_path;
    use cpm_types::{AssetOwnership, AssetSource, EnvSpec};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_mcp_asset(name: &str, transport: McpTransport, env: Vec<EnvSpec>) -> ResolvedAsset {
        ResolvedAsset {
            name: name.to_owned(),
            kind: AssetKind::Mcp,
            source: AssetSource {
                url: None,
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: Some(transport),
                env,
                args: vec![],
                engine: None,
            },
            resolved_rev: "a".repeat(40),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:abc".into(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![],
            executable: vec![],
            file_hashes: Default::default(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        }
    }

    #[test]
    fn install_dir_local() {
        let root = PathBuf::from("/repo");
        assert_eq!(
            install_dir(AssetKind::Plugin, Scope::Local, &root),
            PathBuf::from("/repo/.github/plugins")
        );
        assert_eq!(
            install_dir(AssetKind::Hook, Scope::Local, &root),
            PathBuf::from("/repo/.github/hooks")
        );
        assert_eq!(
            install_dir(AssetKind::Workflow, Scope::Local, &root),
            PathBuf::from("/repo/.github/workflows")
        );
    }

    #[test]
    fn copilot_mcp_config_path_local() {
        let root = PathBuf::from("/repo");
        assert_eq!(
            copilot_mcp_config_path(Scope::Local, &root),
            PathBuf::from("/repo/.vscode/mcp.json")
        );
    }

    #[test]
    fn mcp_json_includes_from_env_as_substitution() {
        let env = vec![
            EnvSpec::from_raw("KEY", "literal_value"),
            EnvSpec::from_raw("GITHUB_TOKEN", "$GITHUB_TOKEN"),
        ];
        let asset = make_mcp_asset(
            "test",
            McpTransport::Npx {
                package: "@org/pkg".into(),
                entrypoint: None,
                args: vec![],
            },
            env,
        );
        let json = mcp_json(&asset).expect("json");
        assert_eq!(json["type"], "npx");
        let env_obj = json.get("env").expect("env key");
        assert_eq!(
            env_obj.get("KEY").and_then(|v| v.as_str()),
            Some("literal_value"),
            "literal key should be present verbatim"
        );
        assert_eq!(
            env_obj.get("GITHUB_TOKEN").and_then(|v| v.as_str()),
            Some("${env:GITHUB_TOKEN}"),
            "FromEnv must be written as ${{env:VAR}} substitution"
        );
    }

    #[test]
    fn copilot_server_entry_http_no_env() {
        let asset = make_mcp_asset(
            "my-http",
            McpTransport::Http {
                url: "https://example.com/mcp".into(),
            },
            vec![],
        );
        let entry = copilot_server_entry(&asset).expect("entry");
        assert_eq!(entry["type"], "http");
        assert_eq!(entry["url"], "https://example.com/mcp");
        // No env key when env is empty for http/sse.
        assert!(entry.get("env").is_none());
    }

    #[test]
    fn copilot_server_entry_npx_expands_args() {
        let asset = make_mcp_asset(
            "my-npx",
            McpTransport::Npx {
                package: "@org/pkg".into(),
                entrypoint: None,
                args: vec!["--flag".into()],
            },
            vec![],
        );
        let entry = copilot_server_entry(&asset).expect("entry");
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["command"], "npx");
        let args = entry["args"].as_array().expect("args");
        assert_eq!(args[0], "-y");
        assert_eq!(args[1], "@org/pkg");
        assert_eq!(args[2], "--flag");
    }

    #[test]
    fn copilot_server_entry_uvx_expands_args() {
        let asset = make_mcp_asset(
            "my-uvx",
            McpTransport::Uvx {
                package: "mcp-server-time".into(),
                entrypoint: None,
                args: vec!["--local-timezone=America/New_York".into()],
            },
            vec![],
        );
        let entry = copilot_server_entry(&asset).expect("entry");
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["command"], "uvx");
        let args = entry["args"].as_array().expect("args");
        assert_eq!(args[0], "mcp-server-time");
        assert_eq!(args[1], "--local-timezone=America/New_York");
    }

    #[test]
    fn copilot_server_entry_uvx_uses_from_for_entrypoint() {
        let asset = make_mcp_asset(
            "my-uvx",
            McpTransport::Uvx {
                package: "mcp-zen-of-docs".into(),
                entrypoint: Some("mcp-zen-of-docs-server".into()),
                args: vec!["--transport=stdio".into()],
            },
            vec![],
        );
        let entry = copilot_server_entry(&asset).expect("entry");
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["command"], "uvx");
        let args = entry["args"].as_array().expect("args");
        assert_eq!(args[0], "--from");
        assert_eq!(args[1], "mcp-zen-of-docs");
        assert_eq!(args[2], "mcp-zen-of-docs-server");
        assert_eq!(args[3], "--transport=stdio");
    }

    #[test]
    fn copilot_server_entry_npx_uses_package_flag_for_entrypoint() {
        let asset = make_mcp_asset(
            "my-npx",
            McpTransport::Npx {
                package: "@org/pkg".into(),
                entrypoint: Some("pkg-server".into()),
                args: vec!["--flag".into()],
            },
            vec![],
        );
        let entry = copilot_server_entry(&asset).expect("entry");
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["command"], "npx");
        let args = entry["args"].as_array().expect("args");
        assert_eq!(args[0], "-y");
        assert_eq!(args[1], "--package");
        assert_eq!(args[2], "@org/pkg");
        assert_eq!(args[3], "pkg-server");
        assert_eq!(args[4], "--flag");
    }

    #[test]
    fn copilot_server_entry_docker_expands_image() {
        let asset = make_mcp_asset(
            "my-docker",
            McpTransport::Docker {
                image: "ghcr.io/org/mcp-server:latest".into(),
                args: vec![],
            },
            vec![],
        );
        let entry = copilot_server_entry(&asset).expect("entry");
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["command"], "docker");
        let args = entry["args"].as_array().expect("args");
        assert_eq!(args[0], "run");
        assert_eq!(args[1], "--rm");
        assert_eq!(args[2], "-i");
        assert_eq!(args[3], "ghcr.io/org/mcp-server:latest");
    }

    #[test]
    fn copilot_server_entry_script_wraps_in_sh_when_no_args() {
        let asset = make_mcp_asset(
            "my-script",
            McpTransport::Script {
                command: "node server.js".into(),
                args: vec![],
            },
            vec![],
        );
        let entry = copilot_server_entry(&asset).expect("entry");
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["command"], "sh");
        let args = entry["args"].as_array().expect("args");
        assert_eq!(args[0], "-c");
        assert_eq!(args[1], "node server.js");
    }

    #[test]
    fn copilot_server_entry_script_with_args_runs_command_directly() {
        let asset = make_mcp_asset(
            "my-script",
            McpTransport::Script {
                command: "python".into(),
                args: vec!["-m".into(), "my_server".into()],
            },
            vec![],
        );
        let entry = copilot_server_entry(&asset).expect("entry");
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["command"], "python");
        let args = entry["args"].as_array().expect("args");
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-m");
        assert_eq!(args[1], "my_server");
    }

    #[test]
    fn copilot_server_entry_binary_without_bin_path_returns_none() {
        let asset = make_mcp_asset(
            "my-binary",
            McpTransport::Binary {
                url: "https://github.com/org/repo/releases/tag/v1.0.0".into(),
                bin: "my-server".into(),
                args: vec![],
            },
            vec![],
        );
        // bin_path is None in make_mcp_asset — must return None, not broken config.
        assert!(
            copilot_server_entry(&asset).is_none(),
            "binary transport without bin_path must not emit a config entry"
        );
    }

    #[test]
    fn copilot_server_entry_serializes_from_env_as_substitution() {
        let env = vec![
            EnvSpec::from_raw("KEY", "literal_value"),
            EnvSpec::from_raw("GITHUB_TOKEN", "$GITHUB_TOKEN"),
        ];
        let asset = make_mcp_asset(
            "test",
            McpTransport::Npx {
                package: "@org/pkg".into(),
                entrypoint: None,
                args: vec![],
            },
            env,
        );
        let entry = copilot_server_entry(&asset).expect("entry");
        let env_obj = entry.get("env").expect("env key");
        assert_eq!(
            env_obj.get("KEY").and_then(|v| v.as_str()),
            Some("literal_value"),
            "literal key should be present verbatim"
        );
        assert_eq!(
            env_obj.get("GITHUB_TOKEN").and_then(|v| v.as_str()),
            Some("${env:GITHUB_TOKEN}"),
            "FromEnv must be written as ${{env:VAR}} substitution reference"
        );
    }

    #[test]
    fn install_mcp_writes_vscode_mcp_json() {
        let dir = TempDir::new().expect("tempdir");
        let asset = make_mcp_asset(
            "my-mcp",
            McpTransport::Http {
                url: "https://api.example.com/mcp".into(),
            },
            vec![],
        );
        install_asset(&asset, dir.path()).expect("install");
        let config_path = join_portable_path(dir.path(), ".vscode/mcp.json");
        assert!(config_path.exists(), ".vscode/mcp.json should be created");
        let raw = std::fs::read_to_string(&config_path).expect("read");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        assert_eq!(v["servers"]["my-mcp"]["type"], "http");
        assert_eq!(v["servers"]["my-mcp"]["url"], "https://api.example.com/mcp");
    }

    #[test]
    fn install_mcp_upserts_multiple_servers() {
        let dir = TempDir::new().expect("tempdir");
        let asset_a = make_mcp_asset(
            "server-a",
            McpTransport::Http {
                url: "https://a.example.com/mcp".into(),
            },
            vec![],
        );
        let asset_b = make_mcp_asset(
            "server-b",
            McpTransport::Npx {
                package: "@org/b".into(),
                entrypoint: None,
                args: vec![],
            },
            vec![],
        );
        install_asset(&asset_a, dir.path()).expect("install a");
        install_asset(&asset_b, dir.path()).expect("install b");

        let config_path = join_portable_path(dir.path(), ".vscode/mcp.json");
        let raw = std::fs::read_to_string(&config_path).expect("read");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        assert_eq!(v["servers"]["server-a"]["type"], "http");
        assert_eq!(v["servers"]["server-b"]["type"], "stdio");
        assert_eq!(v["servers"]["server-b"]["command"], "npx");
    }

    #[test]
    fn remove_mcp_removes_entry_not_file() {
        let dir = TempDir::new().expect("tempdir");
        let asset_a = make_mcp_asset(
            "server-a",
            McpTransport::Http {
                url: "https://a.example.com/mcp".into(),
            },
            vec![],
        );
        let asset_b = make_mcp_asset(
            "server-b",
            McpTransport::Http {
                url: "https://b.example.com/mcp".into(),
            },
            vec![],
        );
        install_asset(&asset_a, dir.path()).expect("install a");
        install_asset(&asset_b, dir.path()).expect("install b");
        remove_asset(&asset_a, dir.path()).expect("remove a");

        let config_path = join_portable_path(dir.path(), ".vscode/mcp.json");
        assert!(
            config_path.exists(),
            "config file should survive after partial removal"
        );
        let raw = std::fs::read_to_string(&config_path).expect("read");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        assert!(
            v["servers"].get("server-a").is_none(),
            "server-a should be gone"
        );
        assert!(
            v["servers"].get("server-b").is_some(),
            "server-b should remain"
        );
    }

    #[test]
    fn remove_mcp_no_op_when_config_absent() {
        let dir = TempDir::new().expect("tempdir");
        let asset = make_mcp_asset(
            "ghost",
            McpTransport::Http {
                url: "https://example.com/mcp".into(),
            },
            vec![],
        );
        // Removing without ever installing should not error.
        remove_asset(&asset, dir.path()).expect("remove should be a no-op");
    }

    #[test]
    fn write_mcp_server_entry_preserves_mcp_servers_key() {
        let dir = TempDir::new().expect("tempdir");
        let config_path = dir.path().join("mcp-config.json");
        std::fs::write(&config_path, r#"{ "mcpServers": { "existing": { "type": "http", "url": "https://old.example.com" } } }"#)
            .expect("write config");

        write_mcp_server_entry_at_path(
            "added",
            json!({ "type": "http", "url": "https://new.example.com" }),
            &config_path,
            "mcpServers",
        )
        .expect("upsert");

        let raw = std::fs::read_to_string(&config_path).expect("read");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        assert!(
            v.get("servers").is_none(),
            "should not create legacy servers key"
        );
        assert_eq!(v["mcpServers"]["added"]["url"], "https://new.example.com");
        assert_eq!(
            v["mcpServers"]["existing"]["url"],
            "https://old.example.com"
        );
    }

    #[test]
    fn remove_mcp_server_handles_mcp_servers_key() {
        let dir = TempDir::new().expect("tempdir");
        let config_path = dir.path().join("mcp-config.json");
        std::fs::write(
            &config_path,
            r#"{ "mcpServers": { "ghost": { "type": "http", "url": "https://ghost.example.com" }, "keep": { "type": "http", "url": "https://keep.example.com" } } }"#,
        )
        .expect("write config");

        remove_mcp_server_by_name_at_path("ghost", &config_path, "mcpServers").expect("remove");

        let raw = std::fs::read_to_string(&config_path).expect("read");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        assert!(v["mcpServers"].get("ghost").is_none());
        assert!(v["mcpServers"].get("keep").is_some());
    }

    #[test]
    fn read_mcp_server_names_accepts_mcp_servers_key() {
        let dir = TempDir::new().expect("tempdir");
        let config_path = dir.path().join("mcp-config.json");
        std::fs::write(
            &config_path,
            r#"{ "mcpServers": { "zeta": { "type": "http", "url": "https://z.example.com" }, "alpha": { "type": "http", "url": "https://a.example.com" } } }"#,
        )
        .expect("write config");

        let names =
            read_copilot_mcp_server_names_from(&config_path, "mcpServers").expect("read names");
        assert_eq!(names, vec!["alpha".to_owned(), "zeta".to_owned()]);
    }

    #[test]
    fn remove_asset_prunes_empty_install_directories() {
        let dir = TempDir::new().expect("tempdir");
        let install_path = join_portable_path(dir.path(), ".github/skills/my-skill/SKILL.md");
        std::fs::create_dir_all(install_path.parent().expect("skill dir"))
            .expect("create skill dir");
        std::fs::write(&install_path, "# My Skill\n").expect("write skill");

        let asset = ResolvedAsset {
            name: "my-skill".to_owned(),
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
            resolved_date: chrono::Utc::now(),
            hash: "sha256:abc".into(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![camino::Utf8PathBuf::from("my-skill/SKILL.md").into()],
            executable: vec![],
            file_hashes: Default::default(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        };

        remove_asset(&asset, dir.path()).expect("remove");

        assert!(!join_portable_path(dir.path(), ".github/skills/my-skill").exists());
        assert!(!join_portable_path(dir.path(), ".github/skills").exists());
    }
}
