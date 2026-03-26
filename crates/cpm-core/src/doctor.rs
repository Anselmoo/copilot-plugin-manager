//! `cpm doctor` — verify installed file hashes against the lockfile.
//!
//! Walk every `files` entry in `cpm.lock`, recompute its hash, and compare
//! against the stored hash. Exit 0 only if all entries match.

use std::path::Path;

use cpm_types::{AssetKind, GlobalLockfile, Lockfile, ResolvedAsset};
use tracing::{debug, warn};

use crate::fetcher::{hash_installed_files, sha256_file};
use crate::installer::install_dir;
use crate::resolver::{inspect_global_claims, GlobalClaimIssue};
use crate::CpmError;

/// A single hash mismatch found by [`run_doctor`].
#[derive(Debug, Clone)]
pub struct DoctorError {
    /// Asset name.
    pub name: String,
    /// The file whose hash differs (or `"<binary>"` for binary MCPs).
    pub path: String,
    /// Expected hash from the lockfile.
    pub expected: String,
    /// Actual hash computed from disk.
    pub actual: String,
}

/// Verify all installed assets against `lockfile`.
///
/// Returns `Ok(vec![])` when everything matches. Returns a non-empty list of
/// [`DoctorError`]s when mismatches are found — the caller decides whether to
/// surface them as errors or warnings.
///
/// # Errors
/// Returns [`CpmError::Io`] only on unreadable files not caused by a mismatch.
pub fn run_doctor(lockfile: &Lockfile, repo_root: &Path) -> Result<Vec<DoctorError>, CpmError> {
    run_doctor_with_options(lockfile, repo_root, false)
}

/// Verify all installed assets against `lockfile`, optionally stopping after
/// the first mismatch.
///
/// # Errors
/// Returns [`CpmError::Io`] only on unreadable files not caused by a mismatch.
pub fn run_doctor_with_options(
    lockfile: &Lockfile,
    repo_root: &Path,
    fail_fast: bool,
) -> Result<Vec<DoctorError>, CpmError> {
    run_doctor_with_global_lock(lockfile, &GlobalLockfile::new(), repo_root, fail_fast)
}

/// Verify all installed assets against the repo lockfile and the machine-local
/// global claim state.
pub fn run_doctor_with_global_lock(
    lockfile: &Lockfile,
    global_lockfile: &GlobalLockfile,
    repo_root: &Path,
    fail_fast: bool,
) -> Result<Vec<DoctorError>, CpmError> {
    let mut errors = Vec::new();

    for asset in lockfile.all_assets() {
        check_asset(asset, repo_root, &mut errors)?;
        if fail_fast && !errors.is_empty() {
            break;
        }
    }

    if !fail_fast || errors.is_empty() {
        for issue in inspect_global_claims(lockfile, global_lockfile, repo_root)? {
            errors.push(global_issue_error(issue));
            if fail_fast {
                break;
            }
        }
    }

    Ok(errors)
}

fn check_asset(
    asset: &ResolvedAsset,
    repo_root: &Path,
    errors: &mut Vec<DoctorError>,
) -> Result<(), CpmError> {
    // Verify binary MCPs.
    if asset.kind == AssetKind::Mcp {
        if let Some(bin_path) = &asset.bin_path {
            let path = bin_path.as_std_path();
            if !path.exists() {
                warn!("binary not found: {}", path.display());
                errors.push(DoctorError {
                    name: asset.name.clone(),
                    path: path.display().to_string(),
                    expected: asset.hash.clone(),
                    actual: "<missing>".to_owned(),
                });
                return Ok(());
            }
            let actual = sha256_file(path)?;
            if actual != asset.hash {
                errors.push(DoctorError {
                    name: asset.name.clone(),
                    path: path.display().to_string(),
                    expected: asset.hash.clone(),
                    actual,
                });
            }
            return Ok(());
        }
    }

    // Verify regular file assets.
    if asset.files.is_empty() {
        debug!("no files to verify for asset '{}'", asset.name);
        return Ok(());
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

    // Check all files exist.
    for (_relative, _expected_raw, expected_display, path) in &installed_files {
        let path = path.as_path();
        if !path.exists() {
            errors.push(DoctorError {
                name: asset.name.clone(),
                path: path.display().to_string(),
                expected: expected_display
                    .clone()
                    .unwrap_or_else(|| asset.hash.clone()),
                actual: "<missing>".to_owned(),
            });
            return Ok(());
        }
    }

    let file_hashes_complete = asset
        .files
        .iter()
        .all(|file| file.sha256.is_some() || asset.file_hashes.contains_key(&file.path));
    if file_hashes_complete {
        let error_count_before = errors.len();
        for (_relative, expected_raw, expected_display, path) in &installed_files {
            let actual = sha256_file(path)?;
            let Some(expected) = expected_raw else {
                continue;
            };
            if actual.strip_prefix("sha256:").unwrap_or(actual.as_str()) != expected {
                errors.push(DoctorError {
                    name: asset.name.clone(),
                    path: path.display().to_string(),
                    expected: expected_display
                        .clone()
                        .unwrap_or_else(|| format!("sha256:{expected}")),
                    actual,
                });
            }
        }
        if errors.len() > error_count_before {
            return Ok(());
        }
    }

    let full_paths: Vec<_> = installed_files
        .iter()
        .map(|(_, _, _, path)| path.clone())
        .collect();
    let actual = hash_installed_files(&full_paths)?;
    if actual != asset.hash {
        errors.push(DoctorError {
            name: asset.name.clone(),
            path: base.display().to_string(),
            expected: asset.hash.clone(),
            actual,
        });
    }

    Ok(())
}

fn global_issue_error(issue: GlobalClaimIssue) -> DoctorError {
    match issue {
        GlobalClaimIssue::MissingClaim { asset } => DoctorError {
            name: asset.name,
            path: "~/.copilot/cpm.lock".to_owned(),
            expected: "claimed by current repository".to_owned(),
            actual: "<missing>".to_owned(),
        },
        GlobalClaimIssue::OwnedByOtherRepo { asset, claimed_by } => DoctorError {
            name: asset.name,
            path: "~/.copilot/cpm.lock".to_owned(),
            expected: "claimed by current repository".to_owned(),
            actual: claimed_by.to_string(),
        },
        GlobalClaimIssue::ConflictingClaim {
            asset,
            claimed_by,
            claimed_rev,
        } => DoctorError {
            name: asset.name,
            path: "~/.copilot/cpm.lock".to_owned(),
            expected: asset.resolved_rev,
            actual: format!("{claimed_rev} ({claimed_by})"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpm_types::{AssetOwnership, AssetSource, GlobalLockfile, Scope};
    use tempfile::TempDir;

    fn make_asset_with_file(
        name: &str,
        dir: &Path,
        content: &[u8],
        hash: &str,
    ) -> (ResolvedAsset, std::path::PathBuf) {
        let file_path = dir.join(".github/plugins").join(format!("{name}.yml"));
        std::fs::create_dir_all(file_path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file_path, content).expect("write");

        let asset = ResolvedAsset {
            name: name.to_owned(),
            kind: AssetKind::Plugin,
            source: AssetSource {
                url: Some("https://example.com".into()),
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
            hash: hash.to_owned(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![camino::Utf8PathBuf::from(format!("{name}.yml")).into()],
            executable: vec![],
            file_hashes: Default::default(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        };
        (asset, file_path)
    }

    #[test]
    fn doctor_clean_when_hash_matches() {
        let dir = TempDir::new().expect("tempdir");
        let content = b"plugin: data";
        let (asset, file_path) =
            make_asset_with_file("myplugin", dir.path(), content, "sha256:tmp");
        let hash =
            crate::fetcher::hash_installed_files(std::slice::from_ref(&file_path)).expect("hash");
        let mut asset = asset;
        asset.hash = hash;
        let mut lf = Lockfile::new();
        lf.plugins.push(asset);
        let errors = run_doctor(&lf, dir.path()).expect("doctor");
        assert!(errors.is_empty(), "should be clean");
    }

    #[test]
    fn doctor_detects_mismatch() {
        let dir = TempDir::new().expect("tempdir");
        let (asset, file_path) =
            make_asset_with_file("myplugin", dir.path(), b"original", "sha256:wrong_hash");
        // Tamper with the file.
        std::fs::write(&file_path, b"tampered").expect("tamper");
        let mut lf = Lockfile::new();
        lf.plugins.push(asset);
        let errors = run_doctor(&lf, dir.path()).expect("doctor");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].name, "myplugin");
        assert_eq!(errors[0].expected, "sha256:wrong_hash");
    }

    #[test]
    fn doctor_reports_missing_file() {
        let dir = TempDir::new().expect("tempdir");
        let asset = ResolvedAsset {
            name: "gone".to_owned(),
            kind: AssetKind::Plugin,
            source: AssetSource {
                url: Some("https://example.com".into()),
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
            files: vec![camino::Utf8PathBuf::from("gone.yml").into()],
            executable: vec![],
            file_hashes: Default::default(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        };
        let mut lf = Lockfile::new();
        lf.plugins.push(asset);
        let errors = run_doctor(&lf, dir.path()).expect("doctor");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].actual, "<missing>");
    }

    #[test]
    fn doctor_reports_per_file_hash_mismatch() {
        let dir = TempDir::new().expect("tempdir");
        let file_path = dir.path().join(".github/plugins").join("bundle.yml");
        std::fs::create_dir_all(file_path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file_path, b"tampered").expect("write");

        let asset = ResolvedAsset {
            name: "bundle".to_owned(),
            kind: AssetKind::Plugin,
            source: AssetSource {
                url: Some("https://example.com".into()),
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
            hash: "sha256:aggregate".into(),
            scope: Scope::Local,
            ownership: AssetOwnership::Upstream,
            files: vec![cpm_types::LockedFile {
                path: camino::Utf8PathBuf::from("bundle.yml"),
                sha256: Some(
                    crate::fetcher::sha256_hex(b"expected")
                        .strip_prefix("sha256:")
                        .unwrap()
                        .to_owned(),
                ),
                executable: false,
            }],
            executable: vec![],
            file_hashes: Default::default(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        };
        let mut lf = Lockfile::new();
        lf.plugins.push(asset);

        let errors = run_doctor(&lf, dir.path()).expect("doctor");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, file_path.display().to_string());
        assert_eq!(errors[0].expected, crate::fetcher::sha256_hex(b"expected"));
        assert_eq!(errors[0].actual, crate::fetcher::sha256_hex(b"tampered"));
    }

    #[test]
    fn doctor_fail_fast_stops_after_first_mismatch() {
        let dir = TempDir::new().expect("tempdir");
        let content = b"plugin: data";
        let (first, _) = make_asset_with_file("first", dir.path(), content, "sha256:wrong_hash");
        let (second, second_path) =
            make_asset_with_file("second", dir.path(), content, "sha256:also_wrong");
        std::fs::write(second_path, b"tampered-again").expect("tamper second");

        let mut lf = Lockfile::new();
        lf.plugins.push(first);
        lf.plugins.push(second);

        let errors = run_doctor_with_options(&lf, dir.path(), true).expect("doctor fail fast");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].name, "first");
    }

    #[test]
    fn doctor_reports_missing_global_claim() {
        let dir = TempDir::new().expect("tempdir");
        let mut lf = Lockfile::new();
        lf.plugins.push(ResolvedAsset {
            name: "shared".to_owned(),
            kind: AssetKind::Plugin,
            source: AssetSource {
                url: Some("https://example.com".into()),
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
            resolved_date: chrono::Utc::now(),
            hash: "sha256:abc".into(),
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
            plugin_meta: None,
        });

        let errors = run_doctor_with_global_lock(&lf, &GlobalLockfile::new(), dir.path(), false)
            .expect("doctor");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].name, "shared");
        assert_eq!(errors[0].actual, "<missing>");
    }
}
