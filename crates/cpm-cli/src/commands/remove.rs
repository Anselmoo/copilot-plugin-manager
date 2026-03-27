//! `cpm remove` — remove an asset.

use clap::Args;
use cpm_core::{
    installer::remove_asset,
    plugin_index::find_installed_plugin_by_name,
    project::{
        drop_asset_from_lockfile, load_global_lockfile, load_lockfile, load_manifest,
        write_global_lockfile, write_lockfile, write_manifest,
    },
    resolver::reconcile_global_lockfile,
    CpmError,
};
use cpm_types::Scope;

use super::{
    discovered_plugin_request, effective_plugin_scope, plugin_source_is_native,
    print_plugin_summary, remove_manifest_asset, remove_plugin_lock_entry, report_skipped_plugin,
    resolve_required_kind, run_plugin_operations, PluginAction, PluginOperation,
};

/// Arguments for `cpm remove`.
#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// Asset name to remove.
    pub name: String,

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

    /// Scope to remove from.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeArg>,
}

/// Scope argument.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ScopeArg {
    Local,
    Global,
}

pub async fn run(args: RemoveArgs) -> Result<(), CpmError> {
    let kind = resolve_required_kind(
        args.plugin,
        args.skill,
        args.agent,
        args.mcp,
        args.hook,
        args.workflow,
        args.instruction,
    )?;
    let manifest_path = std::path::Path::new("cpm.toml");
    let lockfile_path = std::path::Path::new("cpm.lock");
    let repo_root = std::path::Path::new(".");
    let requested_scope = match args.scope {
        Some(scope) => Some(Scope::from(scope)),
        None if kind == cpm_types::AssetKind::Plugin => None,
        None => Some(Scope::Local),
    };
    let mut manifest = load_manifest(manifest_path)?;

    let removed_source = remove_manifest_asset(&mut manifest, kind, &args.name, requested_scope)
        .ok_or_else(|| CpmError::InvalidSource {
            input: args.name.clone(),
            reason: requested_scope
                .map(|scope| format!("no {kind} named '{}' was found in {scope} scope", args.name))
                .unwrap_or_else(|| format!("no {kind} named '{}' was found", args.name)),
        })?;
    let removed_scope = if kind == cpm_types::AssetKind::Plugin {
        effective_plugin_scope(&removed_source)
    } else {
        removed_source.scope
    };

    if kind == cpm_types::AssetKind::Plugin && !plugin_source_is_native(&removed_source) {
        let installed_plugins = cpm_core::plugin_index::read_installed_plugins()?;
        let mut summary = Default::default();
        if find_installed_plugin_by_name(&installed_plugins, &args.name).is_some() {
            summary = run_plugin_operations(vec![PluginOperation::remove_with_request(
                &args.name,
                discovered_plugin_request(&installed_plugins, &args.name),
            )])
            .await?;
        } else {
            report_skipped_plugin(PluginAction::Remove, &args.name);
        }

        let mut lockfile = load_lockfile(lockfile_path).unwrap_or_default();
        remove_plugin_lock_entry(&mut lockfile, &args.name, removed_scope);
        write_manifest(manifest_path, &manifest)?;
        write_lockfile(lockfile_path, &lockfile)?;
        let global_lockfile = load_global_lockfile()?;
        let reconciled = reconcile_global_lockfile(&lockfile, &global_lockfile, repo_root)?;
        if reconciled != global_lockfile {
            write_global_lockfile(&reconciled)?;
        }
        print_plugin_summary(summary);
        println!(
            "✓ removed plugin '{}' from {removed_scope} scope",
            args.name
        );
        return Ok(());
    }

    // Load the existing lockfile, find the entry we're removing (to know
    // which files to delete from disk), then drop it in place.  We must NOT
    // call apply_manifest here: doing so would re-resolve every remaining
    // manifest entry (including GitHub-backed plugins/skills) through the
    // GitHub API, causing spurious auth errors when those entries haven't
    // changed at all.
    let mut lockfile = load_lockfile(lockfile_path).unwrap_or_default();
    let previous_asset = lockfile
        .all_assets()
        .find(|asset| asset.kind == kind && asset.name == args.name && asset.scope == removed_scope)
        .cloned();
    drop_asset_from_lockfile(&mut lockfile, kind, &args.name, Some(removed_scope));

    write_manifest(manifest_path, &manifest)?;
    write_lockfile(lockfile_path, &lockfile)?;
    let global_lockfile = load_global_lockfile()?;
    let reconciled = reconcile_global_lockfile(&lockfile, &global_lockfile, repo_root)?;
    if reconciled != global_lockfile {
        write_global_lockfile(&reconciled)?;
    }

    if let Some(previous_asset) = &previous_asset {
        remove_asset(previous_asset, repo_root)?;
    }

    println!(
        "✓ removed {kind} '{}' from {removed_scope} scope",
        args.name
    );
    Ok(())
}

impl From<ScopeArg> for Scope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::Local => Scope::Local,
            ScopeArg::Global => Scope::Global,
        }
    }
}
