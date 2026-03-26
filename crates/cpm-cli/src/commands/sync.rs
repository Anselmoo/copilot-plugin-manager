//! `cpm sync` — install everything in `cpm.lock`.

use std::collections::{HashMap, HashSet};

use clap::Args;
use cpm_core::{
    auth,
    config::{build_http_client, load_runtime_config},
    installer::remove_asset,
    license::enforce_license_policy,
    plugin_index::read_installed_plugins,
    project::{
        apply_manifest, install_resolved_asset, load_global_lockfile, load_lockfile, load_manifest,
        write_global_lockfile, write_lockfile, ApplyOptions,
    },
    resolver::{check_lock_freshness, detect_global_install_conflicts, reconcile_global_lockfile},
    CpmError,
};
use cpm_types::{AssetKind, Lockfile, ResolvedAsset, Scope};

use crate::progress::{OperationKind, OperationStatus, ProgressReporter};

use super::{
    collect_plugin_lock_entries, discovered_plugin_request, effective_asset_scope,
    effective_plugin_scope, merge_delegated_plugin_lock_entries, plugin_asset_is_delegated,
    plugin_requested_spec, plugin_source_is_native, print_plugin_summary, run_plugin_operations,
    strip_delegated_plugins_from_manifest, style_success, PluginOperation,
};

/// Arguments for `cpm sync`.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Also install assets from this named group.
    #[arg(long)]
    pub group: Option<String>,

    /// Only sync assets with this scope.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeArg>,

    /// Fail if the lockfile would change (CI mode).
    #[arg(long)]
    pub frozen: bool,
}

/// Scope argument.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ScopeArg {
    Local,
    Global,
}

pub async fn run(args: SyncArgs) -> Result<(), CpmError> {
    let manifest_path = std::path::Path::new("cpm.toml");
    let lockfile_path = std::path::Path::new("cpm.lock");
    let repo_root = std::path::Path::new(".");
    let manifest = load_manifest(manifest_path)?;
    let existing_lock = load_lockfile(lockfile_path).unwrap_or_default();
    let runtime = load_runtime_config(&manifest)?;
    let scope_filter = args.scope.map(Into::into);
    let selected_plugins: Vec<_> = manifest
        .effective_section(AssetKind::Plugin)
        .into_iter()
        .filter(|(_, source)| {
            should_install(
                source.group.as_str(),
                args.group.as_deref(),
                &runtime.settings.auto_groups,
                scope_filter,
                effective_plugin_scope(source),
            )
        })
        .collect();
    let selected_plugin_keys: HashSet<(String, Scope)> = selected_plugins
        .iter()
        .filter(|(_, source)| !plugin_source_is_native(source))
        .map(|(name, source)| (name.clone(), effective_plugin_scope(source)))
        .collect();
    let selected_plugin_names: HashSet<String> = selected_plugins
        .iter()
        .filter(|(_, source)| !plugin_source_is_native(source))
        .map(|(name, _)| name.clone())
        .collect();
    let previously_managed_plugin_names: HashSet<String> = existing_lock
        .plugins
        .iter()
        .filter(|asset| plugin_asset_is_delegated(asset))
        .filter(|asset| {
            should_install(
                asset.source.group.as_str(),
                args.group.as_deref(),
                &runtime.settings.auto_groups,
                scope_filter,
                effective_asset_scope(asset),
            )
        })
        .map(|asset| asset.name.clone())
        .collect();
    let client = build_http_client(
        format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        &runtime.settings,
    )?;
    let token = auth::resolve_token();

    if args.frozen {
        let lockfile = existing_lock.clone();
        let global_lockfile = load_global_lockfile()?;
        check_lock_freshness(&manifest, &lockfile)?;
        detect_global_install_conflicts(lockfile.all_assets(), &global_lockfile, repo_root)?;
        let frozen_reporter = ProgressReporter::auto();
        for asset in lockfile.all_assets().filter(|asset| {
            asset.kind != AssetKind::Plugin
                && should_install(
                    asset.source.group.as_str(),
                    args.group.as_deref(),
                    &runtime.settings.auto_groups,
                    scope_filter,
                    asset.scope,
                )
        }) {
            enforce_license_policy(asset, &runtime.settings)?;
            let mut handle = frozen_reporter.begin_operation(
                OperationKind::Install,
                format!("{}:{}", asset.kind, asset.name),
            );
            handle.set_status(OperationStatus::Running);
            let result = install_resolved_asset(
                asset,
                &client,
                token.as_deref(),
                repo_root,
                Some(&frozen_reporter),
                &runtime.source_rules,
            )
            .await;
            handle.finish(if result.is_ok() {
                OperationStatus::Succeeded
            } else {
                OperationStatus::Failed
            });
            result?;
        }

        let installed_plugins = read_installed_plugins()?;
        let installed_names: HashSet<String> = installed_plugins
            .iter()
            .filter_map(|plugin| plugin.name.clone())
            .collect();
        let plugin_ops: Vec<_> = lockfile
            .plugins
            .iter()
            .filter(|asset| {
                plugin_asset_is_delegated(asset)
                    && should_install(
                        asset.source.group.as_str(),
                        args.group.as_deref(),
                        &runtime.settings.auto_groups,
                        scope_filter,
                        effective_asset_scope(asset),
                    )
                    && !installed_names.contains(&asset.name)
            })
            .map(|asset| {
                PluginOperation::install(
                    &asset.name,
                    plugin_requested_spec(&asset.name, &asset.source),
                )
            })
            .collect();
        let plugin_summary = run_plugin_operations(plugin_ops).await?;
        print_plugin_summary(plugin_summary);

        let reconciled = reconcile_global_lockfile(&lockfile, &global_lockfile, repo_root)?;
        if reconciled != global_lockfile {
            write_global_lockfile(&reconciled)?;
        }
        println!(
            "{} synced from {}",
            style_success("✓"),
            lockfile_path.display()
        );
        return Ok(());
    }

    let reporter = ProgressReporter::auto();
    let mut lockfile = apply_manifest(
        &strip_delegated_plugins_from_manifest(manifest.clone()),
        &client,
        token.as_deref(),
        ApplyOptions {
            repo_root,
            install: false,
            install_group: None,
            install_scope: None,
            settings: &runtime.settings,
            source_rules: &runtime.source_rules,
            existing_lock: Some(&existing_lock),
            download_progress: Some(&reporter),
        },
    )
    .await?;
    let global_lockfile = load_global_lockfile()?;
    detect_global_install_conflicts(lockfile.all_assets(), &global_lockfile, repo_root)?;

    // Remove non-plugin assets that have been dropped from the manifest or
    // moved to a different scope since the last successful sync.
    for stale in stale_natively_managed_assets(
        &existing_lock,
        &lockfile,
        args.group.as_deref(),
        &runtime.settings.auto_groups,
        scope_filter,
    ) {
        remove_asset(&stale, repo_root)?;
    }

    for asset in lockfile.all_assets().filter(|asset| {
        !plugin_asset_is_delegated(asset)
            && should_install(
                asset.source.group.as_str(),
                args.group.as_deref(),
                &runtime.settings.auto_groups,
                scope_filter,
                asset.scope,
            )
    }) {
        enforce_license_policy(asset, &runtime.settings)?;
        let mut handle = reporter.begin_operation(
            OperationKind::Install,
            format!("{}:{}", asset.kind, asset.name),
        );
        handle.set_status(OperationStatus::Running);
        let result = install_resolved_asset(
            asset,
            &client,
            token.as_deref(),
            repo_root,
            Some(&reporter),
            &runtime.source_rules,
        )
        .await;
        handle.finish(if result.is_ok() {
            OperationStatus::Succeeded
        } else {
            OperationStatus::Failed
        });
        result?;
    }

    let installed_plugins = read_installed_plugins()?;
    let installed_names: HashSet<String> = installed_plugins
        .iter()
        .filter_map(|plugin| plugin.name.clone())
        .collect();
    let mut plugin_ops = Vec::new();
    for (name, source) in &selected_plugins {
        if !plugin_source_is_native(source) && !installed_names.contains(name) {
            plugin_ops.push(PluginOperation::install(
                name,
                plugin_requested_spec(name, source),
            ));
        }
    }
    for name in previously_managed_plugin_names.difference(&selected_plugin_names) {
        if installed_names.contains(name) {
            plugin_ops.push(PluginOperation::remove_with_request(
                name,
                discovered_plugin_request(&installed_plugins, name),
            ));
        }
    }
    let plugin_summary = run_plugin_operations(plugin_ops).await?;
    let installed_plugins = read_installed_plugins()?;
    merge_delegated_plugin_lock_entries(
        &mut lockfile,
        collect_plugin_lock_entries(
            &manifest,
            &existing_lock,
            &installed_plugins,
            Some(&selected_plugin_keys),
            true,
        )?,
    );
    let reconciled = reconcile_global_lockfile(&lockfile, &global_lockfile, repo_root)?;
    if reconciled != global_lockfile {
        write_global_lockfile(&reconciled)?;
    }
    write_lockfile(lockfile_path, &lockfile)?;
    print_plugin_summary(plugin_summary);
    println!(
        "{} synced and wrote {}",
        style_success("✓"),
        lockfile_path.display()
    );
    Ok(())
}

fn should_install(
    group: &str,
    install_group: Option<&str>,
    auto_groups: &[String],
    install_scope: Option<Scope>,
    scope: Scope,
) -> bool {
    let group_matches = group == "default"
        || auto_groups.iter().any(|configured| configured == group)
        || install_group
            .map(|requested| requested == group)
            .unwrap_or(false);
    let scope_matches = install_scope
        .map(|requested| requested == scope)
        .unwrap_or(true);
    group_matches && scope_matches
}

/// Return the non-plugin assets from `existing_lock` that should be removed
/// from disk because they are absent from `new_lock` or moved to a new scope.
///
/// An asset is considered stale when:
/// - It passes the active group/scope filter (same filter applied during install), AND
/// - Its `(name, kind)` key no longer appears in `new_lock`, OR its scope in
///   `new_lock` differs from the scope recorded in `existing_lock`.
fn stale_natively_managed_assets(
    existing_lock: &Lockfile,
    new_lock: &Lockfile,
    install_group: Option<&str>,
    auto_groups: &[String],
    scope_filter: Option<Scope>,
) -> Vec<ResolvedAsset> {
    // Build a lookup: (name, kind) → scope for everything in the new lock.
    let new_map: HashMap<(String, AssetKind), Scope> = new_lock
        .all_assets()
        .filter(|asset| !plugin_asset_is_delegated(asset))
        .map(|a| ((a.name.clone(), a.kind), a.scope))
        .collect();

    existing_lock
        .all_assets()
        .filter(|asset| !plugin_asset_is_delegated(asset))
        .filter(|a| {
            should_install(
                a.source.group.as_str(),
                install_group,
                auto_groups,
                scope_filter,
                a.scope,
            )
        })
        .filter(|a| match new_map.get(&(a.name.clone(), a.kind)) {
            None => true,               // Removed from manifest.
            Some(&ns) => ns != a.scope, // Moved to a different scope.
        })
        .cloned()
        .collect()
}

#[cfg(test)]
fn stale_non_plugin_assets(
    existing_lock: &Lockfile,
    new_lock: &Lockfile,
    install_group: Option<&str>,
    auto_groups: &[String],
    scope_filter: Option<Scope>,
) -> Vec<ResolvedAsset> {
    stale_natively_managed_assets(
        existing_lock,
        new_lock,
        install_group,
        auto_groups,
        scope_filter,
    )
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
    use cpm_types::{AssetOwnership, AssetSource};

    use super::*;

    fn make_test_asset(name: &str, kind: AssetKind, scope: Scope) -> ResolvedAsset {
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
            resolved_rev: "abc123".to_owned(),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:abc".to_owned(),
            scope,
            ownership: AssetOwnership::Upstream,
            files: vec![camino::Utf8PathBuf::from(format!("{name}/file.md")).into()],
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
    fn auto_groups_are_installed_without_explicit_flag() {
        let auto_groups = vec!["research".to_owned()];
        assert!(should_install(
            "research",
            None,
            &auto_groups,
            None,
            Scope::Local
        ));
    }

    #[test]
    fn explicit_scope_filter_still_applies() {
        assert!(!should_install(
            "default",
            None,
            &[],
            Some(Scope::Global),
            Scope::Local
        ));
    }

    #[test]
    fn stale_assets_includes_removed_skill() {
        let mut existing = Lockfile::new();
        existing
            .skills
            .push(make_test_asset("my-skill", AssetKind::Skill, Scope::Local));

        let new_lock = Lockfile::new(); // empty — skill was removed

        let stale = stale_non_plugin_assets(&existing, &new_lock, None, &[], None);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].name, "my-skill");
    }

    #[test]
    fn stale_assets_includes_scope_changed_asset() {
        let mut existing = Lockfile::new();
        existing
            .agents
            .push(make_test_asset("my-agent", AssetKind::Agent, Scope::Local));

        let mut new_lock = Lockfile::new();
        new_lock
            .agents
            .push(make_test_asset("my-agent", AssetKind::Agent, Scope::Global));

        let stale = stale_non_plugin_assets(&existing, &new_lock, None, &[], None);
        assert_eq!(stale.len(), 1, "scope change should be detected as stale");
        assert_eq!(stale[0].scope, Scope::Local, "old scope should be returned");
    }

    #[test]
    fn stale_assets_excludes_unchanged_asset() {
        let mut existing = Lockfile::new();
        existing
            .skills
            .push(make_test_asset("stable", AssetKind::Skill, Scope::Local));

        let mut new_lock = Lockfile::new();
        new_lock
            .skills
            .push(make_test_asset("stable", AssetKind::Skill, Scope::Local));

        let stale = stale_non_plugin_assets(&existing, &new_lock, None, &[], None);
        assert!(stale.is_empty(), "unchanged asset should not be stale");
    }

    #[test]
    fn stale_assets_excludes_plugins() {
        let mut existing = Lockfile::new();
        existing.plugins.push(make_test_asset(
            "my-plugin",
            AssetKind::Plugin,
            Scope::Local,
        ));

        let new_lock = Lockfile::new();

        let stale = stale_non_plugin_assets(&existing, &new_lock, None, &[], None);
        assert!(stale.is_empty(), "plugins should not be cleaned up here");
    }

    #[test]
    fn stale_assets_respects_scope_filter() {
        let mut existing = Lockfile::new();
        existing.skills.push(make_test_asset(
            "local-skill",
            AssetKind::Skill,
            Scope::Local,
        ));
        existing.skills.push(make_test_asset(
            "global-skill",
            AssetKind::Skill,
            Scope::Global,
        ));

        let new_lock = Lockfile::new(); // both removed from manifest

        // Only clean up global scope
        let stale = stale_non_plugin_assets(&existing, &new_lock, None, &[], Some(Scope::Global));
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].name, "global-skill");
    }
}
