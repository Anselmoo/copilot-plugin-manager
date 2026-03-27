//! `cpm overview` — consolidated view of manifest, lockfile, and installed assets.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use clap::Args;
use cpm_core::{
    external::{scan_external_assets, ExternalAssets},
    installer::{copilot_mcp_config_path, install_dir, read_copilot_mcp_server_names},
    paths::portable_path_string,
    plugin_index::{
        delegated_plugin_marker_path_by_name, installed_plugin_request, plugin_install_root,
        plugin_install_root_candidates, read_installed_plugins,
    },
    project::{load_global_lockfile, load_lockfile, load_manifest},
    status::{check_status_with_global_lock, AssetStatus},
    CpmError,
};
use cpm_types::{
    AssetKind, GlobalLockfile, Lockfile, Manifest, Scope, SubAsset, SubAssetOwnership,
};
use serde::{Serialize, Serializer};

use super::{
    asset_install_target, asset_source_path, asset_source_url, effective_asset_scope,
    effective_plugin_scope, json_group, json_rev, style_asset_heading, style_count, style_heading,
    style_label,
};

#[derive(Debug, Args)]
pub struct OverviewArgs {
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

    /// Filter by scope.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeArg>,

    /// Include manifest/lock status details.
    #[arg(long)]
    pub with_status: bool,

    /// Discover externally installed assets not tracked by cpm.
    ///
    /// Shows: Copilot-discovered plugins not in the manifest, unclaimed global
    /// assets under ~/.copilot/, assets claimed by other repos, and untracked
    /// MCP servers in ~/.copilot/mcp-config.json.
    #[arg(long)]
    pub external: bool,

    /// Emit JSON instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ScopeArg {
    Local,
    Global,
}

#[derive(Debug, Clone, Serialize)]
struct OverviewSummary {
    manifest_count: usize,
    locked_count: usize,
    sub_asset_count: usize,
    unmanaged_count: usize,
    by_kind: BTreeMap<String, usize>,
    by_scope: BTreeMap<String, usize>,
    groups: BTreeMap<String, usize>,
    status: StatusCounts,
    locked_assets: Vec<LockedAssetRow>,
    nested_assets: Vec<LockedSubAssetRow>,
    unmanaged: Vec<UnmanagedInstall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external: Option<ExternalAssets>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct StatusCounts {
    clean: usize,
    unlocked: usize,
    stale: usize,
    drift: usize,
    global_state: usize,
}

#[derive(Debug, Clone, Serialize)]
struct LockedAssetRow {
    name: String,
    kind: String,
    scope: String,
    group: Option<String>,
    rev: Option<String>,
    sub_asset_count: usize,
    install_target: String,
    source_url: Option<String>,
    source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LockedSubAssetRow {
    parent_name: String,
    parent_kind: String,
    parent_scope: String,
    kind: String,
    name: String,
    path: String,
    ownership: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct UnmanagedInstall {
    #[serde(skip_serializing)]
    pub kind_value: AssetKind,
    #[serde(skip_serializing)]
    pub scope_value: Scope,
    #[serde(serialize_with = "serialize_path")]
    pub full_path: PathBuf,
    pub kind: String,
    pub scope: String,
    pub path: String,
    pub entry_type: String,
    pub file_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_key: Option<String>,
}

pub async fn run(args: OverviewArgs) -> Result<(), CpmError> {
    let manifest = load_manifest(Path::new("cpm.toml"))?;
    let lockfile = load_lockfile(Path::new("cpm.lock"))?;
    let global_lockfile = load_global_lockfile()?;
    let repo_root = Path::new(".");
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
    let statuses =
        check_status_with_global_lock(&manifest, &lockfile, &global_lockfile, repo_root)?;
    let unmanaged = scan_unmanaged_assets(
        &lockfile,
        &global_lockfile,
        repo_root,
        &selected_kinds,
        selected_scope,
    )?;
    let external = if args.external {
        Some(scan_external_assets(
            &manifest,
            &lockfile,
            &global_lockfile,
            repo_root,
            None,
            None,
        )?)
    } else {
        None
    };
    let summary = build_summary(
        &manifest,
        &lockfile,
        &statuses,
        &unmanaged,
        external,
        &selected_kinds,
        selected_scope,
    );

    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }

    println!(
        "Manifest:  {} asset(s)",
        style_count(summary.manifest_count)
    );
    println!("Locked:    {} asset(s)", style_count(summary.locked_count));
    println!(
        "Nested:    {} sub-asset(s)",
        style_count(summary.sub_asset_count)
    );
    println!(
        "Unmanaged: {} file(s)",
        style_count(summary.unmanaged_count)
    );
    if let Some(ext) = &summary.external {
        println!("External:  {} item(s)", style_count(ext.total_count()));
    }
    println!();

    if !summary.by_kind.is_empty() {
        println!("{}", style_heading("By kind:"));
        for (kind, count) in &summary.by_kind {
            println!("  {} {count}", style_label(kind));
        }
        println!();
    }

    if !summary.by_scope.is_empty() {
        println!("{}", style_heading("By scope:"));
        for (scope, count) in &summary.by_scope {
            println!("  {} {count}", style_label(scope));
        }
        println!();
    }

    if !summary.groups.is_empty() {
        println!("{}", style_heading("By group:"));
        for (group, count) in &summary.groups {
            println!("  {} {count}", style_label(group));
        }
        println!();
    }

    println!("{}", style_heading("Status:"));
    println!(
        "  {} {}",
        style_label("clean"),
        style_count(summary.status.clean)
    );
    println!(
        "  {} {}",
        style_label("unlocked"),
        style_count(summary.status.unlocked)
    );
    println!(
        "  {} {}",
        style_label("stale"),
        style_count(summary.status.stale)
    );
    println!(
        "  {} {}",
        style_label("drift"),
        style_count(summary.status.drift)
    );
    println!(
        "  {} {}",
        style_label("global"),
        summary.status.global_state
    );

    if !summary.locked_assets.is_empty() {
        println!();
        println!("{}", style_heading("Locked assets:"));
        for asset in &summary.locked_assets {
            println!(
                "  {}",
                style_asset_heading(&asset.kind, &asset.scope, &asset.name)
            );
            println!("    {} {}", style_label("installed"), asset.install_target);
            if let Some(url) = &asset.source_url {
                println!("    {} {url}", style_label("source-url"));
            }
            if let Some(path) = &asset.source_path {
                println!("    {} {path}", style_label("source-path"));
            }
            if let Some(group) = &asset.group {
                println!("    {} {}", style_label("group"), group);
            }
            if let Some(rev) = &asset.rev {
                println!("    {} {}", style_label("rev"), rev);
            }
            if asset.sub_asset_count > 0 {
                println!(
                    "    {} {}",
                    style_label("sub-assets"),
                    asset.sub_asset_count
                );
            }
        }
    }

    if !summary.nested_assets.is_empty() {
        println!();
        println!("{}", style_heading("Nested assets:"));
        for asset in &summary.nested_assets {
            println!(
                "  {} [{}] {} -> {} {} [{}] path={}",
                asset.parent_kind,
                asset.parent_scope,
                asset.parent_name,
                asset.kind,
                asset.name,
                asset.ownership,
                asset.path
            );
        }
    }

    if args.with_status && !statuses.is_empty() {
        println!();
        println!("{}", style_heading("Status details:"));
        for status in statuses {
            match status {
                AssetStatus::Unlocked { name } => {
                    println!("  {} {name}", style_label("unlocked"))
                }
                AssetStatus::Stale {
                    name,
                    locked_rev,
                    requested_rev,
                } => println!(
                    "  {} {name} locked={locked_rev} requested={requested_rev}",
                    style_label("stale")
                ),
                AssetStatus::Drift { name, detail } => {
                    println!("  {} {name} {detail}", style_label("drift"))
                }
                AssetStatus::GlobalState { name, detail } => {
                    println!("  {} {name} {detail}", style_label("global"))
                }
            }
        }
    }

    if !summary.unmanaged.is_empty() {
        println!();
        println!("{}", style_heading("Unmanaged installs:"));
        for entry in &summary.unmanaged {
            println!(
                "  {}",
                style_asset_heading(&entry.kind, &entry.scope, &entry.path)
            );
            println!(
                "    {} {}",
                style_label("entry"),
                format_unmanaged_entry(entry)
            );
            println!(
                "    {} {}",
                style_label("full-path"),
                entry.full_path.display()
            );
        }
    }

    if let Some(ext) = &summary.external {
        if !ext.unindexed_plugins.is_empty() {
            println!();
            println!(
                "{}",
                style_heading("External plugins (Copilot discovery, not in manifest):")
            );
            for plugin in &ext.unindexed_plugins {
                let name = plugin.name.as_deref().unwrap_or("<unnamed>");
                println!("  {}", style_heading(name));
                if let Some(src) = &plugin.source {
                    println!("    {} {src}", style_label("source"));
                }
                if let Some(ver) = &plugin.version {
                    println!("    {} {ver}", style_label("version"));
                }
                if let Some(reg) = &plugin.registry {
                    println!("    {} {reg}", style_label("registry"));
                }
            }
        }

        if !ext.unclaimed_global.is_empty() {
            println!();
            println!(
                "{}",
                style_heading("Unclaimed global assets (~/.copilot/):")
            );
            for asset in &ext.unclaimed_global {
                println!(
                    "  {}",
                    style_asset_heading(&asset.kind, Scope::Global, &asset.path)
                );
                println!("    {} {} file(s)", style_label("files"), asset.file_count);
                println!(
                    "    {} {}",
                    style_label("full-path"),
                    asset.full_path.display()
                );
            }
        }

        if !ext.cross_repo_claims.is_empty() {
            println!();
            println!("{}", style_heading("Global assets claimed by other repos:"));
            for claim in &ext.cross_repo_claims {
                println!(
                    "  {} [{}] claimed by {}",
                    style_heading(&claim.name),
                    claim.kind,
                    claim.claimed_by
                );
                if let Some(src) = &claim.source {
                    println!("    {} {src}", style_label("source"));
                }
            }
        }

        if !ext.unmanaged_mcp.is_empty() {
            println!();
            println!(
                "{}",
                style_heading("Unmanaged MCP servers (~/.copilot/mcp-config.json):")
            );
            for server in &ext.unmanaged_mcp {
                println!("  {}", style_heading(&server.name));
            }
        }
    }

    Ok(())
}

fn build_summary(
    manifest: &Manifest,
    lockfile: &Lockfile,
    statuses: &[AssetStatus],
    unmanaged: &[UnmanagedInstall],
    external: Option<ExternalAssets>,
    selected_kinds: &[AssetKind],
    selected_scope: Option<Scope>,
) -> OverviewSummary {
    let manifest_count = selected_kinds
        .iter()
        .map(|kind| {
            manifest
                .effective_section(*kind)
                .into_values()
                .filter(|source| {
                    let effective_scope = if *kind == AssetKind::Plugin {
                        effective_plugin_scope(source)
                    } else {
                        source.scope
                    };
                    selected_scope
                        .map(|scope| effective_scope == scope)
                        .unwrap_or(true)
                })
                .count()
        })
        .sum();

    let locked_assets: Vec<_> = lockfile
        .all_assets()
        .filter(|asset| {
            asset_matches(
                asset.kind,
                effective_asset_scope(asset),
                selected_kinds,
                selected_scope,
            )
        })
        .map(|asset| LockedAssetRow {
            name: asset.name.clone(),
            kind: asset.kind.to_string(),
            scope: effective_asset_scope(asset).to_string(),
            group: json_group(&asset.source.group),
            rev: json_rev(&asset.resolved_rev),
            sub_asset_count: asset.sub_assets.len(),
            install_target: asset_install_target(asset),
            source_url: asset_source_url(asset).map(ToOwned::to_owned),
            source_path: asset_source_path(asset).map(ToOwned::to_owned),
        })
        .collect();

    let mut nested_assets = Vec::new();
    let mut sub_asset_count = 0;
    for asset in lockfile
        .all_assets()
        .filter(|asset| asset_matches(asset.kind, asset.scope, selected_kinds, selected_scope))
    {
        for sub_asset in sorted_sub_assets(&asset.sub_assets) {
            sub_asset_count += 1;
            nested_assets.push(LockedSubAssetRow {
                parent_name: asset.name.clone(),
                parent_kind: asset.kind.to_string(),
                parent_scope: asset.scope.to_string(),
                kind: sub_asset.kind.to_string(),
                name: sub_asset.name.clone(),
                path: sub_asset.path.to_string(),
                ownership: format_ownership(sub_asset.ownership).to_owned(),
            });
        }
    }

    let mut by_kind = BTreeMap::new();
    let mut by_scope = BTreeMap::new();
    let mut groups = BTreeMap::new();
    for asset in lockfile
        .all_assets()
        .filter(|asset| asset_matches(asset.kind, asset.scope, selected_kinds, selected_scope))
    {
        *by_kind.entry(asset.kind.to_string()).or_insert(0) += 1;
        *by_scope.entry(asset.scope.to_string()).or_insert(0) += 1;
        *groups.entry(asset.source.group.clone()).or_insert(0) += 1;
    }

    let mut status_counts = StatusCounts::default();
    status_counts.unlocked = statuses
        .iter()
        .filter(|status| matches!(status, AssetStatus::Unlocked { .. }))
        .count();
    status_counts.stale = statuses
        .iter()
        .filter(|status| matches!(status, AssetStatus::Stale { .. }))
        .count();
    status_counts.drift = statuses
        .iter()
        .filter(|status| matches!(status, AssetStatus::Drift { .. }))
        .count();
    status_counts.global_state = statuses
        .iter()
        .filter(|status| matches!(status, AssetStatus::GlobalState { .. }))
        .count();
    status_counts.clean = locked_assets
        .len()
        .saturating_sub(status_counts.stale + status_counts.drift + status_counts.global_state);

    OverviewSummary {
        manifest_count,
        locked_count: locked_assets.len(),
        sub_asset_count,
        unmanaged_count: unmanaged.len(),
        by_kind,
        by_scope,
        groups,
        status: status_counts,
        locked_assets,
        nested_assets,
        unmanaged: unmanaged.to_vec(),
        external,
    }
}

pub(super) fn selected_kinds(
    plugin: bool,
    skill: bool,
    agent: bool,
    mcp: bool,
    hook: bool,
    workflow: bool,
    instruction: bool,
) -> Vec<AssetKind> {
    let mut kinds = Vec::new();
    if plugin {
        kinds.push(AssetKind::Plugin);
    }
    if skill {
        kinds.push(AssetKind::Skill);
    }
    if agent {
        kinds.push(AssetKind::Agent);
    }
    if mcp {
        kinds.push(AssetKind::Mcp);
    }
    if hook {
        kinds.push(AssetKind::Hook);
    }
    if workflow {
        kinds.push(AssetKind::Workflow);
    }
    if instruction {
        kinds.push(AssetKind::Instruction);
    }
    if kinds.is_empty() {
        return vec![
            AssetKind::Plugin,
            AssetKind::Skill,
            AssetKind::Agent,
            AssetKind::Mcp,
            AssetKind::Hook,
            AssetKind::Workflow,
            AssetKind::Instruction,
        ];
    }
    kinds
}

pub(super) fn asset_matches(
    kind: AssetKind,
    scope: Scope,
    selected_kinds: &[AssetKind],
    selected_scope: Option<Scope>,
) -> bool {
    selected_kinds.contains(&kind)
        && selected_scope
            .map(|requested| scope == requested)
            .unwrap_or(true)
}

pub(super) fn scan_unmanaged_assets(
    lockfile: &Lockfile,
    global_lockfile: &GlobalLockfile,
    repo_root: &Path,
    selected_kinds: &[AssetKind],
    selected_scope: Option<Scope>,
) -> Result<Vec<UnmanagedInstall>, CpmError> {
    let mut managed_paths = HashSet::new();
    let mut managed_dirs = HashSet::new();
    let mut claimed_dirs = HashSet::new();
    for asset in lockfile
        .all_assets()
        .filter(|asset| asset_matches(asset.kind, asset.scope, selected_kinds, selected_scope))
    {
        record_managed_asset_paths(
            asset,
            repo_root,
            &mut managed_paths,
            &mut managed_dirs,
            &mut claimed_dirs,
        );
    }
    for asset in global_lockfile
        .all_assets()
        .filter(|asset| asset_matches(asset.kind, asset.scope, selected_kinds, selected_scope))
    {
        record_managed_asset_paths(
            asset,
            repo_root,
            &mut managed_paths,
            &mut managed_dirs,
            &mut claimed_dirs,
        );
    }

    let mut unmanaged = Vec::new();
    let scopes: Vec<_> = selected_scope
        .map(|scope| vec![scope])
        .unwrap_or_else(|| vec![Scope::Local, Scope::Global]);

    for kind in selected_kinds {
        for scope in &scopes {
            if *kind == AssetKind::Workflow && *scope == Scope::Global {
                continue;
            }
            if *kind == AssetKind::Mcp {
                let config_path = copilot_mcp_config_path(*scope, repo_root);
                let managed_names: HashSet<_> = lockfile
                    .all_assets()
                    .chain(global_lockfile.all_assets())
                    .filter(|asset| asset.kind == AssetKind::Mcp && asset.scope == *scope)
                    .map(|asset| asset.name.clone())
                    .collect();
                for name in read_copilot_mcp_server_names(*scope, repo_root)? {
                    if managed_names.contains(&name) {
                        continue;
                    }
                    unmanaged.push(UnmanagedInstall {
                        kind_value: AssetKind::Mcp,
                        scope_value: *scope,
                        full_path: config_path.clone(),
                        kind: AssetKind::Mcp.to_string(),
                        scope: scope.to_string(),
                        path: format!("{}#{name}", portable_path_string(&config_path)),
                        entry_type: "mcp-server".to_owned(),
                        file_count: 1,
                        config_key: Some(name),
                    });
                }
                continue;
            }
            if *kind == AssetKind::Plugin && *scope == Scope::Global {
                let managed_requests: HashSet<_> = lockfile
                    .all_assets()
                    .chain(global_lockfile.all_assets())
                    .filter(|asset| asset.kind == AssetKind::Plugin && asset.scope == *scope)
                    .map(|asset| {
                        let registry = asset
                            .plugin_meta
                            .as_ref()
                            .and_then(|meta| meta.registry.as_deref());
                        cpm_core::plugin_index::plugin_request(&asset.name, registry)
                    })
                    .collect();
                for plugin in read_installed_plugins()? {
                    let Some(name) = plugin.name.as_deref() else {
                        continue;
                    };
                    let request =
                        installed_plugin_request(&plugin).unwrap_or_else(|| name.to_owned());
                    if managed_requests.contains(&request) {
                        continue;
                    }
                    let full_path = plugin_install_root(&plugin)
                        .unwrap_or_else(|| PathBuf::from(format!("plugin:{request}")));
                    let file_count = if full_path.is_dir() {
                        count_files(&full_path).unwrap_or(0)
                    } else {
                        1
                    };
                    unmanaged.push(UnmanagedInstall {
                        kind_value: AssetKind::Plugin,
                        scope_value: *scope,
                        full_path,
                        kind: AssetKind::Plugin.to_string(),
                        scope: scope.to_string(),
                        path: request.clone(),
                        entry_type: "plugin".to_owned(),
                        file_count,
                        config_key: Some(request),
                    });
                }
                continue;
            }
            let root = install_dir(*kind, *scope, repo_root);
            if !root.exists() {
                continue;
            }
            let context = UnmanagedScanContext {
                root: &root,
                managed_paths: &managed_paths,
                managed_dirs: &managed_dirs,
                claimed_dirs: &claimed_dirs,
                kind: *kind,
                scope: *scope,
            };
            collect_unmanaged_entries(&context, &root, &mut unmanaged)?;
        }
    }

    unmanaged.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(unmanaged)
}

struct UnmanagedScanContext<'a> {
    root: &'a Path,
    managed_paths: &'a HashSet<PathBuf>,
    managed_dirs: &'a HashSet<PathBuf>,
    claimed_dirs: &'a HashSet<PathBuf>,
    kind: AssetKind,
    scope: Scope,
}

fn collect_unmanaged_entries(
    context: &UnmanagedScanContext<'_>,
    current: &Path,
    unmanaged: &mut Vec<UnmanagedInstall>,
) -> Result<(), CpmError> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if context.claimed_dirs.contains(&path) {
                continue;
            }
            if context.managed_dirs.contains(&path) {
                collect_unmanaged_entries(context, &path, unmanaged)?;
            } else {
                unmanaged.push(UnmanagedInstall {
                    kind_value: context.kind,
                    scope_value: context.scope,
                    full_path: path.clone(),
                    kind: context.kind.to_string(),
                    scope: context.scope.to_string(),
                    path: unmanaged_path(context.root, &path, true),
                    entry_type: unmanaged_dir_type(context.root, &path).to_owned(),
                    file_count: count_files(&path)?,
                    config_key: None,
                });
            }
        } else if path.is_file() && !context.managed_paths.contains(&path) {
            unmanaged.push(UnmanagedInstall {
                kind_value: context.kind,
                scope_value: context.scope,
                full_path: path.clone(),
                kind: context.kind.to_string(),
                scope: context.scope.to_string(),
                path: unmanaged_path(context.root, &path, false),
                entry_type: "file".to_owned(),
                file_count: 1,
                config_key: None,
            });
        }
    }
    Ok(())
}

fn record_managed_asset_paths(
    asset: &cpm_types::ResolvedAsset,
    repo_root: &Path,
    managed_paths: &mut HashSet<PathBuf>,
    managed_dirs: &mut HashSet<PathBuf>,
    claimed_dirs: &mut HashSet<PathBuf>,
) {
    if asset.kind == AssetKind::Mcp {
        managed_paths.insert(copilot_mcp_config_path(asset.scope, repo_root));
        return;
    }
    if asset.kind == AssetKind::Plugin && asset.files.is_empty() {
        let registry = asset
            .plugin_meta
            .as_ref()
            .and_then(|meta| meta.registry.as_deref());
        for plugin_root in plugin_install_root_candidates(&asset.name, registry) {
            claimed_dirs.insert(plugin_root);
        }
        managed_paths.insert(delegated_plugin_marker_path_by_name(&asset.name));
        return;
    }

    let base = install_dir(asset.kind, asset.scope, repo_root);
    for relative in &asset.files {
        let full = base.join(relative.path.as_std_path());
        managed_paths.insert(full.clone());
        let mut current = full.parent();
        while let Some(dir) = current {
            if !dir.starts_with(&base) {
                break;
            }
            managed_dirs.insert(dir.to_path_buf());
            if dir == base {
                break;
            }
            current = dir.parent();
        }
        if asset.kind == AssetKind::Workflow {
            if let Some(compiled) = compiled_workflow_path(&full) {
                managed_paths.insert(compiled);
            }
        }
    }
}

fn unmanaged_path(root: &Path, path: &Path, is_dir: bool) -> String {
    let mut relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    if is_dir && !relative.ends_with('/') {
        relative.push('/');
    }
    relative
}

fn unmanaged_dir_type(root: &Path, path: &Path) -> &'static str {
    let relative = path.strip_prefix(root).unwrap_or(path);
    if relative.components().count() == 1 {
        "bundle"
    } else {
        "directory"
    }
}

fn count_files(path: &Path) -> Result<usize, CpmError> {
    let mut count = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        if child.is_dir() {
            count += count_files(&child)?;
        } else if child.is_file() {
            count += 1;
        }
    }
    Ok(count)
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

fn sorted_sub_assets(sub_assets: &[SubAsset]) -> Vec<&SubAsset> {
    let mut sub_assets: Vec<_> = sub_assets.iter().collect();
    sub_assets.sort_by(|left, right| {
        left.kind
            .to_string()
            .cmp(&right.kind.to_string())
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.path.cmp(&right.path))
    });
    sub_assets
}

fn format_ownership(ownership: SubAssetOwnership) -> &'static str {
    match ownership {
        SubAssetOwnership::Parent => "parent",
        SubAssetOwnership::Standalone => "standalone",
    }
}

fn serialize_path<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&portable_path_string(path))
}

pub(super) fn compiled_workflow_path(path: &Path) -> Option<PathBuf> {
    let file_name = path.file_name()?.to_str()?;
    let stem = file_name.strip_suffix(".md")?;
    Some(path.with_file_name(format!("{stem}.lock.yml")))
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
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use camino::Utf8PathBuf;
    use cpm_core::project::load_lockfile;
    use tempfile::TempDir;

    use super::*;
    use cpm_types::{
        AssetOwnership, AssetSource, Lockfile, Manifest, PluginMeta, ResolvedAsset, Scope,
        SubAsset, SubAssetOwnership,
    };

    fn make_source() -> AssetSource {
        AssetSource {
            url: Some("https://example.com/partners".to_owned()),
            rev: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()),
            path: Some(Utf8PathBuf::from("plugins/partners")),
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        }
    }

    fn home_env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("lock HOME/USERPROFILE")
    }

    fn make_resolved() -> ResolvedAsset {
        ResolvedAsset {
            name: "partners".to_owned(),
            kind: AssetKind::Plugin,
            source: make_source(),
            resolved_rev: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:partners".to_owned(),
            scope: Scope::Local,
            ownership: cpm_types::AssetOwnership::Upstream,
            files: vec![],
            executable: vec![],
            file_hashes: Default::default(),
            git: None,
            sub_assets: vec![
                SubAsset {
                    name: "terraform".to_owned(),
                    kind: AssetKind::Agent,
                    path: Utf8PathBuf::from("partners/agents/terraform.md"),
                    ownership: SubAssetOwnership::Parent,
                },
                SubAsset {
                    name: "prompt-lib".to_owned(),
                    kind: AssetKind::Skill,
                    path: Utf8PathBuf::from("partners/skills/prompt-lib"),
                    ownership: SubAssetOwnership::Parent,
                },
            ],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        }
    }

    #[test]
    fn selected_kinds_defaults_to_all_supported_kinds() {
        let kinds = selected_kinds(false, false, false, false, false, false, false);
        assert_eq!(
            kinds,
            vec![
                AssetKind::Plugin,
                AssetKind::Skill,
                AssetKind::Agent,
                AssetKind::Mcp,
                AssetKind::Hook,
                AssetKind::Workflow,
                AssetKind::Instruction,
            ]
        );
    }

    #[test]
    fn workflow_compiled_sidecar_is_treated_as_managed() {
        let dir = TempDir::new().expect("tempdir");
        let workflows_dir = dir.path().join(".github").join("workflows");
        let lock_path = dir.path().join("cpm.lock");
        std::fs::create_dir_all(&workflows_dir).expect("create workflows dir");
        std::fs::write(workflows_dir.join("review.md"), "# review\n").expect("write workflow");
        std::fs::write(workflows_dir.join("review.lock.yml"), "name: review\n")
            .expect("write compiled workflow");
        std::fs::write(
            &lock_path,
            r#"
version = 1

[[workflow]]
name = "review"
rev = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
resolved = "2026-03-22T14:05:31Z"
hash = "sha256:test"
scope = "local"
group = "default"
files = ["review.md"]
path = "workflows/review.md"
"#,
        )
        .expect("write lockfile");
        let lockfile = load_lockfile(&lock_path).expect("load lockfile");

        let unmanaged = scan_unmanaged_assets(
            &lockfile,
            &GlobalLockfile::new(),
            dir.path(),
            &[AssetKind::Workflow],
            Some(Scope::Local),
        )
        .expect("scan unmanaged");

        assert!(unmanaged.is_empty());
    }

    #[test]
    fn scan_unmanaged_assets_collapses_bundle_directories() {
        let dir = TempDir::new().expect("tempdir");
        let skills_dir = dir.path().join(".github").join("skills");
        let lock_path = dir.path().join("cpm.lock");
        std::fs::create_dir_all(skills_dir.join("tracked-skill")).expect("create tracked dir");
        std::fs::write(skills_dir.join("tracked-skill/SKILL.md"), "# tracked\n")
            .expect("write tracked");
        std::fs::create_dir_all(skills_dir.join("manual/docs")).expect("create unmanaged dir");
        std::fs::write(skills_dir.join("manual/SKILL.md"), "# manual\n").expect("write manual");
        std::fs::write(skills_dir.join("manual/docs/guide.md"), "# guide\n").expect("write guide");
        std::fs::write(
            &lock_path,
            r#"
version = 1

[[skill]]
name = "tracked-skill"
rev = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
resolved = "2026-03-22T14:05:31Z"
hash = "sha256:test"
scope = "local"
group = "default"
files = ["tracked-skill/SKILL.md"]
path = "skills/tracked-skill"
"#,
        )
        .expect("write lockfile");
        let lockfile = load_lockfile(&lock_path).expect("load lockfile");

        let unmanaged = scan_unmanaged_assets(
            &lockfile,
            &GlobalLockfile::new(),
            dir.path(),
            &[AssetKind::Skill],
            Some(Scope::Local),
        )
        .expect("scan unmanaged");

        assert_eq!(unmanaged.len(), 1);
        assert_eq!(unmanaged[0].path, "manual/");
        assert_eq!(unmanaged[0].entry_type, "bundle");
        assert_eq!(unmanaged[0].file_count, 2);
    }

    #[test]
    fn scan_unmanaged_assets_keeps_nested_files_inside_managed_bundle() {
        let dir = TempDir::new().expect("tempdir");
        let skills_dir = dir.path().join(".github").join("skills");
        let lock_path = dir.path().join("cpm.lock");
        std::fs::create_dir_all(skills_dir.join("tracked-skill/docs")).expect("create tracked dir");
        std::fs::write(skills_dir.join("tracked-skill/SKILL.md"), "# tracked\n")
            .expect("write tracked");
        std::fs::write(skills_dir.join("tracked-skill/docs/notes.md"), "# notes\n")
            .expect("write extra file");
        std::fs::write(
            &lock_path,
            r#"
version = 1

[[skill]]
name = "tracked-skill"
rev = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
resolved = "2026-03-22T14:05:31Z"
hash = "sha256:test"
scope = "local"
group = "default"
files = ["tracked-skill/SKILL.md"]
path = "skills/tracked-skill"
"#,
        )
        .expect("write lockfile");
        let lockfile = load_lockfile(&lock_path).expect("load lockfile");

        let unmanaged = scan_unmanaged_assets(
            &lockfile,
            &GlobalLockfile::new(),
            dir.path(),
            &[AssetKind::Skill],
            Some(Scope::Local),
        )
        .expect("scan unmanaged");

        assert_eq!(unmanaged.len(), 1);
        assert_eq!(unmanaged[0].path, "tracked-skill/docs/");
        assert_eq!(unmanaged[0].entry_type, "directory");
        assert_eq!(unmanaged[0].file_count, 1);
    }

    #[test]
    fn build_summary_includes_nested_assets() {
        let mut manifest = Manifest::default();
        manifest
            .plugins
            .insert("partners".to_owned(), make_source());

        let mut lockfile = Lockfile::new();
        lockfile.plugins.push(make_resolved());

        let summary = build_summary(
            &manifest,
            &lockfile,
            &[],
            &[],
            None,
            &[AssetKind::Plugin],
            Some(Scope::Local),
        );

        assert_eq!(summary.locked_count, 1);
        assert_eq!(summary.sub_asset_count, 2);
        assert_eq!(summary.locked_assets[0].sub_asset_count, 2);
        assert_eq!(summary.nested_assets.len(), 2);
        assert_eq!(summary.nested_assets[0].parent_name, "partners");
        assert_eq!(summary.nested_assets[0].kind, "agent");
        assert_eq!(summary.nested_assets[0].name, "terraform");
        assert_eq!(summary.nested_assets[1].kind, "skill");
    }

    #[test]
    fn scan_unmanaged_assets_discovers_global_plugins_from_modern_install_layout() {
        let _env_lock = home_env_lock();
        let dir = TempDir::new().expect("tempdir");
        let home = TempDir::new().expect("home");
        let previous_home = std::env::var_os("HOME");
        let previous_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", home.path());
        std::env::set_var("USERPROFILE", home.path());

        let plugin_dir = home
            .path()
            .join(".copilot/installed-plugins/awesome-copilot/orphan-plugin");
        std::fs::create_dir_all(plugin_dir.join(".github/plugin")).expect("mkdir plugin dir");
        std::fs::write(
            plugin_dir.join(".github/plugin/plugin.json"),
            br#"{"name":"orphan-plugin"}"#,
        )
        .expect("write plugin json");

        let unmanaged = scan_unmanaged_assets(
            &Lockfile::new(),
            &GlobalLockfile::new(),
            dir.path(),
            &[AssetKind::Plugin],
            Some(Scope::Global),
        )
        .expect("scan unmanaged");

        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match previous_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }

        assert_eq!(unmanaged.len(), 1);
        assert_eq!(unmanaged[0].path, "orphan-plugin@awesome-copilot");
        assert_eq!(unmanaged[0].entry_type, "plugin");
        assert_eq!(
            unmanaged[0].config_key.as_deref(),
            Some("orphan-plugin@awesome-copilot")
        );
    }

    #[test]
    fn scan_unmanaged_assets_keeps_cross_registry_global_plugins_visible() {
        let _env_lock = home_env_lock();
        let dir = TempDir::new().expect("tempdir");
        let home = TempDir::new().expect("home");
        let previous_home = std::env::var_os("HOME");
        let previous_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", home.path());
        std::env::set_var("USERPROFILE", home.path());

        let plugin_dir = home
            .path()
            .join(".copilot/installed-plugins/awesome-copilot/orphan-plugin");
        std::fs::create_dir_all(plugin_dir.join(".github/plugin")).expect("mkdir plugin dir");
        std::fs::write(
            plugin_dir.join(".github/plugin/plugin.json"),
            br#"{"name":"orphan-plugin"}"#,
        )
        .expect("write plugin json");

        let mut lockfile = Lockfile::new();
        lockfile.plugins.push(ResolvedAsset {
            name: "orphan-plugin".to_owned(),
            kind: AssetKind::Plugin,
            source: AssetSource {
                url: Some("https://example.com/orphan-plugin".to_owned()),
                rev: None,
                path: None,
                group: "default".to_owned(),
                scope: Scope::Global,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:orphan".to_owned(),
            scope: Scope::Global,
            ownership: AssetOwnership::Upstream,
            files: vec![],
            executable: vec![],
            file_hashes: Default::default(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: Some(PluginMeta {
                registry: Some("different-registry".to_owned()),
                ..PluginMeta::default()
            }),
        });

        let unmanaged = scan_unmanaged_assets(
            &lockfile,
            &GlobalLockfile::new(),
            dir.path(),
            &[AssetKind::Plugin],
            Some(Scope::Global),
        )
        .expect("scan unmanaged");

        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match previous_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }

        assert_eq!(unmanaged.len(), 1);
        assert_eq!(unmanaged[0].path, "orphan-plugin@awesome-copilot");
        assert_eq!(
            unmanaged[0].config_key.as_deref(),
            Some("orphan-plugin@awesome-copilot")
        );
    }
}
