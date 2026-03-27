//! `cpm add` — add an asset to `cpm.toml`.

use std::path::{Path, PathBuf};

use clap::Args;
use cpm_core::{
    auth,
    config::{build_http_client, load_runtime_config},
    plugin_index::find_installed_plugin_by_name,
    project::{
        add_single_asset, load_global_lockfile, load_lockfile, load_manifest,
        write_global_lockfile, write_lockfile, write_manifest, ApplyOptions,
    },
    resolver::reconcile_global_lockfile,
    source::{
        docker_image_name, docker_image_pin, infer_github_repo_mcp_runner, infer_kind_from_source,
        normalize_asset_source, parse_github_source, resolve_pinned_rev, InferredMcpRunner,
    },
    CpmError,
};
use cpm_types::{AssetKind, AssetSource, EnvSpec, Manifest, McpTransport, Scope};
use reqwest::Url;

use crate::progress::{OperationKind, OperationStatus, ProgressReporter};

use super::{
    build_locked_plugin_asset, derive_plugin_name, plugin_request_is_native, print_plugin_summary,
    report_skipped_plugin, run_plugin_operations, style_success, upsert_plugin_lock_entry,
    PluginAction, PluginOperation,
};

/// Arguments for `cpm add`.
#[derive(Debug, Args)]
pub struct AddArgs {
    /// URL, path, or package name of the asset to add.
    #[arg(required_unless_present = "url")]
    pub source: Option<String>,

    /// Remote MCP endpoint URL.
    #[arg(long, conflicts_with = "source")]
    pub url: Option<String>,

    /// Add as a plugin.
    #[arg(long, group = "kind")]
    pub plugin: bool,

    /// Add as a skill.
    #[arg(long, group = "kind")]
    pub skill: bool,

    /// Add as an agent.
    #[arg(long, group = "kind")]
    pub agent: bool,

    /// Add as an MCP.
    #[arg(long, group = "kind")]
    pub mcp: bool,

    /// Add as a hook bundle.
    #[arg(long, group = "kind")]
    pub hook: bool,

    /// Add as a workflow definition.
    #[arg(long, group = "kind")]
    pub workflow: bool,

    /// Add as an instruction file.
    #[arg(long, group = "kind")]
    pub instruction: bool,

    /// Install scope.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeArg>,

    /// Dependency group.
    #[arg(long, default_value = "default")]
    pub group: String,

    /// Pin to this git ref (tag, branch, or SHA).
    #[arg(long)]
    pub rev: Option<String>,

    /// Use SSE transport (MCP only).
    #[arg(long, group = "transport")]
    pub sse: bool,

    /// Explicit MCP protocol for remote URLs.
    #[arg(long = "type", value_enum, group = "transport")]
    pub transport_type: Option<McpTypeArg>,

    /// Use npx transport (MCP only).
    #[arg(long, group = "transport")]
    pub npx: bool,

    /// Use Docker transport (MCP only).
    #[arg(long, group = "transport")]
    pub docker: bool,

    /// Use uvx transport (MCP only).
    #[arg(long, group = "transport")]
    pub uvx: bool,

    /// Use script transport (MCP only).
    #[arg(long, group = "transport")]
    pub script: bool,

    /// Use GitHub Release binary transport (MCP only).
    #[arg(long, group = "transport")]
    pub release: bool,

    /// Binary filename inside the release archive.
    #[arg(long)]
    pub bin: Option<String>,

    /// Use local path transport (MCP only).
    #[arg(long, group = "transport")]
    pub path: bool,

    /// Extra command argument (repeat for multiple values).
    #[arg(long = "arg")]
    pub command_args: Vec<String>,

    /// Environment variable (`KEY=value` or `KEY=$ENV_VAR`).
    #[arg(long = "env")]
    pub env: Vec<String>,
}

/// Scope argument.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ScopeArg {
    Local,
    Global,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub enum McpTypeArg {
    Http,
    Sse,
}

pub async fn run(args: AddArgs) -> Result<(), CpmError> {
    let kind = infer_kind(&args)?;
    let manifest_path = std::path::Path::new("cpm.toml");
    let lockfile_path = std::path::Path::new("cpm.lock");
    let mut manifest = load_manifest(manifest_path)?;
    let runtime = load_runtime_config(&manifest)?;
    let resolved_scope = resolve_scope(args.scope, runtime.settings.default_scope);
    validate_kind_scope(kind, resolved_scope)?;
    let requested_source = requested_add_source(&args)?;

    if kind == AssetKind::Plugin && !plugin_request_is_native(requested_source) {
        let requested = requested_source.trim().to_owned();
        let name = derive_plugin_name(&requested);
        let asset_source = AssetSource {
            url: Some(requested.clone()),
            rev: None,
            path: None,
            group: args.group.clone(),
            scope: Scope::Global,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        };
        let installed_before = cpm_core::plugin_index::read_installed_plugins()?;
        let mut summary = Default::default();
        if find_installed_plugin_by_name(&installed_before, &name).is_some() {
            report_skipped_plugin(PluginAction::Install, &name);
        } else {
            summary =
                run_plugin_operations(vec![PluginOperation::install(&name, &requested)]).await?;
        }

        let replaced = insert_asset(
            &mut manifest,
            kind,
            name.clone(),
            relativize_asset_source(asset_source.clone(), Path::new(".")),
        )
        .is_some();
        let installed_after = cpm_core::plugin_index::read_installed_plugins()?;
        let installed =
            find_installed_plugin_by_name(&installed_after, &name).ok_or_else(|| {
                CpmError::InvalidSource {
                    input: requested.clone(),
                    reason: format!(
                        "plugin '{name}' did not appear in Copilot discovery after install"
                    ),
                }
            })?;
        let mut lockfile = load_lockfile(lockfile_path).unwrap_or_default();
        upsert_plugin_lock_entry(
            &mut lockfile,
            build_locked_plugin_asset(&name, asset_source, installed)?,
        );

        write_manifest(manifest_path, &manifest)?;
        write_lockfile(lockfile_path, &lockfile)?;
        let global_lockfile = load_global_lockfile()?;
        let reconciled = reconcile_global_lockfile(&lockfile, &global_lockfile, Path::new("."))?;
        if reconciled != global_lockfile {
            write_global_lockfile(&reconciled)?;
        }
        print_plugin_summary(summary);
        let action = if replaced { "Updated" } else { "Added" };
        println!(
            "{} {action} plugin '{name}' in {} and reconciled it with Copilot",
            style_success("✓"),
            manifest_path.display()
        );
        return Ok(());
    }

    let client = build_http_client(
        format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        &runtime.settings,
    )?;
    let token = auth::resolve_token();

    // For bare GitHub repo URLs with --mcp and no explicit transport flag,
    // probe the repo for high-confidence packaging signals before normalization.
    let github_inferred_runner = if kind == AssetKind::Mcp
        && !explicit_mcp_transport_requested(&args)
        && (requested_source.starts_with("https://github.com/")
            || requested_source.starts_with("http://github.com/"))
    {
        infer_github_repo_mcp_runner(
            &client,
            token.as_deref(),
            requested_source,
            &runtime.source_rules,
        )
        .await
    } else {
        None
    };

    let (name, asset_source) = match kind {
        AssetKind::Mcp => normalize_mcp_source(&args, resolved_scope, github_inferred_runner)?,
        _ => {
            let normalized = normalize_asset_source(kind, requested_source)?;
            let rev = resolve_pinned_rev(
                &client,
                token.as_deref(),
                normalized.url.as_deref(),
                args.rev.as_deref(),
                &runtime.source_rules,
            )
            .await?;
            (
                normalized.name,
                AssetSource {
                    url: normalized.url,
                    rev,
                    path: normalized.path,
                    group: args.group.clone(),
                    scope: resolved_scope,
                    transport: None,
                    env: vec![],
                    args: vec![],
                    engine: None,
                },
            )
        }
    };

    let asset_source = relativize_asset_source(asset_source, Path::new("."));
    // Clone before moving into the manifest so we can pass a reference to
    // add_single_asset without re-borrowing from the manifest.
    let replaced = insert_asset(&mut manifest, kind, name.clone(), asset_source.clone()).is_some();
    let existing_lock = load_lockfile(lockfile_path).ok();
    let reporter = ProgressReporter::auto();
    let mut handle = reporter.begin_operation(OperationKind::Install, format!("{kind}:{name}"));
    handle.set_status(OperationStatus::Running);
    let lockfile = add_single_asset(
        kind,
        &name,
        &asset_source,
        &client,
        token.as_deref(),
        ApplyOptions {
            repo_root: std::path::Path::new("."),
            install: true,
            install_group: Some(args.group.as_str()),
            install_scope: Some(resolved_scope),
            settings: &runtime.settings,
            source_rules: &runtime.source_rules,
            existing_lock: existing_lock.as_ref(),
            download_progress: Some(&reporter),
        },
    )
    .await;
    handle.finish(if lockfile.is_ok() {
        OperationStatus::Succeeded
    } else {
        OperationStatus::Failed
    });
    let lockfile = lockfile?;

    write_manifest(manifest_path, &manifest)?;
    write_lockfile(lockfile_path, &lockfile)?;

    let action = if replaced { "Updated" } else { "Added" };
    println!(
        "✓ {action} {kind} '{name}' in {} and materialized it on disk",
        manifest_path.display()
    );
    if let Some(McpTransport::Docker { image, .. }) = asset_source.transport.as_ref() {
        if docker_image_pin(image).is_none() {
            eprintln!(
                "! Docker MCP image '{image}' is floating. Prefer an explicit tag like `ghcr.io/owner/server:1.2.3` or a digest like `ghcr.io/owner/server@sha256:...` for stable locking."
            );
        }
    }
    Ok(())
}

fn validate_kind_scope(kind: AssetKind, scope: Scope) -> Result<(), CpmError> {
    if kind == AssetKind::Workflow && scope == Scope::Global {
        return Err(CpmError::InvalidConfig {
            key: "scope".to_owned(),
            reason: "workflow assets are local-only; use `--scope local` or omit the flag"
                .to_owned(),
        });
    }
    Ok(())
}

fn infer_kind(args: &AddArgs) -> Result<AssetKind, CpmError> {
    if args.plugin {
        return Ok(AssetKind::Plugin);
    }
    if args.skill {
        return Ok(AssetKind::Skill);
    }
    if args.agent {
        return Ok(AssetKind::Agent);
    }
    if args.hook {
        return Ok(AssetKind::Hook);
    }
    if args.workflow {
        return Ok(AssetKind::Workflow);
    }
    if args.instruction {
        return Ok(AssetKind::Instruction);
    }
    if args.mcp
        || args.sse
        || args.transport_type.is_some()
        || args.npx
        || args.docker
        || args.uvx
        || args.script
        || args.release
        || args.path
        || args.url.is_some()
    {
        return Ok(AssetKind::Mcp);
    }

    let source = requested_add_source(args)?;
    infer_kind_from_source(source).ok_or_else(|| CpmError::InvalidSource {
        input: source.to_owned(),
        reason: "could not infer asset kind; pass --plugin, --skill, --agent, or --mcp".to_owned(),
    })
}

/// Extract a runnable package name and optional pinned version from a PyPI
/// project URL, or return the input unchanged when it is already a plain
/// package name or version spec.
///
/// ```text
/// https://pypi.org/project/mcp-zen-of-docs/        → ("mcp-zen-of-docs", None)
/// https://pypi.org/project/mcp-zen-of-docs          → ("mcp-zen-of-docs", None)
/// https://pypi.org/project/mcp-zen-of-docs/1.2.3/  → ("mcp-zen-of-docs", Some("1.2.3"))
/// mcp-zen-of-docs                                   → ("mcp-zen-of-docs", None)
/// ```
fn parse_pypi_package_request(source: &str) -> (String, Option<String>) {
    let stripped = source
        .strip_prefix("https://pypi.org/project/")
        .or_else(|| source.strip_prefix("http://pypi.org/project/"));
    if let Some(rest) = stripped {
        let trimmed = rest.trim_end_matches('/');
        if let Some((package, version)) = trimmed.split_once('/') {
            return (package.to_owned(), Some(version.to_owned()));
        }
        return (trimmed.to_owned(), None);
    }
    (source.to_owned(), None)
}

fn normalize_pypi_package(source: &str) -> String {
    parse_pypi_package_request(source).0
}

/// Extract a runnable package name from an npm registry URL, or return the
/// input unchanged when the input is already a plain package / scoped name.
///
/// ```text
/// https://www.npmjs.com/package/@scope/name  → "@scope/name"
/// https://npmjs.com/package/my-pkg           → "my-pkg"
/// @scope/name                                → "@scope/name"
/// ```
fn normalize_npm_package(source: &str) -> &str {
    let stripped = source
        .strip_prefix("https://www.npmjs.com/package/")
        .or_else(|| source.strip_prefix("http://www.npmjs.com/package/"))
        .or_else(|| source.strip_prefix("https://npmjs.com/package/"))
        .or_else(|| source.strip_prefix("http://npmjs.com/package/"));
    if let Some(rest) = stripped {
        return rest.trim_end_matches('/');
    }
    source
}

fn source_looks_like_local_path(source: &str) -> bool {
    source_looks_like_windows_absolute_path(source)
        || source.starts_with("./")
        || source.starts_with("../")
        || source.starts_with('/')
        || source.starts_with("~/")
        || source.starts_with(".\\")
        || source.starts_with("..\\")
        || (!looks_like_url(source) && !source.starts_with('@') && contains_path_separator(source))
}

fn source_looks_like_windows_absolute_path(source: &str) -> bool {
    let mut chars = source.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(drive), Some(':'), Some(separator))
            if drive.is_ascii_alphabetic() && matches!(separator, '\\' | '/')
    )
}

fn contains_path_separator(source: &str) -> bool {
    source.contains('/') || source.contains('\\')
}

fn looks_like_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn infer_package_runner(source: &str) -> McpTransport {
    let trimmed = source.trim();
    if trimmed.starts_with('@') || trimmed.contains("/package/") {
        return McpTransport::Npx {
            package: normalize_npm_package(trimmed).to_owned(),
            entrypoint: None,
            args: vec![],
        };
    }

    McpTransport::Uvx {
        package: normalize_pypi_package(trimmed),
        entrypoint: None,
        args: vec![],
    }
}

fn normalize_mcp_source(
    args: &AddArgs,
    scope: Scope,
    inferred_runner: Option<InferredMcpRunner>,
) -> Result<(String, AssetSource), CpmError> {
    let source = requested_add_source(args)?;
    let env = parse_env_specs(&args.env)?;
    let (pypi_package, pypi_version) = parse_pypi_package_request(source);
    let docker_pin = docker_image_pin(source);
    let transport = if let Some(protocol) = args.transport_type {
        Some(remote_mcp_transport(source, protocol)?)
    } else if args.npx {
        Some(McpTransport::Npx {
            package: normalize_npm_package(source).to_owned(),
            entrypoint: None,
            args: args.command_args.clone(),
        })
    } else if args.uvx {
        Some(McpTransport::Uvx {
            package: pypi_package.clone(),
            entrypoint: None,
            args: args.command_args.clone(),
        })
    } else if args.docker {
        Some(McpTransport::Docker {
            image: source.to_owned(),
            args: args.command_args.clone(),
        })
    } else if args.sse {
        Some(remote_mcp_transport(source, McpTypeArg::Sse)?)
    } else if args.script {
        Some(McpTransport::Script {
            command: source.to_owned(),
            args: args.command_args.clone(),
        })
    } else if args.release {
        return Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: "--release MCP binary transport is not yet implemented; binary download and \
                 materialization are pending. Use --uvx, --npx, --docker, --script, or --path \
                 instead."
                .to_owned(),
        });
    } else if args.path || source_looks_like_local_path(source) {
        Some(McpTransport::Path {
            path: source.to_owned().into(),
            args: args.command_args.clone(),
        })
    } else if source.starts_with("https://pypi.org/project/")
        || source.starts_with("http://pypi.org/project/")
    {
        Some(McpTransport::Uvx {
            package: pypi_package.clone(),
            entrypoint: None,
            args: args.command_args.clone(),
        })
    } else if source.starts_with("https://www.npmjs.com/package/")
        || source.starts_with("http://www.npmjs.com/package/")
        || source.starts_with("https://npmjs.com/package/")
        || source.starts_with("http://npmjs.com/package/")
        || source.starts_with('@')
    {
        Some(McpTransport::Npx {
            package: normalize_npm_package(source).to_owned(),
            entrypoint: None,
            args: args.command_args.clone(),
        })
    } else if source.starts_with("https://github.com/") || source.starts_with("http://github.com/")
    {
        // Use pre-probed packaging signals when available; otherwise error.
        match inferred_runner {
            Some(InferredMcpRunner::Uvx) => {
                let package = derive_name(source);
                Some(McpTransport::Uvx {
                    package,
                    entrypoint: None,
                    args: args.command_args.clone(),
                })
            }
            Some(InferredMcpRunner::Npx) => {
                let package = derive_name(source);
                Some(McpTransport::Npx {
                    package,
                    entrypoint: None,
                    args: args.command_args.clone(),
                })
            }
            None => {
                return Err(CpmError::InvalidSource {
                    input: source.to_owned(),
                    reason: "cannot infer an MCP runner from this GitHub URL; use --npx, --uvx, \
                         --docker, --script, or --path to choose a transport"
                        .to_owned(),
                });
            }
        }
    } else if source.starts_with("http://") || source.starts_with("https://") {
        Some(remote_mcp_transport(source, McpTypeArg::Http)?)
    } else {
        let inferred = infer_package_runner(source);
        Some(match inferred {
            McpTransport::Npx {
                package,
                entrypoint,
                ..
            } => McpTransport::Npx {
                package,
                entrypoint,
                args: args.command_args.clone(),
            },
            McpTransport::Uvx {
                package,
                entrypoint,
                ..
            } => McpTransport::Uvx {
                package,
                entrypoint,
                args: args.command_args.clone(),
            },
            _ => unreachable!("package runner inference only returns npx or uvx"),
        })
    };
    let name = derive_mcp_name(source, transport.as_ref());

    validate_mcp_rev_usage(args, transport.as_ref())?;

    let path = if args.path {
        Some(source.to_owned().into())
    } else {
        None
    };

    Ok((
        name,
        AssetSource {
            url: if matches!(
                transport,
                Some(McpTransport::Http { .. })
                    | Some(McpTransport::Sse { .. })
                    | Some(McpTransport::Binary { .. })
            ) {
                Some(source.to_owned())
            } else {
                None
            },
            rev: pypi_version.or(docker_pin).or_else(|| args.rev.clone()),
            path,
            group: args.group.clone(),
            scope,
            transport,
            env,
            args: args.command_args.clone(),
            engine: None,
        },
    ))
}

fn derive_mcp_name(source: &str, transport: Option<&McpTransport>) -> String {
    match transport {
        Some(McpTransport::Http { url }) | Some(McpTransport::Sse { url }) => {
            derive_remote_mcp_name(url)
        }
        Some(McpTransport::Npx { package, .. }) => {
            package.rsplit('/').next().unwrap_or(package).to_owned()
        }
        Some(McpTransport::Uvx { package, .. }) => derive_name(package),
        Some(McpTransport::Docker { image, .. }) => docker_image_name(image),
        Some(McpTransport::Binary { url, .. }) => derive_remote_mcp_name(url),
        _ => derive_name(source),
    }
}

fn requested_add_source(args: &AddArgs) -> Result<&str, CpmError> {
    args.url
        .as_deref()
        .or(args.source.as_deref())
        .ok_or_else(|| CpmError::InvalidSource {
            input: String::new(),
            reason: "missing source; pass a positional source or `--url <MCP_URL>`".to_owned(),
        })
}

fn explicit_mcp_transport_requested(args: &AddArgs) -> bool {
    args.transport_type.is_some()
        || args.sse
        || args.npx
        || args.docker
        || args.uvx
        || args.script
        || args.release
        || args.path
}

fn remote_mcp_transport(source: &str, requested: McpTypeArg) -> Result<McpTransport, CpmError> {
    if !looks_like_url(source) {
        return Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: "remote MCP transports require an http(s) URL; pass `--url <URL>` or an https:// source".to_owned(),
        });
    }

    Ok(match canonical_remote_mcp_type(source, requested) {
        McpTypeArg::Http => McpTransport::Http {
            url: source.to_owned(),
        },
        McpTypeArg::Sse => McpTransport::Sse {
            url: source.to_owned(),
        },
    })
}

fn canonical_remote_mcp_type(source: &str, requested: McpTypeArg) -> McpTypeArg {
    let known_http_host = Url::parse(source)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| matches!(host, "mcp.context7.com" | "api.githubcopilot.com"))
        })
        .unwrap_or(false);
    if known_http_host {
        McpTypeArg::Http
    } else {
        requested
    }
}

fn derive_remote_mcp_name(source: &str) -> String {
    let Some(host) = Url::parse(source)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
    else {
        return derive_name(source);
    };

    match host.as_str() {
        "mcp.context7.com" => "context7".to_owned(),
        "api.githubcopilot.com" => "githubcopilot".to_owned(),
        _ => derive_host_identity(&host),
    }
}

fn derive_host_identity(host: &str) -> String {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return sanitize_name_component(host);
    }

    let labels: Vec<_> = host.split('.').filter(|label| !label.is_empty()).collect();
    if labels.is_empty() {
        return derive_name(host);
    }
    if labels.len() == 1 {
        return sanitize_name_component(labels[0]);
    }
    sanitize_name_component(labels[labels.len() - 2])
}

fn sanitize_name_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    sanitized.trim_matches('-').to_owned()
}

fn validate_mcp_rev_usage(
    args: &AddArgs,
    transport: Option<&McpTransport>,
) -> Result<(), CpmError> {
    let Some(rev) = args.rev.as_deref() else {
        return Ok(());
    };

    match transport {
        Some(McpTransport::Binary { url, .. }) => {
            if parse_github_source(url).is_none() {
                return Err(CpmError::InvalidSource {
                    input: rev.to_owned(),
                    reason: "`--rev` is only supported for GitHub-backed MCP release URLs"
                        .to_owned(),
                });
            }
        }
        Some(McpTransport::Http { .. }) | Some(McpTransport::Sse { .. }) => {
            return Err(CpmError::InvalidSource {
                input: rev.to_owned(),
                reason: "`--rev` is not supported for HTTP or SSE MCP endpoints".to_owned(),
            });
        }
        Some(McpTransport::Npx { .. })
        | Some(McpTransport::Uvx { .. })
        | Some(McpTransport::Docker { .. })
        | Some(McpTransport::Path { .. })
        | Some(McpTransport::Script { .. }) => {
            return Err(CpmError::InvalidSource {
                input: rev.to_owned(),
                reason: "`--rev` only applies to Git-backed asset sources, not package or local MCP transports".to_owned(),
            });
        }
        None => {}
    }

    Ok(())
}

fn parse_env_specs(values: &[String]) -> Result<Vec<EnvSpec>, CpmError> {
    values
        .iter()
        .map(|value| {
            let (key, raw_value) =
                value
                    .split_once('=')
                    .ok_or_else(|| CpmError::InvalidSource {
                        input: value.clone(),
                        reason: "expected `KEY=value` or `KEY=$ENV_VAR`".to_owned(),
                    })?;
            if key.is_empty() {
                return Err(CpmError::InvalidSource {
                    input: value.clone(),
                    reason: "environment variable key cannot be empty".to_owned(),
                });
            }
            Ok(EnvSpec::from_raw(key, raw_value))
        })
        .collect()
}

fn derive_name(source: &str) -> String {
    source
        .trim_end_matches('/')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(source)
        .trim_end_matches(".agent.md")
        .to_owned()
}

fn insert_asset(
    manifest: &mut Manifest,
    kind: AssetKind,
    name: String,
    source: AssetSource,
) -> Option<AssetSource> {
    if source.group == "default" {
        return match kind {
            AssetKind::Plugin => manifest.plugins.insert(name, source),
            AssetKind::Skill => manifest.skills.insert(name, source),
            AssetKind::Agent => manifest.agents.insert(name, source),
            AssetKind::Mcp => manifest.mcps.insert(name, source),
            AssetKind::Hook => manifest.hooks.insert(name, source),
            AssetKind::Workflow => manifest.workflows.insert(name, source),
            AssetKind::Instruction => manifest.instructions.insert(name, source),
        };
    }

    let group = manifest.groups.entry(source.group.clone()).or_default();

    match kind {
        AssetKind::Plugin => group.plugins.insert(name, source),
        AssetKind::Skill => group.skills.insert(name, source),
        AssetKind::Agent => group.agents.insert(name, source),
        AssetKind::Mcp => group.mcps.insert(name, source),
        AssetKind::Hook => group.hooks.insert(name, source),
        AssetKind::Workflow => group.workflows.insert(name, source),
        AssetKind::Instruction => group.instructions.insert(name, source),
    }
}

fn resolve_scope(scope: Option<ScopeArg>, default_scope: Scope) -> Scope {
    scope.map(Into::into).unwrap_or(default_scope)
}

fn relativize_asset_source(mut source: AssetSource, repo_root: &Path) -> AssetSource {
    source.path = source.path.take().map(|path| {
        let original = path.clone();
        let relative = relativize_pathbuf(PathBuf::from(path.as_str()), repo_root);
        relative.try_into().unwrap_or(original)
    });

    source.transport = source.transport.take().map(|transport| match transport {
        McpTransport::Path { path, args } => McpTransport::Path {
            path: relativize_pathbuf(path, repo_root),
            args,
        },
        other => other,
    });

    source
}
fn relativize_pathbuf(path: PathBuf, repo_root: &Path) -> PathBuf {
    if !path.is_absolute() {
        return path;
    }

    let repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    match path.strip_prefix(&repo_root) {
        Ok(relative) => relative.to_path_buf(),
        Err(_) => path,
    }
}

impl From<ScopeArg> for Scope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::Local => Scope::Local,
            ScopeArg::Global => Scope::Global,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_skill_kind_from_source_when_flag_missing() {
        let args = AddArgs {
            source: Some("https://github.com/anthropics/skills/tree/main/skills/pdf".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: false,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        assert!(matches!(infer_kind(&args), Ok(AssetKind::Skill)));
    }

    #[test]
    fn writes_manifest_entries_into_expected_kind_table() {
        let mut manifest = Manifest::default();
        let source = AssetSource {
            url: Some("https://github.com/anthropics/skills/blob/main/skills/pdf/SKILL.md".into()),
            rev: None,
            path: None,
            group: "default".into(),
            scope: Scope::Local,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        };

        insert_asset(&mut manifest, AssetKind::Skill, "pdf".into(), source);

        assert!(manifest.skills.contains_key("pdf"));
    }

    #[test]
    fn writes_non_default_group_entries_into_group_table() {
        let mut manifest = Manifest::default();
        let source = AssetSource {
            url: Some(
                "https://github.com/github/awesome-copilot/blob/main/instructions/shell.instructions.md"
                    .into(),
            ),
            rev: None,
            path: None,
            group: "dev".into(),
            scope: Scope::Local,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        };

        insert_asset(
            &mut manifest,
            AssetKind::Instruction,
            "shell".into(),
            source,
        );

        assert!(manifest.instructions.is_empty());
        assert!(manifest.groups.contains_key("dev"));
        assert!(manifest.groups["dev"].instructions.contains_key("shell"));
    }

    #[test]
    fn uvx_transport_does_not_write_bogus_url() {
        let args = AddArgs {
            source: Some("mcp-server-git".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: true,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec!["--repository".to_owned(), ".".to_owned()],
            env: vec![],
        };

        let (_name, source) = normalize_mcp_source(&args, Scope::Local, None).expect("normalize");
        assert!(source.url.is_none());
        assert!(matches!(source.transport, Some(McpTransport::Uvx { .. })));
    }

    #[test]
    fn rejects_rev_for_npx_transport() {
        let args = AddArgs {
            source: Some("@modelcontextprotocol/server-github".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: Some("v1.2.3".to_owned()),
            sse: false,
            transport_type: None,
            npx: true,
            docker: false,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let err =
            normalize_mcp_source(&args, Scope::Local, None).expect_err("rev should be rejected");
        assert!(matches!(err, CpmError::InvalidSource { .. }));
    }

    #[test]
    fn parses_env_specs_from_flags() {
        let env = parse_env_specs(&[
            "LOG_LEVEL=debug".to_owned(),
            "GITHUB_TOKEN=$GITHUB_TOKEN".to_owned(),
        ])
        .expect("env");

        assert_eq!(env.len(), 2);
        assert_eq!(env[0], EnvSpec::from_raw("LOG_LEVEL", "debug"));
        assert_eq!(env[1], EnvSpec::from_raw("GITHUB_TOKEN", "$GITHUB_TOKEN"));
    }

    #[test]
    fn falls_back_to_configured_default_scope_when_flag_missing() {
        assert_eq!(resolve_scope(None, Scope::Global), Scope::Global);
        assert_eq!(
            resolve_scope(Some(ScopeArg::Local), Scope::Global),
            Scope::Local
        );
    }

    #[test]
    fn rejects_global_workflow_scope() {
        let err = validate_kind_scope(AssetKind::Workflow, Scope::Global)
            .expect_err("global workflows should be rejected");
        assert!(matches!(err, CpmError::InvalidConfig { .. }));
    }

    // ── PyPI / npm URL normalisation ──────────────────────────────────────────

    #[test]
    fn normalize_pypi_package_strips_url_prefix_and_trailing_slash() {
        assert_eq!(
            normalize_pypi_package("https://pypi.org/project/mcp-zen-of-docs/"),
            "mcp-zen-of-docs"
        );
        assert_eq!(
            normalize_pypi_package("https://pypi.org/project/mcp-zen-of-docs"),
            "mcp-zen-of-docs"
        );
    }

    #[test]
    fn normalize_pypi_package_strips_version_segment() {
        assert_eq!(
            normalize_pypi_package("https://pypi.org/project/mcp-zen-of-docs/1.2.3/"),
            "mcp-zen-of-docs"
        );
    }

    #[test]
    fn parse_pypi_package_request_extracts_version_segment() {
        assert_eq!(
            parse_pypi_package_request("https://pypi.org/project/mcp-zen-of-docs/1.2.3/"),
            ("mcp-zen-of-docs".to_owned(), Some("1.2.3".to_owned()))
        );
        assert_eq!(
            parse_pypi_package_request("mcp-zen-of-docs"),
            ("mcp-zen-of-docs".to_owned(), None)
        );
    }

    #[test]
    fn normalize_pypi_package_is_identity_for_plain_names() {
        assert_eq!(normalize_pypi_package("mcp-zen-of-docs"), "mcp-zen-of-docs");
        assert_eq!(normalize_pypi_package("mypackage>=1.0"), "mypackage>=1.0");
    }

    #[test]
    fn normalize_npm_package_strips_npmjs_com_url() {
        assert_eq!(
            normalize_npm_package("https://www.npmjs.com/package/@scope/name"),
            "@scope/name"
        );
        assert_eq!(
            normalize_npm_package("https://npmjs.com/package/my-pkg"),
            "my-pkg"
        );
    }

    #[test]
    fn normalize_npm_package_is_identity_for_plain_names() {
        assert_eq!(normalize_npm_package("@scope/name"), "@scope/name");
        assert_eq!(normalize_npm_package("my-pkg"), "my-pkg");
    }

    #[test]
    fn local_path_detection_does_not_misclassify_urls_or_scoped_packages() {
        assert!(!source_looks_like_local_path(
            "https://pypi.org/project/mcp-zen-of-languages/"
        ));
        assert!(!source_looks_like_local_path(
            "https://www.npmjs.com/package/@scope/name"
        ));
        assert!(!source_looks_like_local_path("@scope/name"));
        assert!(!source_looks_like_local_path("mcp-server-git"));
        assert!(source_looks_like_local_path("scripts/server.py"));
        assert!(source_looks_like_local_path("./scripts/server.py"));
        assert!(source_looks_like_local_path("../scripts/server.py"));
    }

    /// Regression test: `--uvx https://pypi.org/project/mcp-zen-of-docs/`
    /// must store the package slug, not the full PyPI URL, in the transport.
    #[test]
    fn uvx_with_pypi_url_normalizes_package_name() {
        let args = AddArgs {
            source: Some("https://pypi.org/project/mcp-zen-of-docs/".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: true,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let (name, source) = normalize_mcp_source(&args, Scope::Local, None).expect("normalize");
        assert_eq!(name, "mcp-zen-of-docs", "derived name should be the slug");
        assert!(
            source.url.is_none(),
            "uvx transport must not set a bogus url"
        );
        match source.transport {
            Some(McpTransport::Uvx { package, .. }) => {
                assert_eq!(
                    package, "mcp-zen-of-docs",
                    "package must be the slug, not the full PyPI URL"
                );
            }
            other => panic!("expected Uvx transport, got {other:?}"),
        }
    }

    #[test]
    fn inferred_transport_for_pypi_url_is_uvx_not_local_path() {
        let args = AddArgs {
            source: Some("https://pypi.org/project/mcp-zen-of-languages/".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let (name, source) = normalize_mcp_source(&args, Scope::Local, None).expect("normalize");
        assert_eq!(name, "mcp-zen-of-languages");
        assert!(
            source.url.is_none(),
            "uvx transport must not set a bogus url"
        );
        match source.transport {
            Some(McpTransport::Uvx { package, .. }) => {
                assert_eq!(package, "mcp-zen-of-languages");
            }
            other => panic!("expected Uvx transport, got {other:?}"),
        }
    }

    #[test]
    fn versioned_pypi_url_sets_manifest_name_and_rev_pin() {
        let args = AddArgs {
            source: Some("https://pypi.org/project/mcp-zen-of-docs/1.2.3/".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let (name, source) = normalize_mcp_source(&args, Scope::Local, None).expect("normalize");
        assert_eq!(name, "mcp-zen-of-docs");
        assert_eq!(source.rev.as_deref(), Some("1.2.3"));
        match source.transport {
            Some(McpTransport::Uvx { package, .. }) => {
                assert_eq!(package, "mcp-zen-of-docs");
            }
            other => panic!("expected Uvx transport, got {other:?}"),
        }
    }

    #[test]
    fn github_repo_url_requires_explicit_mcp_transport() {
        let args = AddArgs {
            source: Some("https://github.com/oraios/serena".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let err = normalize_mcp_source(&args, Scope::Local, None)
            .expect_err("github repo should be rejected");
        match err {
            CpmError::InvalidSource { reason, .. } => {
                assert!(
                    reason.contains("cannot infer an MCP runner"),
                    "expected inference-failure error, got: {reason}"
                );
            }
            other => panic!("expected InvalidSource, got {other:?}"),
        }
    }

    #[test]
    fn docker_image_name_and_pin_are_preserved_for_tagged_images() {
        let args = AddArgs {
            source: Some("ghcr.io/github/github-mcp-server:1.2.3".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: true,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let (name, source) = normalize_mcp_source(&args, Scope::Local, None).expect("normalize");
        assert_eq!(name, "github-mcp-server");
        assert_eq!(source.rev.as_deref(), Some("1.2.3"));
        match source.transport {
            Some(McpTransport::Docker { image, .. }) => {
                assert_eq!(image, "ghcr.io/github/github-mcp-server:1.2.3");
            }
            other => panic!("expected Docker transport, got {other:?}"),
        }
    }

    #[test]
    fn docker_image_digest_is_preserved_as_rev_pin() {
        let args = AddArgs {
            source: Some("ghcr.io/github/github-mcp-server@sha256:deadbeef".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: true,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let (name, source) = normalize_mcp_source(&args, Scope::Local, None).expect("normalize");
        assert_eq!(name, "github-mcp-server");
        assert_eq!(source.rev.as_deref(), Some("sha256:deadbeef"));
    }

    /// Regression test: `--npx https://www.npmjs.com/package/@scope/pkg`
    /// must store the scoped package name, not the full npm URL.
    #[test]
    fn npx_with_npm_url_normalizes_package_name() {
        let args = AddArgs {
            source: Some(
                "https://www.npmjs.com/package/@modelcontextprotocol/server-github".to_owned(),
            ),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: true,
            docker: false,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let (_name, source) = normalize_mcp_source(&args, Scope::Local, None).expect("normalize");
        assert!(
            source.url.is_none(),
            "npx transport must not set a bogus url"
        );
        match source.transport {
            Some(McpTransport::Npx { package, .. }) => {
                assert_eq!(
                    package, "@modelcontextprotocol/server-github",
                    "package must be the scoped name, not the full npm URL"
                );
            }
            other => panic!("expected Npx transport, got {other:?}"),
        }
    }

    #[test]
    fn release_transport_is_rejected_with_clear_error() {
        let args = AddArgs {
            source: Some("https://github.com/org/repo/releases/tag/v1.0.0".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: false,
            script: false,
            release: true,
            bin: Some("my-server".to_owned()),
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let err = normalize_mcp_source(&args, Scope::Local, None)
            .expect_err("--release must be rejected");
        match err {
            CpmError::InvalidSource { reason, .. } => {
                assert!(
                    reason.contains("not yet implemented"),
                    "error must explain that --release is not yet implemented, got: {reason}"
                );
            }
            other => panic!("expected InvalidSource, got {other:?}"),
        }
    }

    #[test]
    fn script_transport_with_args_sets_command_and_args() {
        let args = AddArgs {
            source: Some("python".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: false,
            script: true,
            release: false,
            bin: None,
            path: false,
            command_args: vec!["-m".to_owned(), "my_server".to_owned()],
            env: vec![],
        };

        let (_name, source) = normalize_mcp_source(&args, Scope::Local, None).expect("normalize");
        match source.transport {
            Some(McpTransport::Script { command, args }) => {
                assert_eq!(command, "python");
                assert_eq!(args, vec!["-m", "my_server"]);
            }
            other => panic!("expected Script transport, got {other:?}"),
        }
        // source.args mirrors the transport args so the manifest writer can
        // emit `args = [...]` at the source level (consistent with other transports).
        assert_eq!(source.args, vec!["-m", "my_server"]);
    }

    #[test]
    fn github_repo_with_inferred_uvx_runner_produces_uvx_transport() {
        let args = AddArgs {
            source: Some("https://github.com/org/python-mcp-server".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let (name, source) =
            normalize_mcp_source(&args, Scope::Local, Some(InferredMcpRunner::Uvx))
                .expect("normalize");
        assert_eq!(name, "python-mcp-server");
        match source.transport {
            Some(McpTransport::Uvx { package, .. }) => {
                assert_eq!(package, "python-mcp-server");
            }
            other => panic!("expected Uvx transport, got {other:?}"),
        }
    }

    #[test]
    fn github_repo_with_inferred_npx_runner_produces_npx_transport() {
        let args = AddArgs {
            source: Some("https://github.com/org/node-mcp-server".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let (name, source) =
            normalize_mcp_source(&args, Scope::Local, Some(InferredMcpRunner::Npx))
                .expect("normalize");
        assert_eq!(name, "node-mcp-server");
        match source.transport {
            Some(McpTransport::Npx { package, .. }) => {
                assert_eq!(package, "node-mcp-server");
            }
            other => panic!("expected Npx transport, got {other:?}"),
        }
    }

    #[test]
    fn github_repo_without_inference_produces_clear_error() {
        let args = AddArgs {
            source: Some("https://github.com/org/ambiguous-server".to_owned()),
            url: None,
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        // None means no packaging signals were detected — must error rather than guess.
        let err = normalize_mcp_source(&args, Scope::Local, None)
            .expect_err("ambiguous github repo should error");
        match err {
            CpmError::InvalidSource { reason, .. } => {
                assert!(
                    reason.contains("cannot infer an MCP runner"),
                    "expected inference-failure error, got: {reason}"
                );
            }
            other => panic!("expected InvalidSource, got {other:?}"),
        }
    }

    #[test]
    fn url_based_mcp_adds_infer_kind_and_keep_http_type() {
        let args = AddArgs {
            source: None,
            url: Some("https://api.githubcopilot.com/mcp".to_owned()),
            plugin: false,
            skill: false,
            agent: false,
            mcp: false,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: false,
            transport_type: Some(McpTypeArg::Http),
            npx: false,
            docker: false,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        assert!(matches!(infer_kind(&args), Ok(AssetKind::Mcp)));
        let (name, source) = normalize_mcp_source(&args, Scope::Local, None).expect("normalize");
        assert_eq!(name, "githubcopilot");
        assert_eq!(
            source.url.as_deref(),
            Some("https://api.githubcopilot.com/mcp")
        );
        assert!(matches!(
            source.transport,
            Some(McpTransport::Http { ref url }) if url == "https://api.githubcopilot.com/mcp"
        ));
    }

    #[test]
    fn context7_remote_endpoint_uses_http_and_host_based_name() {
        let args = AddArgs {
            source: None,
            url: Some("https://mcp.context7.com/mcp".to_owned()),
            plugin: false,
            skill: false,
            agent: false,
            mcp: true,
            hook: false,
            workflow: false,
            instruction: false,
            scope: Some(ScopeArg::Local),
            group: "default".to_owned(),
            rev: None,
            sse: true,
            transport_type: None,
            npx: false,
            docker: false,
            uvx: false,
            script: false,
            release: false,
            bin: None,
            path: false,
            command_args: vec![],
            env: vec![],
        };

        let (name, source) = normalize_mcp_source(&args, Scope::Local, None).expect("normalize");
        assert_eq!(name, "context7");
        assert!(matches!(
            source.transport,
            Some(McpTransport::Http { ref url }) if url == "https://mcp.context7.com/mcp"
        ));
    }
}
