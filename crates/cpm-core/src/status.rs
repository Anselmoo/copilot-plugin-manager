//! `cpm status` — compare manifest vs lockfile vs disk.
//!
//! Reports four categories:
//! - `Unlocked` — entry is in the manifest but has no lock entry (run `cpm lock`)
//! - `Stale`    — lock entry exists but the manifest requests a different source
//! - `Drift`    — lock and manifest agree, but a file on disk has been modified
//!   or is missing
//! - (empty)    — everything is clean

use std::path::Path;

use cpm_types::{AssetKind, GlobalLockfile, Lockfile, Manifest};
use tracing::debug;

use crate::{
    fetcher::{hash_installed_files, sha256_file},
    installer::install_dir,
    plugin_index::{
        find_installed_plugin_by_name, hash_installed_plugin_manifest, read_installed_plugins,
        InstalledPlugin,
    },
    resolver::{inspect_global_claims, GlobalClaimIssue},
    CpmError,
};

/// Status of a single asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetStatus {
    /// A file on disk differs from what the lock recorded (or is missing).
    Drift {
        /// Asset name.
        name: String,
        /// Human-readable description of the disk mismatch.
        detail: String,
    },
    /// The manifest requests a different source than what is currently locked.
    Stale {
        /// Asset name.
        name: String,
        /// Currently locked rev.
        locked_rev: String,
        /// Rev requested in the manifest (may be a ref/tag or `"(source change)"`).
        requested_rev: String,
    },
    /// Entry exists in the manifest but has no corresponding lockfile entry.
    ///
    /// Run `cpm lock` or `cpm sync` to resolve.
    Unlocked {
        /// Asset name.
        name: String,
    },
    /// The machine-local global lockfile disagrees with the repo lockfile.
    GlobalState {
        /// Asset name.
        name: String,
        /// Human-readable description of the global lock mismatch.
        detail: String,
    },
}

/// Check whether the lockfile is consistent with the manifest and disk state.
///
/// Returns a list of per-asset status items. An **empty** list means every
/// asset is clean. This function does **not** return `Err` for drift/stale —
/// it reports them as status items so the caller can present them nicely.
///
/// The three checks performed, in order, for each manifest entry:
/// 1. **Lock presence** — if no lock entry matches `(name, kind, scope)`,
///    report [`AssetStatus::Unlocked`].
/// 2. **Source equality** — if the lock entry's source differs from the
///    manifest declaration (url, path, group, scope, transport, env, args, rev),
///    report [`AssetStatus::Stale`].
/// 3. **Disk integrity** — if the installed files / binary differ from the
///    locked hash, report [`AssetStatus::Drift`].
///
/// # Errors
/// Returns [`CpmError::Io`] only for unexpected filesystem access failures.
pub fn check_status(
    manifest: &Manifest,
    lockfile: &Lockfile,
    repo_root: &Path,
) -> Result<Vec<AssetStatus>, CpmError> {
    check_status_with_global_lock(manifest, lockfile, &GlobalLockfile::new(), repo_root)
}

/// Check whether the repo lockfile and machine-local global lockfile agree with
/// the manifest and disk state.
pub fn check_status_with_global_lock(
    manifest: &Manifest,
    lockfile: &Lockfile,
    global_lockfile: &GlobalLockfile,
    repo_root: &Path,
) -> Result<Vec<AssetStatus>, CpmError> {
    let installed_plugins = read_installed_plugins()?;
    check_status_with_installed_plugins(
        manifest,
        lockfile,
        global_lockfile,
        repo_root,
        &installed_plugins,
    )
}

fn check_status_with_installed_plugins(
    manifest: &Manifest,
    lockfile: &Lockfile,
    global_lockfile: &GlobalLockfile,
    repo_root: &Path,
    installed_plugins: &[InstalledPlugin],
) -> Result<Vec<AssetStatus>, CpmError> {
    let mut statuses = Vec::new();
    let global_issues = inspect_global_claims(lockfile, global_lockfile, repo_root)?;

    for kind in [
        AssetKind::Plugin,
        AssetKind::Skill,
        AssetKind::Agent,
        AssetKind::Mcp,
        AssetKind::Hook,
        AssetKind::Workflow,
        AssetKind::Instruction,
    ] {
        let section = manifest.effective_section(kind);
        for (name, source) in &section {
            let locked = lockfile
                .all_assets()
                .find(|a| a.name == *name && a.kind == kind && a.scope == source.scope);

            let Some(asset) = locked else {
                statuses.push(AssetStatus::Unlocked { name: name.clone() });
                continue;
            };

            // ── Source equality check ─────────────────────────────────────
            // Any source field change means the lock is stale relative to the
            // manifest.  We use the same comparisons as check_lock_freshness.
            let source_changed = asset.source.url != source.url
                || asset.source.path != source.path
                || asset.source.group != source.group
                || asset.source.scope != source.scope
                || asset.source.transport != source.transport
                || asset.source.env != source.env
                || asset.source.args != source.args;

            let rev_changed = source.rev.as_deref().is_some_and(|requested| {
                !asset.resolved_rev.starts_with(requested) && asset.resolved_rev != requested
            });

            if source_changed || rev_changed {
                statuses.push(AssetStatus::Stale {
                    name: name.clone(),
                    locked_rev: asset.resolved_rev.clone(),
                    requested_rev: source
                        .rev
                        .clone()
                        .unwrap_or_else(|| "(source change)".to_owned()),
                });
                continue;
            }

            // ── Disk integrity check ──────────────────────────────────────
            if let Some(detail) = check_disk_integrity(asset, repo_root, installed_plugins)? {
                statuses.push(AssetStatus::Drift {
                    name: name.clone(),
                    detail,
                });
                continue;
            }

            debug!("asset '{}' ({kind}) is clean", name);

            if let Some(issue) = global_issues
                .iter()
                .find(|issue| issue_matches(issue, name, kind))
            {
                statuses.push(AssetStatus::GlobalState {
                    name: name.clone(),
                    detail: describe_global_issue(issue),
                });
            }
        }
    }

    Ok(statuses)
}

/// Verify that installed files on disk match the locked hash.
///
/// Returns `None` when the asset is intact, or `Some(description)` when drift
/// is detected (missing file or hash mismatch).
fn check_disk_integrity(
    asset: &cpm_types::ResolvedAsset,
    repo_root: &Path,
    installed_plugins: &[InstalledPlugin],
) -> Result<Option<String>, CpmError> {
    if asset.kind == AssetKind::Plugin && asset.files.is_empty() && asset.plugin_meta.is_some() {
        let Some(installed) = find_installed_plugin_by_name(installed_plugins, &asset.name) else {
            return Ok(Some(format!(
                "delegated plugin missing: {} is not installed in Copilot",
                asset.name
            )));
        };

        if let Some(expected) = asset
            .plugin_meta
            .as_ref()
            .and_then(|meta| meta.plugin_json_hash.as_deref())
        {
            let actual = hash_installed_plugin_manifest(installed)?;
            let Some(actual) = actual else {
                return Ok(Some(format!(
                    "plugin manifest missing: expected {} for {}",
                    expected, asset.name
                )));
            };
            if actual != expected {
                return Ok(Some(format!(
                    "plugin manifest hash mismatch: expected {}, got {}",
                    expected, actual
                )));
            }
        }
        return Ok(None);
    }

    // Binary MCPs.
    if asset.kind == AssetKind::Mcp {
        if let Some(bin_path) = &asset.bin_path {
            let path = bin_path.as_std_path();
            if !path.exists() {
                return Ok(Some(format!("binary not found: {}", path.display())));
            }
            let actual = sha256_file(path)?;
            if actual != asset.hash {
                return Ok(Some(format!(
                    "binary hash mismatch: expected {}, got {actual}",
                    asset.hash
                )));
            }
            return Ok(None);
        }
    }

    // Regular file assets.
    if asset.files.is_empty() {
        return Ok(None);
    }

    let base = install_dir(asset.kind, asset.scope, repo_root);
    let installed_files: Vec<_> = asset
        .files
        .iter()
        .map(|file| {
            let legacy_hash = asset.file_hashes.get(&file.path).cloned();
            let expected_raw = file.sha256.clone().or_else(|| {
                legacy_hash
                    .as_ref()
                    .map(|hash| hash.strip_prefix("sha256:").unwrap_or(hash).to_owned())
            });
            let expected_display = file
                .sha256
                .as_ref()
                .map(|hash| format!("sha256:{hash}"))
                .or(legacy_hash);
            (
                &file.path,
                expected_raw,
                expected_display,
                base.join(file.path.as_std_path()),
            )
        })
        .collect();

    for (_relative, _expected_raw, expected_display, path) in &installed_files {
        if !path.exists() {
            let expected = expected_display
                .clone()
                .unwrap_or_else(|| asset.hash.clone());
            return Ok(Some(format!(
                "file missing: {} (expected {expected})",
                path.display()
            )));
        }
    }

    let file_hashes_complete = asset
        .files
        .iter()
        .all(|file| file.sha256.is_some() || asset.file_hashes.contains_key(&file.path));
    if file_hashes_complete {
        for (_relative, expected_raw, expected_display, path) in &installed_files {
            let actual = sha256_file(path)?;
            let Some(expected) = expected_raw else {
                continue;
            };
            if actual.strip_prefix("sha256:").unwrap_or(actual.as_str()) != expected {
                return Ok(Some(format!(
                    "file hash mismatch: {} expected {}, got {actual}",
                    path.display(),
                    expected_display
                        .clone()
                        .unwrap_or_else(|| format!("sha256:{expected}"))
                )));
            }
        }
    }

    let full_paths: Vec<_> = installed_files
        .iter()
        .map(|(_, _, _, path)| path.clone())
        .collect();
    let actual = hash_installed_files(&full_paths)?;
    if actual != asset.hash {
        return Ok(Some(format!(
            "hash mismatch: expected {}, got {actual}",
            asset.hash
        )));
    }

    Ok(None)
}

fn issue_matches(issue: &GlobalClaimIssue, name: &str, kind: AssetKind) -> bool {
    match issue {
        GlobalClaimIssue::MissingClaim { asset }
        | GlobalClaimIssue::OwnedByOtherRepo { asset, .. }
        | GlobalClaimIssue::ConflictingClaim { asset, .. } => {
            asset.name == name && asset.kind == kind
        }
    }
}

fn describe_global_issue(issue: &GlobalClaimIssue) -> String {
    match issue {
        GlobalClaimIssue::MissingClaim { .. } => {
            "missing machine-local claim in ~/.copilot/cpm.lock".to_owned()
        }
        GlobalClaimIssue::OwnedByOtherRepo { claimed_by, .. } => {
            format!("machine-local claim currently belongs to {}", claimed_by)
        }
        GlobalClaimIssue::ConflictingClaim {
            claimed_by,
            claimed_rev,
            ..
        } => format!(
            "machine-local claim from {} records revision {}",
            claimed_by, claimed_rev
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::join_portable_path;
    use cpm_types::{
        AssetSource, GlobalClaim, GlobalLockfile, Lockfile, Manifest, PluginMeta, ResolvedAsset,
        Scope,
    };
    use tempfile::TempDir;

    fn make_source(url: &str, rev: Option<&str>, scope: Scope) -> AssetSource {
        AssetSource {
            url: Some(url.to_owned()),
            rev: rev.map(|r| r.to_owned()),
            path: None,
            group: "default".to_owned(),
            scope,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        }
    }

    fn make_resolved(name: &str, rev: &str, kind: AssetKind, scope: Scope) -> ResolvedAsset {
        ResolvedAsset {
            name: name.to_owned(),
            kind,
            source: make_source("https://example.com", Some(rev), scope),
            resolved_rev: rev.to_owned(),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:abc".to_owned(),
            scope,
            ownership: cpm_types::AssetOwnership::Upstream,
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

    fn installed_plugin(name: &str, path: &std::path::Path) -> InstalledPlugin {
        InstalledPlugin {
            name: Some(name.to_owned()),
            version: Some("1.0.0".to_owned()),
            revision: Some("rev-install".to_owned()),
            source: Some(format!("https://example.com/{name}")),
            registry: Some("awesome-copilot".to_owned()),
            description: None,
            path: camino::Utf8PathBuf::from_path_buf(path.to_path_buf()).ok(),
            enabled: Some(true),
            installed_at: None,
            extra: Default::default(),
        }
    }

    // ── Clean ─────────────────────────────────────────────────────────────────

    #[test]
    fn empty_list_when_manifest_and_lock_match() {
        let dir = TempDir::new().expect("tempdir");
        let plugin_root = join_portable_path(dir.path(), "plugins/p");
        std::fs::create_dir_all(join_portable_path(&plugin_root, ".github/plugin")).expect("mkdir");
        std::fs::write(
            join_portable_path(&plugin_root, ".github/plugin/plugin.json"),
            br#"{"name":"p"}"#,
        )
        .expect("write");
        let installed = installed_plugin("p", &plugin_root);
        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "p".to_owned(),
            make_source("https://example.com", None, Scope::Local),
        );
        let mut lf = Lockfile::new();
        let mut asset = make_resolved("p", &"a".repeat(40), AssetKind::Plugin, Scope::Local);
        asset.plugin_meta = Some(PluginMeta {
            registry: Some("awesome-copilot".to_owned()),
            plugin_version: Some("1.0.0".to_owned()),
            source_url: Some("https://example.com/p".to_owned()),
            plugin_json_hash: hash_installed_plugin_manifest(&installed).expect("hash"),
        });
        lf.plugins.push(asset);
        let statuses = check_status_with_installed_plugins(
            &manifest,
            &lf,
            &GlobalLockfile::new(),
            dir.path(),
            &[installed],
        )
        .expect("status");
        assert!(statuses.is_empty(), "expected clean, got: {statuses:?}");
    }

    // ── Stale ─────────────────────────────────────────────────────────────────

    #[test]
    fn stale_when_rev_differs() {
        let dir = TempDir::new().expect("tempdir");
        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "p".to_owned(),
            make_source("https://example.com", Some("v2.0.0"), Scope::Local),
        );
        let mut lf = Lockfile::new();
        lf.plugins.push(make_resolved(
            "p",
            &"b".repeat(40),
            AssetKind::Plugin,
            Scope::Local,
        ));
        let statuses = check_status(&manifest, &lf, dir.path()).expect("status");
        assert_eq!(statuses.len(), 1);
        assert!(matches!(statuses[0], AssetStatus::Stale { .. }));
    }

    #[test]
    fn stale_when_url_differs() {
        let dir = TempDir::new().expect("tempdir");
        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "p".to_owned(),
            make_source("https://new-url.example.com", None, Scope::Local),
        );
        let mut lf = Lockfile::new();
        lf.plugins.push(make_resolved(
            "p",
            &"a".repeat(40),
            AssetKind::Plugin,
            Scope::Local,
        ));
        // The locked source URL is "https://example.com"; manifest says "https://new-url"
        let statuses = check_status(&manifest, &lf, dir.path()).expect("status");
        assert!(statuses
            .iter()
            .any(|s| matches!(s, AssetStatus::Stale { .. })));
    }

    // ── Unlocked ──────────────────────────────────────────────────────────────

    #[test]
    fn unlocked_when_not_in_lock() {
        let dir = TempDir::new().expect("tempdir");
        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "new-plugin".to_owned(),
            make_source("https://example.com", None, Scope::Local),
        );
        let lf = Lockfile::new();
        let statuses = check_status(&manifest, &lf, dir.path()).expect("status");
        assert_eq!(statuses.len(), 1);
        assert!(matches!(&statuses[0], AssetStatus::Unlocked { name } if name == "new-plugin"));
    }

    // ── Drift ─────────────────────────────────────────────────────────────────

    #[test]
    fn drift_when_file_missing_from_disk() {
        let dir = TempDir::new().expect("tempdir");

        // Create an asset with a file entry that doesn't actually exist on disk.
        let mut asset = make_resolved(
            "missing-file",
            &"a".repeat(40),
            AssetKind::Plugin,
            Scope::Local,
        );
        asset.files = vec![camino::Utf8PathBuf::from("missing.yml").into()];
        asset.hash = "sha256:anything".to_owned();

        let mut manifest = Manifest::default();
        manifest
            .plugins
            .insert("missing-file".to_owned(), asset.source.clone());

        let mut lf = Lockfile::new();
        lf.plugins.push(asset);

        let statuses = check_status(&manifest, &lf, dir.path()).expect("status");
        assert_eq!(statuses.len(), 1);
        assert!(matches!(&statuses[0], AssetStatus::Drift { .. }));
    }

    #[test]
    fn drift_when_instruction_file_missing_from_disk() {
        let dir = TempDir::new().expect("tempdir");

        let mut asset = make_resolved(
            "shell",
            &"a".repeat(40),
            AssetKind::Instruction,
            Scope::Local,
        );
        asset.files = vec![camino::Utf8PathBuf::from("shell.instructions.md").into()];
        asset.hash = "sha256:anything".to_owned();

        let mut manifest = Manifest::default();
        manifest
            .instructions
            .insert("shell".to_owned(), asset.source.clone());

        let mut lf = Lockfile::new();
        lf.instructions.push(asset);

        let statuses = check_status(&manifest, &lf, dir.path()).expect("status");
        assert_eq!(statuses.len(), 1);
        assert!(matches!(
            &statuses[0],
            AssetStatus::Drift { name, detail }
                if name == "shell" && detail.contains("file missing")
        ));
    }

    #[test]
    fn delegated_plugin_reports_missing_install() {
        let dir = TempDir::new().expect("tempdir");
        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "pptx".to_owned(),
            make_source("https://example.com", None, Scope::Local),
        );
        let mut lf = Lockfile::new();
        let mut asset = make_resolved("pptx", &"a".repeat(40), AssetKind::Plugin, Scope::Local);
        asset.plugin_meta = Some(PluginMeta {
            registry: Some("awesome-copilot".to_owned()),
            plugin_version: Some("1.0.0".to_owned()),
            source_url: Some("https://example.com/pptx".to_owned()),
            plugin_json_hash: None,
        });
        lf.plugins.push(asset);

        let statuses = check_status_with_installed_plugins(
            &manifest,
            &lf,
            &GlobalLockfile::new(),
            dir.path(),
            &[],
        )
        .expect("status");

        assert!(matches!(
            statuses.as_slice(),
            [AssetStatus::Drift { name, detail }]
                if name == "pptx" && detail.contains("delegated plugin missing")
        ));
    }

    #[test]
    fn delegated_plugin_reports_manifest_hash_drift() {
        let dir = TempDir::new().expect("tempdir");
        let plugin_root = join_portable_path(dir.path(), "plugins/pptx");
        std::fs::create_dir_all(join_portable_path(&plugin_root, ".github/plugin")).expect("mkdir");
        std::fs::write(
            join_portable_path(&plugin_root, ".github/plugin/plugin.json"),
            br#"{"name":"pptx","version":"2.0.0"}"#,
        )
        .expect("write");
        let installed = installed_plugin("pptx", &plugin_root);

        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "pptx".to_owned(),
            make_source("https://example.com", None, Scope::Local),
        );
        let mut lf = Lockfile::new();
        let mut asset = make_resolved("pptx", &"a".repeat(40), AssetKind::Plugin, Scope::Local);
        asset.plugin_meta = Some(PluginMeta {
            registry: Some("awesome-copilot".to_owned()),
            plugin_version: Some("1.0.0".to_owned()),
            source_url: Some("https://example.com/pptx".to_owned()),
            plugin_json_hash: Some("sha256:expected".to_owned()),
        });
        lf.plugins.push(asset);

        let statuses = check_status_with_installed_plugins(
            &manifest,
            &lf,
            &GlobalLockfile::new(),
            dir.path(),
            &[installed],
        )
        .expect("status");

        assert!(matches!(
            statuses.as_slice(),
            [AssetStatus::Drift { name, detail }]
                if name == "pptx" && detail.contains("plugin manifest hash mismatch")
        ));
    }

    #[test]
    fn regular_plugin_with_empty_files_and_no_plugin_meta_is_clean() {
        let dir = TempDir::new().expect("tempdir");
        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "fixture".to_owned(),
            make_source("https://example.com", None, Scope::Local),
        );
        let mut lf = Lockfile::new();
        lf.plugins.push(make_resolved(
            "fixture",
            &"a".repeat(40),
            AssetKind::Plugin,
            Scope::Local,
        ));

        let statuses = check_status_with_installed_plugins(
            &manifest,
            &lf,
            &GlobalLockfile::new(),
            dir.path(),
            &[],
        )
        .expect("status");

        assert!(statuses.is_empty(), "expected clean, got: {statuses:?}");
    }

    #[test]
    fn drift_reports_per_file_hash_mismatch_detail() {
        let dir = TempDir::new().expect("tempdir");
        let file_path = dir
            .path()
            .join(".github")
            .join("plugins")
            .join("bundle.yml");
        std::fs::create_dir_all(file_path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file_path, b"tampered").expect("write");

        let mut asset = make_resolved("bundle", &"a".repeat(40), AssetKind::Plugin, Scope::Local);
        asset.files = vec![camino::Utf8PathBuf::from("bundle.yml").into()];
        asset.hash = "sha256:aggregate".to_owned();
        asset.file_hashes = [(
            camino::Utf8PathBuf::from("bundle.yml"),
            crate::fetcher::sha256_hex(b"expected"),
        )]
        .into_iter()
        .collect();

        let mut manifest = Manifest::default();
        manifest
            .plugins
            .insert("bundle".to_owned(), asset.source.clone());

        let mut lf = Lockfile::new();
        lf.plugins.push(asset);

        let statuses = check_status(&manifest, &lf, dir.path()).expect("status");
        assert_eq!(statuses.len(), 1);
        assert!(matches!(
            &statuses[0],
            AssetStatus::Drift { name, detail }
                if name == "bundle"
                    && detail.contains("file hash mismatch")
                    && detail.contains("bundle.yml")
                    && detail.contains(&crate::fetcher::sha256_hex(b"expected"))
        ));
    }

    // ── Kind/scope scoping ────────────────────────────────────────────────────

    #[test]
    fn same_name_different_kind_both_unlocked() {
        let dir = TempDir::new().expect("tempdir");
        let plugin_root = join_portable_path(dir.path(), "plugins/x");
        std::fs::create_dir_all(join_portable_path(&plugin_root, ".github/plugin")).expect("mkdir");
        std::fs::write(
            join_portable_path(&plugin_root, ".github/plugin/plugin.json"),
            br#"{"name":"x"}"#,
        )
        .expect("write");
        let installed = installed_plugin("x", &plugin_root);
        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "x".to_owned(),
            make_source("https://example.com", None, Scope::Local),
        );
        manifest.skills.insert(
            "x".to_owned(),
            make_source("https://example.com", None, Scope::Local),
        );
        // Only a plugin named "x" is locked; the skill "x" is not.
        let mut lf = Lockfile::new();
        lf.plugins.push(make_resolved(
            "x",
            &"a".repeat(40),
            AssetKind::Plugin,
            Scope::Local,
        ));

        let statuses = check_status_with_installed_plugins(
            &manifest,
            &lf,
            &GlobalLockfile::new(),
            dir.path(),
            &[installed],
        )
        .expect("status");
        // Plugin "x" should be clean (empty means clean in that section)
        // Skill "x" should be Unlocked.
        let unlocked: Vec<_> = statuses
            .iter()
            .filter(|s| matches!(s, AssetStatus::Unlocked { .. }))
            .collect();
        assert_eq!(unlocked.len(), 1);
    }

    #[test]
    fn global_asset_reports_missing_machine_claim() {
        let dir = TempDir::new().expect("tempdir");
        let plugin_root = join_portable_path(dir.path(), "plugins/shared");
        std::fs::create_dir_all(join_portable_path(&plugin_root, ".github/plugin")).expect("mkdir");
        std::fs::write(
            join_portable_path(&plugin_root, ".github/plugin/plugin.json"),
            br#"{"name":"shared"}"#,
        )
        .expect("write");
        let installed = installed_plugin("shared", &plugin_root);
        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "shared".to_owned(),
            make_source("https://example.com", None, Scope::Global),
        );
        let mut lf = Lockfile::new();
        lf.plugins.push(make_resolved(
            "shared",
            &"a".repeat(40),
            AssetKind::Plugin,
            Scope::Global,
        ));

        let statuses = check_status_with_installed_plugins(
            &manifest,
            &lf,
            &GlobalLockfile::new(),
            dir.path(),
            &[installed],
        )
        .expect("status");

        assert!(statuses.iter().any(|status| matches!(
            status,
            AssetStatus::GlobalState { name, detail }
                if name == "shared" && detail.contains("missing machine-local claim")
        )));
    }

    #[test]
    fn global_asset_reports_cross_repo_conflict() {
        let dir = TempDir::new().expect("tempdir");
        let plugin_root = join_portable_path(dir.path(), "plugins/shared");
        std::fs::create_dir_all(join_portable_path(&plugin_root, ".github/plugin")).expect("mkdir");
        std::fs::write(
            join_portable_path(&plugin_root, ".github/plugin/plugin.json"),
            br#"{"name":"shared"}"#,
        )
        .expect("write");
        let installed = installed_plugin("shared", &plugin_root);
        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "shared".to_owned(),
            make_source("https://example.com", None, Scope::Global),
        );
        let mut lf = Lockfile::new();
        lf.plugins.push(make_resolved(
            "shared",
            &"b".repeat(40),
            AssetKind::Plugin,
            Scope::Global,
        ));

        let mut global_lock = GlobalLockfile::new();
        global_lock.claims.push(GlobalClaim::new(
            camino::Utf8PathBuf::from("/tmp/other-repo"),
            make_resolved("shared", &"a".repeat(40), AssetKind::Plugin, Scope::Global),
        ));

        let statuses = check_status_with_installed_plugins(
            &manifest,
            &lf,
            &global_lock,
            dir.path(),
            &[installed],
        )
        .expect("status");

        assert!(statuses.iter().any(|status| matches!(
            status,
            AssetStatus::GlobalState { name, detail }
                if name == "shared"
                    && detail.contains("/tmp/other-repo")
                    && detail.contains(&"a".repeat(40))
        )));
    }
}
