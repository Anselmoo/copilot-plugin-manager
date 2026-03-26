//! `cpm reset` — remove managed installs and optionally unmanaged files.

use std::collections::BTreeMap;
use std::path::Path;

use clap::Args;
use cpm_core::{
    installer::remove_asset,
    plugin_index::{
        delegated_plugin_install_root_by_name, delegated_plugin_marker_path_by_name,
        find_installed_plugin_by_name,
    },
    project::{
        drop_asset_from_lockfile, load_global_lockfile, load_lockfile, load_manifest,
        write_global_lockfile, write_lockfile, write_manifest,
    },
    resolver::canonical_repo_root,
    CpmError,
};
use cpm_types::{AssetKind, Scope};

use super::{
    discovered_plugin_request, effective_asset_scope,
    overview::{
        asset_matches, compiled_workflow_path, scan_unmanaged_assets, selected_kinds,
        UnmanagedInstall,
    },
    plugin_asset_is_delegated, print_plugin_summary, remove_manifest_asset, report_skipped_plugin,
    run_plugin_operations, style_count, style_heading, style_success, PluginAction,
    PluginOperation, PluginOperationSummary,
};

#[derive(Debug, Args)]
pub struct ResetArgs {
    #[arg(long, group = "kind")]
    pub plugin: bool,
    #[arg(long, group = "kind")]
    pub skill: bool,
    #[arg(long, group = "kind")]
    pub agent: bool,
    #[arg(long, group = "kind")]
    pub mcp: bool,
    #[arg(long, group = "kind")]
    pub hook: bool,
    #[arg(long, group = "kind")]
    pub workflow: bool,
    #[arg(long, group = "kind")]
    pub instruction: bool,

    /// Restrict reset to one scope.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeArg>,

    /// Show what would be removed without changing files.
    #[arg(long)]
    pub dry_run: bool,

    /// Also remove unmanaged files found in install roots.
    #[arg(long)]
    pub hard: bool,

    /// Skip confirmation guard.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ScopeArg {
    Local,
    Global,
}

pub async fn run(args: ResetArgs) -> Result<(), CpmError> {
    let selected_kinds = selected_kinds(
        args.plugin,
        args.skill,
        args.agent,
        args.mcp,
        args.hook,
        args.workflow,
        args.instruction,
    );
    let selected_scope = args.scope.map(Into::into);
    let manifest_path = Path::new("cpm.toml");
    let lockfile_path = Path::new("cpm.lock");
    let repo_root = Path::new(".");
    let mut manifest = load_manifest(manifest_path)?;
    let lockfile = load_lockfile(lockfile_path)?;
    let global_lockfile = load_global_lockfile()?;

    let managed_assets: Vec<_> = lockfile
        .all_assets()
        .filter(|asset| {
            asset_matches(
                asset.kind,
                effective_asset_scope(asset),
                &selected_kinds,
                selected_scope,
            )
        })
        .cloned()
        .collect();
    let scanned_unmanaged = args.hard;
    let unmanaged = if scanned_unmanaged {
        scan_unmanaged_assets(
            &lockfile,
            &global_lockfile,
            repo_root,
            &selected_kinds,
            selected_scope,
        )?
    } else {
        Vec::new()
    };

    if args.dry_run {
        print_plan(&managed_assets, scanned_unmanaged, &unmanaged);
        return Ok(());
    }

    if !args.force {
        return Err(CpmError::InvalidConfig {
            key: "reset".to_owned(),
            reason: "reset is destructive; pass --dry-run to inspect or --force to execute"
                .to_owned(),
        });
    }

    let local_prune_stop = repo_root;
    let global_prune_stop =
        cpm_core::installer::install_dir(AssetKind::Plugin, Scope::Global, repo_root)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));

    for asset in &managed_assets {
        remove_manifest_asset(
            &mut manifest,
            asset.kind,
            &asset.name,
            Some(effective_asset_scope(asset)),
        );
    }

    // Partition managed assets: plugins must go through the copilot delegate
    // (`copilot plugin uninstall`), not raw file deletion.
    let (plugin_assets, non_plugin_assets): (Vec<_>, Vec<_>) = managed_assets
        .iter()
        .partition(|asset| plugin_asset_is_delegated(asset));

    for asset in &non_plugin_assets {
        remove_asset(asset, repo_root)?;
        if asset.kind == AssetKind::Workflow {
            for relative in &asset.files {
                let full = cpm_core::installer::install_dir(asset.kind, asset.scope, repo_root)
                    .join(relative.path.as_std_path());
                if let Some(compiled) = compiled_workflow_path(&full) {
                    if compiled.exists() {
                        std::fs::remove_file(&compiled)?;
                        prune_empty_dirs(
                            compiled.parent(),
                            prune_stop(asset.scope, local_prune_stop, &global_prune_stop),
                        )?;
                    }
                }
            }
        }
    }

    if !plugin_assets.is_empty() {
        let installed_plugins = cpm_core::plugin_index::read_installed_plugins()?;
        let mut ops = Vec::new();
        for asset in &plugin_assets {
            if find_installed_plugin_by_name(&installed_plugins, &asset.name).is_some() {
                ops.push(PluginOperation::remove_with_request(
                    &asset.name,
                    discovered_plugin_request(&installed_plugins, &asset.name),
                ));
            } else {
                report_skipped_plugin(PluginAction::Remove, &asset.name);
            }
        }
        if !ops.is_empty() {
            let summary = run_plugin_operations(ops).await?;
            print_plugin_summary(summary);
        }
    }

    if args.hard {
        // Collect unique plugin names first to avoid calling `copilot plugin
        // uninstall` twice for the same plugin when both a directory
        // (`<name>/`) and a marker file (`<name>.installed`) appear in the
        // unmanaged list for the same plugin.
        let mut unmanaged_plugin_requests: BTreeMap<String, Vec<&UnmanagedInstall>> =
            BTreeMap::new();
        for entry in &unmanaged {
            if let Some(plugin_request) = delegated_unmanaged_plugin_request(entry) {
                unmanaged_plugin_requests
                    .entry(plugin_request)
                    .or_default()
                    .push(entry);
            }
        }

        if !unmanaged_plugin_requests.is_empty() {
            let mut summary = PluginOperationSummary::default();
            for (request, entries) in &unmanaged_plugin_requests {
                let subject = request.split('@').next().unwrap_or(request.as_str());
                match run_plugin_operations(vec![PluginOperation::remove_with_request(
                    subject,
                    request.clone(),
                )])
                .await
                {
                    Ok(operation_summary) => {
                        summary.removed += operation_summary.removed;
                    }
                    Err(err) if plugin_command_reports_not_installed(&err) => {
                        if cleanup_stale_plugin_install(
                            request,
                            entries,
                            local_prune_stop,
                            &global_prune_stop,
                        )? {
                            summary.removed += 1;
                        } else {
                            return Err(err);
                        }
                    }
                    Err(err) => return Err(err),
                }
            }
            print_plugin_summary(summary);
        }

        for entry in &unmanaged {
            if delegated_unmanaged_plugin_request(entry).is_some() {
                // Already handled via deduplicated uninstall ops above.
                continue;
            }
            if !entry.full_path.exists() {
                continue;
            }
            if entry.kind_value == AssetKind::Mcp {
                if let Some(name) = &entry.config_key {
                    cpm_core::installer::remove_copilot_mcp_server_by_name(
                        name,
                        entry.scope_value,
                        repo_root,
                    )?;
                }
                continue;
            }
            if entry.full_path.is_dir() {
                std::fs::remove_dir_all(&entry.full_path)?;
            } else {
                std::fs::remove_file(&entry.full_path)?;
            }
            let root =
                cpm_core::installer::install_dir(entry.kind_value, entry.scope_value, repo_root);
            prune_empty_dirs(
                entry.full_path.parent(),
                prune_stop(entry.scope_value, local_prune_stop, &global_prune_stop),
            )?;
            prune_empty_dirs(
                Some(root.as_path()),
                prune_stop(entry.scope_value, local_prune_stop, &global_prune_stop),
            )?;
        }
    }

    // Drop entries for removed assets from the local lockfile.  Calling
    // apply_manifest here would re-resolve every remaining entry through the
    // GitHub API – an unnecessary, expensive, and often auth-blocked operation
    // for entries that haven't changed.
    let mut rebuilt = lockfile.clone();
    for asset in &managed_assets {
        drop_asset_from_lockfile(&mut rebuilt, asset.kind, &asset.name, Some(asset.scope));
    }

    write_manifest(manifest_path, &manifest)?;
    write_lockfile(lockfile_path, &rebuilt)?;

    // Reconcile the global lockfile: remove claims owned by this repo for any
    // global assets we just deleted.  Without this step the global lock would
    // permanently hold stale ownership records.
    //
    // Use canonical_repo_root (same as reconcile_global_lockfile) so symlinks
    // and path variations don't leave stale claims behind.
    let has_global_assets = managed_assets.iter().any(|a| a.scope == Scope::Global);
    if has_global_assets {
        if let Ok(repo_path) = canonical_repo_root(repo_root) {
            let mut updated_global = global_lockfile.clone();
            updated_global.claims.retain(|claim| {
                // Keep claims from other repos.
                if claim.claimed_by != repo_path {
                    return true;
                }
                // Drop claims for assets we just removed from this repo.
                !managed_assets.iter().any(|a| {
                    a.scope == Scope::Global
                        && a.name == claim.asset.name
                        && a.kind == claim.asset.kind
                })
            });
            write_global_lockfile(&updated_global)?;
        }
    }

    println!(
        "{} reset removed {} managed asset(s){}",
        style_success("✓"),
        style_count(managed_assets.len()),
        if args.hard {
            format!(" and {} unmanaged install(s)", style_count(unmanaged.len()))
        } else {
            String::new()
        }
    );
    Ok(())
}

fn delegated_unmanaged_plugin_request(entry: &UnmanagedInstall) -> Option<String> {
    if entry.kind_value != AssetKind::Plugin {
        return None;
    }
    entry
        .config_key
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            entry
                .full_path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".installed"))
                .map(ToOwned::to_owned)
        })
}

fn print_plan(
    managed_assets: &[cpm_types::ResolvedAsset],
    scanned_unmanaged: bool,
    unmanaged: &[UnmanagedInstall],
) {
    println!("{}", style_heading("Reset plan:"));
    println!("  managed assets: {}", style_count(managed_assets.len()));
    if scanned_unmanaged {
        println!("  unmanaged installs: {}", style_count(unmanaged.len()));
    } else {
        println!("  unmanaged installs: skipped (pass --hard to scan)");
    }
    if !managed_assets.is_empty() {
        println!();
        println!("Managed assets to remove:");
        for asset in managed_assets {
            println!("  {} [{}] {}", asset.kind, asset.scope, asset.name);
        }
    }
    if !unmanaged.is_empty() {
        println!();
        println!("Unmanaged installs to remove:");
        for entry in unmanaged {
            println!(
                "  {} [{}] {}",
                entry.kind,
                entry.scope,
                format_unmanaged_entry(entry)
            );
        }
    }
}

fn format_unmanaged_entry(entry: &UnmanagedInstall) -> String {
    match entry.entry_type.as_str() {
        "bundle" => format!(
            "{} (bundle directory, {} file(s))",
            entry.path, entry.file_count
        ),
        "directory" => format!("{} (directory, {} file(s))", entry.path, entry.file_count),
        _ => entry.path.clone(),
    }
}

fn plugin_command_reports_not_installed(err: &CpmError) -> bool {
    match err {
        CpmError::PluginCommandFailed { stdout, stderr, .. } => {
            stdout.contains("is not installed") || stderr.contains("is not installed")
        }
        _ => false,
    }
}

fn cleanup_stale_plugin_install(
    request: &str,
    entries: &[&UnmanagedInstall],
    local_prune_stop: &Path,
    global_prune_stop: &Path,
) -> Result<bool, CpmError> {
    let subject = request.split('@').next().unwrap_or(request);
    let registry = request.split_once('@').map(|(_, registry)| registry);
    let mut removed_any = false;

    for entry in entries {
        if entry.full_path.exists() {
            if entry.full_path.is_dir() {
                std::fs::remove_dir_all(&entry.full_path)?;
            } else {
                std::fs::remove_file(&entry.full_path)?;
            }
            removed_any = true;
            prune_empty_dirs(
                entry.full_path.parent(),
                prune_stop(entry.scope_value, local_prune_stop, global_prune_stop),
            )?;
        }
    }

    let legacy_marker = delegated_plugin_marker_path_by_name(subject);
    if legacy_marker.exists() {
        std::fs::remove_file(&legacy_marker)?;
        removed_any = true;
        prune_empty_dirs(
            legacy_marker.parent(),
            prune_stop(Scope::Global, local_prune_stop, global_prune_stop),
        )?;
    }

    let legacy_root = delegated_plugin_install_root_by_name(subject);
    if legacy_root.exists() {
        std::fs::remove_dir_all(&legacy_root)?;
        removed_any = true;
        prune_empty_dirs(
            legacy_root.parent(),
            prune_stop(Scope::Global, local_prune_stop, global_prune_stop),
        )?;
    }

    if remove_stale_plugin_config_entry(subject, registry)? {
        removed_any = true;
    }

    Ok(removed_any)
}

fn remove_stale_plugin_config_entry(
    subject: &str,
    registry: Option<&str>,
) -> Result<bool, CpmError> {
    let Some(config_dir) = cpm_core::plugin_index::default_plugin_index_path()
        .parent()
        .map(Path::to_path_buf)
    else {
        return Ok(false);
    };
    let config_path = config_dir.join("config.json");
    if !config_path.exists() {
        return Ok(false);
    }

    let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&config_path)?)?;
    let Some(installed_plugins) = value
        .get_mut("installed_plugins")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(false);
    };

    let before = installed_plugins.len();
    installed_plugins.retain(|plugin| {
        let name = plugin.get("name").and_then(serde_json::Value::as_str);
        let marketplace = plugin
            .get("marketplace")
            .or_else(|| plugin.get("registry"))
            .and_then(serde_json::Value::as_str);
        !(name == Some(subject)
            && registry
                .map(|value| marketplace == Some(value))
                .unwrap_or(true))
    });

    if installed_plugins.len() == before {
        return Ok(false);
    }

    std::fs::write(&config_path, serde_json::to_vec_pretty(&value)?)?;
    Ok(true)
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

fn prune_stop<'a>(scope: Scope, local_stop: &'a Path, global_stop: &'a Path) -> &'a Path {
    match scope {
        Scope::Local => local_stop,
        Scope::Global => global_stop,
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
