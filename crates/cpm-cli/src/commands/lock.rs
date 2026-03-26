//! `cpm lock` — resolve without installing.

use clap::Args;
use cpm_core::{
    auth,
    config::{build_http_client, load_runtime_config},
    plugin_index::read_installed_plugins,
    project::{apply_manifest, load_lockfile, load_manifest, write_lockfile, ApplyOptions},
    resolver::check_lock_freshness,
    CpmError,
};

use crate::progress::ProgressReporter;

use super::{
    collect_plugin_lock_entries, merge_delegated_plugin_lock_entries,
    strip_delegated_plugins_from_manifest,
};

/// Arguments for `cpm lock`.
#[derive(Debug, Args)]
pub struct LockArgs {
    /// Exit 1 if the lockfile is out of date.
    #[arg(long)]
    pub check: bool,
}

pub async fn run(args: LockArgs) -> Result<(), CpmError> {
    let manifest_path = std::path::Path::new("cpm.toml");
    let lockfile_path = std::path::Path::new("cpm.lock");
    let manifest = load_manifest(manifest_path)?;
    let runtime = load_runtime_config(&manifest)?;

    if args.check {
        if !lockfile_path.exists() {
            return Err(CpmError::MissingLockfile);
        }
        let lockfile = load_lockfile(lockfile_path)?;
        check_lock_freshness(&manifest, &lockfile)?;
        println!("✓ cpm.lock is fresh");
        return Ok(());
    }

    let client = build_http_client(
        format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        &runtime.settings,
    )?;
    let token = auth::resolve_token();
    let reporter = ProgressReporter::auto();
    let existing_lock = load_lockfile(lockfile_path).unwrap_or_default();
    let mut lockfile = apply_manifest(
        &strip_delegated_plugins_from_manifest(manifest.clone()),
        &client,
        token.as_deref(),
        ApplyOptions {
            repo_root: std::path::Path::new("."),
            install: false,
            install_group: None,
            install_scope: None,
            settings: &runtime.settings,
            source_rules: &runtime.source_rules,
            existing_lock: None,
            download_progress: Some(&reporter),
        },
    )
    .await?;
    let installed_plugins = read_installed_plugins()?;
    merge_delegated_plugin_lock_entries(
        &mut lockfile,
        collect_plugin_lock_entries(&manifest, &existing_lock, &installed_plugins, None, true)?,
    );
    write_lockfile(lockfile_path, &lockfile)?;
    println!("✓ wrote {}", lockfile_path.display());
    Ok(())
}
