//! `cpm update` — update assets to their latest versions.

use clap::Args;
use cpm_core::{
    auth,
    config::{build_http_client, load_runtime_config},
    plugin_index::{find_installed_plugin_by_name, read_installed_plugins},
    project::{add_single_asset, load_lockfile, load_manifest, write_lockfile, ApplyOptions},
    CpmError,
};
use cpm_types::AssetKind;

use crate::progress::{OperationKind, OperationStatus, ProgressReporter};

use super::{
    collect_plugin_lock_entries, discovered_plugin_request, effective_plugin_scope,
    merge_delegated_plugin_lock_entries, plugin_source_is_native, print_plugin_summary,
    report_skipped_plugin, run_plugin_operations, PluginAction, PluginOperation,
};

/// Arguments for `cpm update`.
#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Name of a single asset to update (updates all when omitted).
    pub name: Option<String>,

    /// Show what would change without actually updating.
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn run(args: UpdateArgs) -> Result<(), CpmError> {
    let manifest = load_manifest(std::path::Path::new("cpm.toml"))?;
    let lockfile_path = std::path::Path::new("cpm.lock");
    let existing_lock = load_lockfile(lockfile_path).unwrap_or_default();
    let all_plugins: Vec<_> = manifest
        .effective_section(AssetKind::Plugin)
        .into_iter()
        .collect();
    let targets: Vec<_> = match &args.name {
        Some(name) => all_plugins
            .into_iter()
            .filter(|(plugin_name, _)| plugin_name == name)
            .collect(),
        None => all_plugins,
    };

    if targets.is_empty() {
        return Err(CpmError::InvalidSource {
            input: args.name.unwrap_or_else(|| "plugins".to_owned()),
            reason: "no managed plugin entries matched the requested update target".to_owned(),
        });
    }

    if args.dry_run {
        for (name, _) in &targets {
            println!("would update plugin '{name}'");
        }
        println!("{} plugin(s) would be updated", targets.len());
        return Ok(());
    }

    let runtime = load_runtime_config(&manifest)?;
    let client = build_http_client(
        format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        &runtime.settings,
    )?;
    let token = auth::resolve_token();
    let reporter = ProgressReporter::auto();
    let mut lockfile = existing_lock.clone();

    for (name, source) in targets
        .iter()
        .filter(|(_, source)| plugin_source_is_native(source))
    {
        let mut handle = reporter.begin_operation(OperationKind::Update, format!("plugin:{name}"));
        handle.set_status(OperationStatus::Running);
        let result = add_single_asset(
            AssetKind::Plugin,
            name,
            source,
            &client,
            token.as_deref(),
            ApplyOptions {
                repo_root: std::path::Path::new("."),
                install: true,
                install_group: None,
                install_scope: None,
                settings: &runtime.settings,
                source_rules: &runtime.source_rules,
                existing_lock: Some(&lockfile),
                download_progress: Some(&reporter),
            },
        )
        .await;
        handle.finish(if result.is_ok() {
            OperationStatus::Succeeded
        } else {
            OperationStatus::Failed
        });
        lockfile = result?;
    }

    let installed_before = read_installed_plugins()?;
    let mut operations = Vec::new();
    for (name, _) in targets
        .iter()
        .filter(|(_, source)| !plugin_source_is_native(source))
    {
        if find_installed_plugin_by_name(&installed_before, name).is_some() {
            operations.push(PluginOperation::update_with_request(
                name,
                discovered_plugin_request(&installed_before, name),
            ));
        } else {
            report_skipped_plugin(PluginAction::Update, name);
        }
    }
    let summary = run_plugin_operations(operations).await?;
    let installed_after = read_installed_plugins()?;
    let refresh_keys = targets
        .iter()
        .filter(|(_, source)| !plugin_source_is_native(source))
        .map(|(name, source)| (name.clone(), effective_plugin_scope(source)))
        .collect();
    merge_delegated_plugin_lock_entries(
        &mut lockfile,
        collect_plugin_lock_entries(
            &manifest,
            &existing_lock,
            &installed_after,
            Some(&refresh_keys),
            true,
        )?,
    );
    write_lockfile(lockfile_path, &lockfile)?;
    print_plugin_summary(summary);
    match &args.name {
        Some(name) => println!("✓ updated plugin '{name}'"),
        None => println!("✓ updated managed plugins"),
    }
    Ok(())
}
