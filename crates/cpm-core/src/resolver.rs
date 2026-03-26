//! Git ref and npm package resolver.
//!
//! # Behaviour
//! - Resolves a `rev` (tag / branch / SHA) to a full 40-character git SHA.
//! - For npm (npx) MCPs: queries `registry.npmjs.org/<pkg>/latest`.
//! - For GitHub Release binaries: queries the GitHub Releases API.
//! - Raises [`CpmError::ScopeConflict`] immediately when the same `name + kind`
//!   appears in both `local` and `global` scope during a resolve pass.
//! - Never silently downgrades to a cached SHA on network failure.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use camino::Utf8PathBuf;
use chrono::Utc;
use cpm_types::{
    AssetKind, GlobalClaim, GlobalLockfile, Lockfile, Manifest, ResolvedAsset, Scope, SourceRule,
};
use indexmap::IndexMap;
use tracing::{debug, info};

use crate::{source::resolve_pinned_rev, CpmError};

/// Check the manifest for local/global scope conflicts and return an error on
/// the first one found.
///
/// A conflict exists when the *same* `name + kind` pair appears in both the
/// local and global sections of the lockfile simultaneously.
pub fn detect_conflicts(lockfile: &Lockfile) -> Result<(), CpmError> {
    // Build a set of (name, kind, scope) triples and look for duplicates across
    // scopes.
    let mut seen: HashMap<(String, AssetKind), Vec<Scope>> = HashMap::new();

    for asset in lockfile.all_assets() {
        seen.entry((asset.name.clone(), asset.kind))
            .or_default()
            .push(asset.scope);
    }

    for ((name, kind), scopes) in &seen {
        let has_local = scopes.contains(&Scope::Local);
        let has_global = scopes.contains(&Scope::Global);
        if has_local && has_global {
            return Err(CpmError::ScopeConflict {
                name: name.clone(),
                kind: *kind,
            });
        }
    }

    Ok(())
}

/// Global claim state for an asset declared as `scope = "global"` in a repo
/// lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalClaimIssue {
    /// The repo lockfile expects a global claim, but none exists on this
    /// machine.
    MissingClaim {
        /// Locked asset missing from `~/.copilot/cpm.lock`.
        asset: ResolvedAsset,
    },
    /// The asset is installed globally, but another repository currently owns
    /// the machine-local claim.
    OwnedByOtherRepo {
        /// Locked asset in the current repository.
        asset: ResolvedAsset,
        /// Repository currently recorded as the owner.
        claimed_by: Utf8PathBuf,
    },
    /// Another claim exists for the same `(name, kind)` pair, but it points at
    /// a different resolved asset.
    ConflictingClaim {
        /// Locked asset in the current repository.
        asset: ResolvedAsset,
        /// Repository currently recorded as the owner.
        claimed_by: Utf8PathBuf,
        /// Revision currently recorded in the machine-local global lockfile.
        claimed_rev: String,
    },
}

/// Detect whether the provided global assets would overwrite a conflicting
/// claim from another repository.
pub fn detect_global_install_conflicts<'a>(
    assets: impl IntoIterator<Item = &'a ResolvedAsset>,
    global_lockfile: &GlobalLockfile,
    repo_root: &Path,
) -> Result<(), CpmError> {
    let repo_path = canonical_repo_root(repo_root)?;

    for asset in assets
        .into_iter()
        .filter(|asset| asset.scope == Scope::Global)
    {
        let Some(claim) = find_claim(global_lockfile, asset) else {
            continue;
        };

        if claim.claimed_by != repo_path && claim.asset != *asset {
            return Err(CpmError::GlobalInstallConflict {
                name: asset.name.clone(),
                kind: asset.kind,
                claimed_by: claim.claimed_by.to_string(),
                installed_rev: claim.asset.resolved_rev.clone(),
                requested_rev: asset.resolved_rev.clone(),
            });
        }
    }

    Ok(())
}

/// Inspect the repo lockfile against the machine-local global lockfile and
/// return any claim issues for global-scoped assets.
pub fn inspect_global_claims(
    lockfile: &Lockfile,
    global_lockfile: &GlobalLockfile,
    repo_root: &Path,
) -> Result<Vec<GlobalClaimIssue>, CpmError> {
    let repo_path = canonical_repo_root(repo_root)?;
    let mut issues = Vec::new();

    for asset in lockfile
        .all_assets()
        .filter(|asset| asset.scope == Scope::Global)
    {
        match find_claim(global_lockfile, asset) {
            None => issues.push(GlobalClaimIssue::MissingClaim {
                asset: asset.clone(),
            }),
            Some(claim) if claim.asset != *asset => {
                issues.push(GlobalClaimIssue::ConflictingClaim {
                    asset: asset.clone(),
                    claimed_by: claim.claimed_by.clone(),
                    claimed_rev: claim.asset.resolved_rev.clone(),
                });
            }
            Some(claim) if claim.claimed_by != repo_path => {
                issues.push(GlobalClaimIssue::OwnedByOtherRepo {
                    asset: asset.clone(),
                    claimed_by: claim.claimed_by.clone(),
                });
            }
            Some(_) => {}
        }
    }

    Ok(issues)
}

/// Reconcile the machine-local global lockfile so that it reflects the current
/// repository lockfile's global-scoped assets.
pub fn reconcile_global_lockfile(
    lockfile: &Lockfile,
    global_lockfile: &GlobalLockfile,
    repo_root: &Path,
) -> Result<GlobalLockfile, CpmError> {
    let repo_path = canonical_repo_root(repo_root)?;
    let repo_assets: Vec<_> = lockfile
        .all_assets()
        .filter(|asset| asset.scope == Scope::Global)
        .cloned()
        .collect();
    let repo_keys: HashSet<_> = repo_assets
        .iter()
        .map(|asset| (asset.name.clone(), asset.kind))
        .collect();

    let mut claims: Vec<_> = global_lockfile
        .claims
        .iter()
        .filter(|claim| {
            claim.claimed_by != repo_path
                && !repo_keys.contains(&(claim.asset.name.clone(), claim.asset.kind))
        })
        .cloned()
        .collect();

    for asset in repo_assets {
        claims.push(GlobalClaim::new(repo_path.clone(), asset));
    }

    claims.sort_by(|left, right| {
        left.asset
            .kind
            .to_string()
            .cmp(&right.asset.kind.to_string())
            .then_with(|| left.asset.name.cmp(&right.asset.name))
            .then_with(|| left.claimed_by.as_str().cmp(right.claimed_by.as_str()))
    });

    let generated = if claims == global_lockfile.claims {
        global_lockfile.generated
    } else {
        Utc::now()
    };

    Ok(GlobalLockfile {
        version: global_lockfile.version,
        generated,
        claims,
    })
}

/// Lightweight resolution result — the pinned rev + date for a single asset.
#[derive(Debug, Clone)]
pub struct ResolvedRef {
    /// The 40-character git SHA (or npm version string for npm packages).
    pub rev: String,
    /// UTC timestamp of resolution.
    pub date: chrono::DateTime<chrono::Utc>,
}

/// Resolve all assets in `manifest` using the provided HTTP client.
///
/// Returns a list of [`ResolvedRef`] entries in the same order as the manifest
/// iterates (plugins → skills → agents → mcps → hooks → workflows).
///
/// # Errors
/// - [`CpmError::Network`] on HTTP failures.
/// - [`CpmError::AuthRequired`] for 401/403 responses on private repos.
/// - [`CpmError::UnsupportedUrl`] for unrecognised URL schemes.
pub async fn resolve_manifest(
    manifest: &Manifest,
    client: &reqwest::Client,
    token: Option<&str>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<Vec<ResolvedRef>, CpmError> {
    let mut results = Vec::new();
    let plugins = manifest.effective_section(AssetKind::Plugin);
    let skills = manifest.effective_section(AssetKind::Skill);
    let agents = manifest.effective_section(AssetKind::Agent);
    let mcps = manifest.effective_section(AssetKind::Mcp);
    let hooks = manifest.effective_section(AssetKind::Hook);
    let workflows = manifest.effective_section(AssetKind::Workflow);

    for (_name, source) in &plugins {
        debug!("resolving plugin {_name}");
        results.push(resolve_source(source, client, token, source_rules).await?);
    }
    for (_name, source) in &skills {
        results.push(resolve_source(source, client, token, source_rules).await?);
    }
    for (_name, source) in &agents {
        results.push(resolve_source(source, client, token, source_rules).await?);
    }
    for (_name, source) in &mcps {
        results.push(resolve_source(source, client, token, source_rules).await?);
    }
    for (_name, source) in &hooks {
        results.push(resolve_source(source, client, token, source_rules).await?);
    }
    for (_name, source) in &workflows {
        results.push(resolve_source(source, client, token, source_rules).await?);
    }

    info!("resolved {} asset refs", results.len());
    Ok(results)
}

async fn resolve_source(
    source: &cpm_types::AssetSource,
    client: &reqwest::Client,
    token: Option<&str>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<ResolvedRef, CpmError> {
    let rev = resolve_pinned_rev(
        client,
        token,
        source.url.as_deref(),
        source.rev.as_deref(),
        source_rules,
    )
    .await?
    .or_else(|| source.rev.clone())
    .unwrap_or_default();

    Ok(ResolvedRef {
        rev,
        date: chrono::Utc::now(),
    })
}

/// Ensure the lockfile is consistent with the current manifest.
///
/// Returns [`CpmError::LockOutOfDate`] if any manifest entry is missing from
/// the lockfile or has a different URL.
pub fn check_lock_freshness(manifest: &Manifest, lockfile: &Lockfile) -> Result<(), CpmError> {
    check_section(
        &manifest.effective_section(AssetKind::Plugin),
        &lockfile.plugins,
    )?;
    check_section(
        &manifest.effective_section(AssetKind::Skill),
        &lockfile.skills,
    )?;
    check_section(
        &manifest.effective_section(AssetKind::Agent),
        &lockfile.agents,
    )?;
    check_section(&manifest.effective_section(AssetKind::Mcp), &lockfile.mcps)?;
    check_section(
        &manifest.effective_section(AssetKind::Hook),
        &lockfile.hooks,
    )?;
    check_section(
        &manifest.effective_section(AssetKind::Workflow),
        &lockfile.workflows,
    )?;

    Ok(())
}

fn check_section(
    manifest: &indexmap::IndexMap<String, cpm_types::AssetSource>,
    locked: &[cpm_types::ResolvedAsset],
) -> Result<(), CpmError> {
    if manifest.len() != locked.len() {
        return Err(CpmError::LockOutOfDate);
    }

    for (name, source) in manifest {
        let Some(asset) = locked.iter().find(|asset| asset.name == *name) else {
            return Err(CpmError::LockOutOfDate);
        };
        if asset.source.url != source.url
            || asset.source.path != source.path
            || asset.source.group != source.group
            || asset.source.scope != source.scope
            || asset.source.transport != source.transport
            || asset.source.env != source.env
            || asset.source.args != source.args
        {
            return Err(CpmError::LockOutOfDate);
        }
        if let Some(requested_rev) = &source.rev {
            if !asset.resolved_rev.starts_with(requested_rev)
                && asset.resolved_rev != *requested_rev
            {
                return Err(CpmError::LockOutOfDate);
            }
        }
    }

    Ok(())
}

/// Canonicalize `repo_root` to an absolute, symlink-resolved UTF-8 path.
///
/// This is the path used to record and compare `claimed_by` values in the
/// machine-local global lockfile.  All code that reads or writes claim
/// ownership (sync, add, reset) must use this helper so that symlink variants
/// and relative-path representations of the same directory compare equal.
pub fn canonical_repo_root(repo_root: &Path) -> Result<Utf8PathBuf, CpmError> {
    let canonical = repo_root.canonicalize()?;
    Utf8PathBuf::from_path_buf(canonical).map_err(|path| CpmError::InvalidConfig {
        key: "repo_root".to_owned(),
        reason: format!("repository path must be valid UTF-8: {}", path.display()),
    })
}

fn find_claim<'a>(
    global_lockfile: &'a GlobalLockfile,
    asset: &ResolvedAsset,
) -> Option<&'a GlobalClaim> {
    global_lockfile
        .claims
        .iter()
        .find(|claim| claim.asset.name == asset.name && claim.asset.kind == asset.kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpm_types::{AssetSource, ResolvedAsset, Scope};
    use tempfile::TempDir;

    fn make_resolved(name: &str, kind: AssetKind, scope: Scope) -> ResolvedAsset {
        ResolvedAsset {
            name: name.to_owned(),
            kind,
            source: AssetSource {
                url: Some("https://example.com".into()),
                rev: None,
                path: None,
                group: "default".into(),
                scope,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "a".repeat(40),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:abc".into(),
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

    #[test]
    fn no_conflict_when_same_name_different_kind() {
        let mut lf = Lockfile::new();
        lf.plugins
            .push(make_resolved("foo", AssetKind::Plugin, Scope::Local));
        lf.mcps
            .push(make_resolved("foo", AssetKind::Mcp, Scope::Global));
        assert!(detect_conflicts(&lf).is_ok());
    }

    #[test]
    fn conflict_detected_same_name_same_kind() {
        let mut lf = Lockfile::new();
        lf.mcps
            .push(make_resolved("bar", AssetKind::Mcp, Scope::Local));
        lf.mcps
            .push(make_resolved("bar", AssetKind::Mcp, Scope::Global));
        let err = detect_conflicts(&lf).unwrap_err();
        assert!(matches!(err, CpmError::ScopeConflict { .. }));
    }

    #[test]
    fn lock_out_of_date_when_manifest_has_extra_entry() {
        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "new-plugin".into(),
            AssetSource {
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
        );
        let lockfile = Lockfile::new();
        let err = check_lock_freshness(&manifest, &lockfile).unwrap_err();
        assert!(matches!(err, CpmError::LockOutOfDate));
    }

    #[test]
    fn lock_fresh_when_manifest_empty() {
        let manifest = Manifest::default();
        let lockfile = Lockfile::new();
        assert!(check_lock_freshness(&manifest, &lockfile).is_ok());
    }

    #[test]
    fn detects_cross_repo_global_install_conflict() {
        let dir = TempDir::new().expect("tempdir");
        let mut repo_lock = Lockfile::new();
        let mut requested = make_resolved("shared", AssetKind::Plugin, Scope::Global);
        requested.resolved_rev = "b".repeat(40);
        repo_lock.plugins.push(requested);

        let mut global_lock = GlobalLockfile::new();
        global_lock.claims.push(GlobalClaim::new(
            Utf8PathBuf::from("/tmp/other-repo"),
            make_resolved("shared", AssetKind::Plugin, Scope::Global),
        ));

        let err = detect_global_install_conflicts(repo_lock.all_assets(), &global_lock, dir.path())
            .unwrap_err();
        assert!(matches!(err, CpmError::GlobalInstallConflict { .. }));
    }

    #[test]
    fn reconcile_global_lockfile_transfers_matching_claim_to_current_repo() {
        let dir = TempDir::new().expect("tempdir");
        let repo_path = Utf8PathBuf::from_path_buf(dir.path().canonicalize().expect("canonical"))
            .expect("utf8 repo path");
        let mut repo_lock = Lockfile::new();
        repo_lock
            .plugins
            .push(make_resolved("shared", AssetKind::Plugin, Scope::Global));

        let mut global_lock = GlobalLockfile::new();
        global_lock.claims.push(GlobalClaim::new(
            Utf8PathBuf::from("/tmp/other-repo"),
            make_resolved("shared", AssetKind::Plugin, Scope::Global),
        ));
        global_lock.claims.push(GlobalClaim::new(
            repo_path.clone(),
            make_resolved("stale", AssetKind::Skill, Scope::Global),
        ));

        let reconciled =
            reconcile_global_lockfile(&repo_lock, &global_lock, dir.path()).expect("reconcile");

        assert_eq!(reconciled.claims.len(), 1);
        assert_eq!(reconciled.claims[0].claimed_by, repo_path);
        assert_eq!(reconciled.claims[0].asset.name, "shared");
    }

    #[test]
    fn canonical_repo_root_resolves_symlinks() {
        // On non-unix platforms there are no symlinks, skip.
        #[cfg(not(unix))]
        return;

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let real_dir = TempDir::new().expect("real tempdir");
            let link_parent = TempDir::new().expect("link parent tempdir");
            let link_path = link_parent.path().join("link");
            symlink(real_dir.path(), &link_path).expect("create symlink");

            let via_real = canonical_repo_root(real_dir.path()).expect("canonical via real");
            let via_link = canonical_repo_root(&link_path).expect("canonical via link");

            assert_eq!(
                via_real, via_link,
                "canonical_repo_root must resolve symlinks to the same path"
            );
        }
    }
}
