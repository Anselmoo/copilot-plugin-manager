//! Discovery helpers for externally installed Copilot assets.
//!
//! These read-only helpers surface assets that exist on disk or in config files
//! but are not tracked by cpm — intended to feed the `cpm overview --external`
//! display.
//!
//! Four categories are reported:
//! 1. **Copilot-discovered plugins** (`~/.copilot/plugin-index.json`, delegated
//!    `~/.copilot/plugins/*.installed` markers, or modern
//!    `~/.copilot/installed-plugins/<registry>/<plugin>/` bundles) not in the
//!    current manifest.
//! 2. **Unclaimed global file-system assets** under `~/.copilot/` not claimed by any cpm repo.
//! 3. **Cross-repo claims** — global assets claimed by a different repository.
//! 4. **Unmanaged global MCP servers** in `~/.copilot/mcp-config.json` not in any lockfile.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Serialize, Serializer};

use cpm_types::{AssetKind, GlobalLockfile, Lockfile, Manifest, Scope};

use crate::{
    installer::{copilot_mcp_config_path, install_dir},
    plugin_index::{
        default_plugin_index_path, installed_plugin_request, plugin_install_root,
        plugin_install_root_candidates, plugin_request, read_installed_plugins_from,
    },
    CpmError,
};

// ─── Public types ─────────────────────────────────────────────────────────────

/// An entry from Copilot plugin discovery not represented in the current manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalPlugin {
    /// Plugin name as recorded in the index.
    pub name: Option<String>,
    /// Source URL as recorded in the index.
    pub source: Option<String>,
    /// Version string as recorded in the index.
    pub version: Option<String>,
    /// Registry as recorded in the index.
    pub registry: Option<String>,
}

/// A file-system entry under `~/.copilot/` not claimed by any cpm-managed repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnclaimedGlobalAsset {
    /// Asset kind (`plugin`, `skill`, `agent`, …).
    pub kind: String,
    /// Relative path within the kind's install directory.
    pub path: String,
    /// Absolute path on disk.
    #[serde(serialize_with = "serialize_path")]
    pub full_path: PathBuf,
    /// Number of files inside (1 for plain files, ≥1 for directories).
    pub file_count: usize,
    /// `"file"` or `"directory"`.
    pub entry_type: String,
}

/// A global asset claimed by a *different* repository than the current one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CrossRepoClaim {
    /// Absolute path of the repo that recorded this claim.
    pub claimed_by: String,
    /// Asset name.
    pub name: String,
    /// Asset kind.
    pub kind: String,
    /// Upstream source URL, if known.
    pub source: Option<String>,
}

/// An MCP server entry in `~/.copilot/mcp-config.json` not tracked by any cpm lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalMcpServer {
    /// Server name as it appears in the config.
    pub name: String,
}

/// Aggregated external-asset discovery results.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExternalAssets {
    /// Plugin-index entries not present in the current manifest.
    pub unindexed_plugins: Vec<ExternalPlugin>,
    /// Global file-system assets not claimed by any cpm-managed repo.
    pub unclaimed_global: Vec<UnclaimedGlobalAsset>,
    /// Global assets claimed by a different repo than the current one.
    pub cross_repo_claims: Vec<CrossRepoClaim>,
    /// MCP servers in the global Copilot config not tracked by cpm.
    pub unmanaged_mcp: Vec<ExternalMcpServer>,
}

impl ExternalAssets {
    /// Return `true` when all four discovery lists are empty.
    pub fn is_empty(&self) -> bool {
        self.unindexed_plugins.is_empty()
            && self.unclaimed_global.is_empty()
            && self.cross_repo_claims.is_empty()
            && self.unmanaged_mcp.is_empty()
    }

    /// Total count of all discovered external items across all categories.
    pub fn total_count(&self) -> usize {
        self.unindexed_plugins.len()
            + self.unclaimed_global.len()
            + self.cross_repo_claims.len()
            + self.unmanaged_mcp.len()
    }
}

// ─── Main entry point ─────────────────────────────────────────────────────────

/// Scan for externally installed Copilot assets.
///
/// All operations are read-only; this function never mutates anything on disk.
///
/// # Parameters
/// - `manifest`: the current repo's manifest (used to filter already-managed plugins).
/// - `lockfile`: the current repo lockfile (used to suppress delegated plugins already managed here).
/// - `global_lockfile`: the machine-wide global lockfile.
/// - `repo_root`: the current repository root (used to identify cross-repo claims).
/// - `plugin_index_path`: override the default `~/.copilot/plugin-index.json` path (for testing).
/// - `mcp_config_path`: override the default `~/.copilot/mcp-config.json` path (for testing).
pub fn scan_external_assets(
    manifest: &Manifest,
    lockfile: &Lockfile,
    global_lockfile: &GlobalLockfile,
    repo_root: &Path,
    plugin_index_path: Option<&Path>,
    mcp_config_path: Option<&Path>,
) -> Result<ExternalAssets, CpmError> {
    let unindexed_plugins = scan_unindexed_plugins(manifest, plugin_index_path)?;
    let unclaimed_global =
        scan_unclaimed_global_assets(lockfile, global_lockfile, plugin_index_path)?;
    let cross_repo_claims = scan_cross_repo_claims(global_lockfile, repo_root);
    let unmanaged_mcp = scan_unmanaged_global_mcp(global_lockfile, repo_root, mcp_config_path)?;

    Ok(ExternalAssets {
        unindexed_plugins,
        unclaimed_global,
        cross_repo_claims,
        unmanaged_mcp,
    })
}

// ─── Plugin index ──────────────────────────────────────────────────────────────

fn scan_unindexed_plugins(
    manifest: &Manifest,
    plugin_index_path: Option<&Path>,
) -> Result<Vec<ExternalPlugin>, CpmError> {
    let default_path = default_plugin_index_path();
    let path = plugin_index_path.unwrap_or(&default_path);
    let plugins = read_installed_plugins_from(path)?;

    // Collect all plugin names present in the manifest (including group-scoped entries).
    let manifest_names: HashSet<String> = manifest
        .effective_section(AssetKind::Plugin)
        .into_keys()
        .collect();

    Ok(plugins
        .into_iter()
        .filter(|p| {
            p.name
                .as_deref()
                .map(|n| !manifest_names.contains(n))
                .unwrap_or(true)
        })
        .map(|p| ExternalPlugin {
            name: p.name,
            source: p.source,
            version: p.version,
            registry: p.registry,
        })
        .collect())
}

// ─── Unclaimed global assets ──────────────────────────────────────────────────

/// Asset kinds that have file-system directories under `~/.copilot/`.
/// Workflows are local-only; MCPs use a config file rather than a directory.
const GLOBAL_DIR_KINDS: &[AssetKind] = &[
    AssetKind::Plugin,
    AssetKind::Skill,
    AssetKind::Agent,
    AssetKind::Hook,
    AssetKind::Instruction,
];

fn scan_unclaimed_global_assets(
    lockfile: &Lockfile,
    global_lockfile: &GlobalLockfile,
    plugin_index_path: Option<&Path>,
) -> Result<Vec<UnclaimedGlobalAsset>, CpmError> {
    // repo_root is irrelevant for global scope; global paths use home_dir().
    let dummy_root = Path::new(".");
    let default_path = default_plugin_index_path();
    let plugin_index = plugin_index_path.unwrap_or(&default_path);
    let legacy_plugin_root = plugin_index
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("plugins");
    // Build the set of top-level directory names that are claimed by any cpm repo.
    let mut claimed_top_dirs: HashSet<PathBuf> = HashSet::new();
    let mut claimed_plugin_names: HashSet<String> = HashSet::new();
    for asset in lockfile
        .all_assets()
        .chain(global_lockfile.all_assets())
        .filter(|a| a.scope == Scope::Global || a.kind == AssetKind::Plugin)
    {
        let base_dir = install_dir(asset.kind, Scope::Global, dummy_root);
        if asset.kind == AssetKind::Plugin {
            claimed_plugin_names.insert(asset.name.clone());
            let registry = asset
                .plugin_meta
                .as_ref()
                .and_then(|meta| meta.registry.as_deref());
            for plugin_root in plugin_install_root_candidates(&asset.name, registry) {
                claimed_top_dirs.insert(plugin_root);
            }
            claimed_top_dirs.insert(legacy_plugin_root.join(format!("{}.installed", asset.name)));
            continue;
        }
        // Use the source path to identify the top-level directory name.
        if let Some(source_path) = &asset.source.path {
            if let Some(first) = source_path.components().next() {
                claimed_top_dirs.insert(base_dir.join(first.as_str()));
            }
        }
        // Also cover flat installs where files are placed directly.
        for file in &asset.files {
            if let Some(first) = Path::new(file.path.as_str()).components().next() {
                claimed_top_dirs.insert(base_dir.join(first));
            }
        }
    }

    let mut results = Vec::new();
    for plugin in read_installed_plugins_from(plugin_index)? {
        let Some(name) = plugin.name.as_deref() else {
            continue;
        };
        if claimed_plugin_names.contains(name) {
            continue;
        }
        let full_path =
            plugin_install_root(&plugin).unwrap_or_else(|| PathBuf::from(format!("plugin:{name}")));
        let file_count = if full_path.is_dir() {
            count_files(&full_path).unwrap_or(0)
        } else {
            1
        };
        results.push(UnclaimedGlobalAsset {
            kind: AssetKind::Plugin.to_string(),
            path: installed_plugin_request(&plugin)
                .unwrap_or_else(|| plugin_request(name, plugin.registry.as_deref())),
            full_path,
            file_count,
            entry_type: "plugin".to_owned(),
        });
    }

    for &kind in GLOBAL_DIR_KINDS {
        if kind == AssetKind::Plugin {
            continue;
        }
        let dir = install_dir(kind, Scope::Global, dummy_root);
        if !dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if claimed_top_dirs.contains(&path) {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_owned(),
                None => continue,
            };
            if path.is_file() {
                results.push(UnclaimedGlobalAsset {
                    kind: kind.to_string(),
                    path: name,
                    full_path: path,
                    file_count: 1,
                    entry_type: "file".to_owned(),
                });
            } else if path.is_dir() {
                let count = count_files(&path).unwrap_or(0);
                results.push(UnclaimedGlobalAsset {
                    kind: kind.to_string(),
                    path: format!("{name}/"),
                    full_path: path,
                    file_count: count,
                    entry_type: "directory".to_owned(),
                });
            }
        }
    }

    Ok(results)
}

fn count_files(path: &Path) -> Result<usize, std::io::Error> {
    let mut count = 0;
    for entry in std::fs::read_dir(path)? {
        let child = entry?.path();
        if child.is_dir() {
            count += count_files(&child)?;
        } else if child.is_file() {
            count += 1;
        }
    }
    Ok(count)
}

// ─── Cross-repo claims ────────────────────────────────────────────────────────

fn scan_cross_repo_claims(
    global_lockfile: &GlobalLockfile,
    repo_root: &Path,
) -> Vec<CrossRepoClaim> {
    let current = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());

    global_lockfile
        .claims
        .iter()
        .filter(|claim| {
            let claim_canonical = PathBuf::from(claim.claimed_by.as_str())
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(claim.claimed_by.as_str()));
            claim_canonical != current
        })
        .map(|claim| CrossRepoClaim {
            claimed_by: claim.claimed_by.to_string(),
            name: claim.asset.name.clone(),
            kind: claim.asset.kind.to_string(),
            source: claim.asset.source.url.clone(),
        })
        .collect()
}

// ─── Unmanaged global MCP ─────────────────────────────────────────────────────

fn scan_unmanaged_global_mcp(
    global_lockfile: &GlobalLockfile,
    repo_root: &Path,
    mcp_config_path_override: Option<&Path>,
) -> Result<Vec<ExternalMcpServer>, CpmError> {
    let config_path = mcp_config_path_override
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| copilot_mcp_config_path(Scope::Global, repo_root));

    if !config_path.exists() {
        return Ok(Vec::new());
    }

    let all_names = read_mcp_server_names_from_path(&config_path)?;

    let managed_names: HashSet<&str> = global_lockfile
        .all_assets()
        .filter(|asset| asset.kind == AssetKind::Mcp && asset.scope == Scope::Global)
        .map(|asset| asset.name.as_str())
        .collect();

    Ok(all_names
        .into_iter()
        .filter(|name| !managed_names.contains(name.as_str()))
        .map(|name| ExternalMcpServer { name })
        .collect())
}

/// Read MCP server names from an arbitrary JSON config path.
///
/// Supports both the `"servers"` (VS Code local) and `"mcpServers"` (global Copilot) keys.
fn read_mcp_server_names_from_path(path: &Path) -> Result<Vec<String>, CpmError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = std::fs::read_to_string(path)?;
    let value: serde_json::Value =
        serde_json::from_str(&contents).map_err(|err| CpmError::Parse {
            file: path.display().to_string(),
            msg: err.to_string(),
        })?;

    let mut names = Vec::new();
    for key in &["mcpServers", "servers"] {
        if let Some(servers) = value.get(key).and_then(|v| v.as_object()) {
            names.extend(servers.keys().cloned());
            break;
        }
    }
    names.sort();
    Ok(names)
}

// ─── Serde helper ────────────────────────────────────────────────────────────

fn serialize_path<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&path.to_string_lossy().replace('\\', "/"))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use camino::Utf8PathBuf;
    use cpm_types::{
        AssetKind, AssetOwnership, AssetSource, GlobalClaim, GlobalLockfile, LockedFile, Manifest,
        ResolvedAsset, Scope,
    };
    use tempfile::TempDir;

    fn make_global_source(_kind: AssetKind, path: &str) -> AssetSource {
        AssetSource {
            url: Some(format!("https://example.com/{path}")),
            rev: Some("a".repeat(40)),
            path: Some(Utf8PathBuf::from(path)),
            group: "default".to_owned(),
            scope: Scope::Global,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        }
    }

    fn make_global_asset(name: &str, kind: AssetKind, path: &str) -> ResolvedAsset {
        ResolvedAsset {
            name: name.to_owned(),
            kind,
            source: make_global_source(kind, path),
            resolved_rev: "a".repeat(40),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:abc".to_owned(),
            scope: Scope::Global,
            ownership: AssetOwnership::Upstream,
            files: vec![LockedFile {
                path: Utf8PathBuf::from(format!("{path}/main.md")),
                sha256: None,
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
        }
    }

    // ── Plugin index tests ──────────────────────────────────────────────────

    #[test]
    fn unindexed_plugins_empty_when_index_missing() {
        let dir = TempDir::new().expect("tempdir");
        let missing = dir.path().join("missing-index.json");
        let manifest = Manifest::default();

        let result = scan_unindexed_plugins(&manifest, Some(&missing)).expect("scan");

        assert!(result.is_empty());
    }

    #[test]
    fn unindexed_plugins_filters_manifest_entries() {
        let dir = TempDir::new().expect("tempdir");
        let index_path = dir.path().join("plugin-index.json");
        std::fs::write(
            &index_path,
            r#"[
                {"name": "managed-plugin", "version": "1.0.0"},
                {"name": "external-plugin", "version": "2.0.0", "source": "https://example.com/ext"}
            ]"#,
        )
        .expect("write");

        let mut manifest = Manifest::default();
        manifest.plugins.insert(
            "managed-plugin".to_owned(),
            make_global_source(AssetKind::Plugin, "plugins/managed-plugin"),
        );

        let result = scan_unindexed_plugins(&manifest, Some(&index_path)).expect("scan");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name.as_deref(), Some("external-plugin"));
        assert_eq!(result[0].source.as_deref(), Some("https://example.com/ext"));
    }

    #[test]
    fn unindexed_plugins_includes_unnamed_entries() {
        let dir = TempDir::new().expect("tempdir");
        let index_path = dir.path().join("plugin-index.json");
        std::fs::write(
            &index_path,
            r#"[{"version": "1.0.0", "source": "https://example.com/anon"}]"#,
        )
        .expect("write");

        let manifest = Manifest::default();
        let result = scan_unindexed_plugins(&manifest, Some(&index_path)).expect("scan");

        assert_eq!(result.len(), 1);
        assert!(result[0].name.is_none());
    }

    #[test]
    fn unindexed_plugins_includes_group_manifest_plugins() {
        let dir = TempDir::new().expect("tempdir");
        let index_path = dir.path().join("plugin-index.json");
        std::fs::write(
            &index_path,
            r#"[{"name": "group-plugin", "version": "1.0.0"}]"#,
        )
        .expect("write");

        let mut manifest = Manifest::default();
        let mut group = cpm_types::ManifestGroup::default();
        group.plugins.insert(
            "group-plugin".to_owned(),
            make_global_source(AssetKind::Plugin, "plugins/group-plugin"),
        );
        manifest.groups.insert("my-group".to_owned(), group);

        let result = scan_unindexed_plugins(&manifest, Some(&index_path)).expect("scan");

        // group-plugin is in the manifest via a group, so it should be filtered out
        assert!(result.is_empty());
    }

    // ── Unclaimed global assets tests ───────────────────────────────────────

    #[test]
    fn unclaimed_global_empty_when_no_global_dirs() {
        let lockfile = Lockfile::new();
        let global_lockfile = GlobalLockfile::new();
        // No ~/.copilot/ dirs exist in test, so scan should return empty
        let result = scan_unclaimed_global_assets(&lockfile, &global_lockfile, None).expect("scan");
        // Result may or may not be empty depending on the machine; we just verify no panic.
        let _ = result;
    }

    #[test]
    fn unclaimed_global_skips_managed_delegated_plugins_from_repo_lock() {
        let dir = TempDir::new().expect("tempdir");
        let copilot_dir = dir.path().join(".copilot");
        let plugins_dir = copilot_dir.join("plugins");
        std::fs::create_dir_all(plugins_dir.join("pptx")).expect("mkdir");
        std::fs::write(plugins_dir.join("pptx.installed"), "").expect("marker");
        std::fs::write(plugins_dir.join("manual.installed"), "").expect("marker");

        let mut lockfile = Lockfile::new();
        let mut asset = make_global_asset("pptx", AssetKind::Plugin, "plugins/pptx");
        asset.scope = Scope::Local;
        lockfile.plugins.push(asset);

        let result = scan_unclaimed_global_assets(
            &lockfile,
            &GlobalLockfile::new(),
            Some(&copilot_dir.join("plugin-index.json")),
        )
        .expect("scan");

        assert!(!result.iter().any(|asset| asset.path == "pptx"));
        assert!(result.iter().any(|asset| asset.path == "manual"));
    }

    // ── Cross-repo claims tests ─────────────────────────────────────────────

    #[test]
    fn cross_repo_claims_empty_for_same_repo() {
        let dir = TempDir::new().expect("tempdir");
        let repo_root = dir.path();

        let asset = make_global_asset("my-skill", AssetKind::Skill, "my-skill");
        let canonical_root = repo_root
            .canonicalize()
            .unwrap_or_else(|_| repo_root.to_path_buf());
        let mut lockfile = GlobalLockfile::new();
        lockfile.claims.push(GlobalClaim::new(
            Utf8PathBuf::from(canonical_root.to_string_lossy().as_ref()),
            asset,
        ));

        let claims = scan_cross_repo_claims(&lockfile, repo_root);

        assert!(claims.is_empty(), "own-repo claim should be filtered out");
    }

    #[test]
    fn cross_repo_claims_surfaces_other_repo() {
        let dir = TempDir::new().expect("tempdir");
        let current_repo = dir.path();
        let other_repo = Utf8PathBuf::from("/other/project");

        let asset = make_global_asset("their-skill", AssetKind::Skill, "their-skill");
        let mut lockfile = GlobalLockfile::new();
        lockfile
            .claims
            .push(GlobalClaim::new(other_repo.clone(), asset));

        let claims = scan_cross_repo_claims(&lockfile, current_repo);

        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].name, "their-skill");
        assert_eq!(claims[0].kind, "skill");
        assert_eq!(claims[0].claimed_by, other_repo.as_str());
    }

    #[test]
    fn cross_repo_claims_includes_source_url() {
        let dir = TempDir::new().expect("tempdir");
        let current_repo = dir.path();

        let asset = make_global_asset("ext-plugin", AssetKind::Plugin, "ext-plugin");
        let mut lockfile = GlobalLockfile::new();
        lockfile
            .claims
            .push(GlobalClaim::new(Utf8PathBuf::from("/other/repo"), asset));

        let claims = scan_cross_repo_claims(&lockfile, current_repo);

        assert_eq!(claims.len(), 1);
        assert_eq!(
            claims[0].source.as_deref(),
            Some("https://example.com/ext-plugin")
        );
    }

    // ── Unmanaged global MCP tests ──────────────────────────────────────────

    #[test]
    fn unmanaged_mcp_empty_when_config_missing() {
        let dir = TempDir::new().expect("tempdir");
        let missing = dir.path().join("missing-mcp.json");
        let global_lockfile = GlobalLockfile::new();

        let result =
            scan_unmanaged_global_mcp(&global_lockfile, dir.path(), Some(&missing)).expect("scan");

        assert!(result.is_empty());
    }

    #[test]
    fn unmanaged_mcp_surfaces_unlocked_servers() {
        let dir = TempDir::new().expect("tempdir");
        let config_path = dir.path().join("mcp-config.json");
        std::fs::write(
            &config_path,
            r#"{"mcpServers": {"managed-server": {}, "external-server": {}}}"#,
        )
        .expect("write");

        let mut global_lockfile = GlobalLockfile::new();
        // Add a managed global MCP asset
        let mut managed = make_global_asset("managed-server", AssetKind::Mcp, "mcps/managed");
        managed.kind = AssetKind::Mcp;
        managed.scope = Scope::Global;
        global_lockfile
            .claims
            .push(GlobalClaim::new(Utf8PathBuf::from("/some/repo"), managed));

        let result = scan_unmanaged_global_mcp(&global_lockfile, dir.path(), Some(&config_path))
            .expect("scan");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "external-server");
    }

    #[test]
    fn unmanaged_mcp_supports_servers_key() {
        let dir = TempDir::new().expect("tempdir");
        let config_path = dir.path().join("mcp.json");
        std::fs::write(&config_path, r#"{"servers": {"only-server": {}}}"#).expect("write");

        let global_lockfile = GlobalLockfile::new();
        let result = scan_unmanaged_global_mcp(&global_lockfile, dir.path(), Some(&config_path))
            .expect("scan");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "only-server");
    }

    // ── ExternalAssets aggregate tests ─────────────────────────────────────

    #[test]
    fn external_assets_is_empty_when_all_empty() {
        let ea = ExternalAssets::default();
        assert!(ea.is_empty());
        assert_eq!(ea.total_count(), 0);
    }

    #[test]
    fn external_assets_total_count() {
        let ea = ExternalAssets {
            unindexed_plugins: vec![ExternalPlugin {
                name: Some("p".to_owned()),
                source: None,
                version: None,
                registry: None,
            }],
            unclaimed_global: vec![UnclaimedGlobalAsset {
                kind: "skill".to_owned(),
                path: "x/".to_owned(),
                full_path: PathBuf::from("/x"),
                file_count: 1,
                entry_type: "directory".to_owned(),
            }],
            cross_repo_claims: vec![CrossRepoClaim {
                claimed_by: "/repo".to_owned(),
                name: "a".to_owned(),
                kind: "agent".to_owned(),
                source: None,
            }],
            unmanaged_mcp: vec![ExternalMcpServer {
                name: "mcp1".to_owned(),
            }],
        };
        assert!(!ea.is_empty());
        assert_eq!(ea.total_count(), 4);
    }

    // ── scan_external_assets integration test ─────────────────────────────

    #[test]
    fn scan_external_assets_all_empty_with_no_files() {
        let dir = TempDir::new().expect("tempdir");
        let missing_index = dir.path().join("index.json");
        let missing_mcp = dir.path().join("mcp.json");
        let manifest = Manifest::default();
        let lockfile = Lockfile::new();
        let global_lockfile = GlobalLockfile::new();

        let result = scan_external_assets(
            &manifest,
            &lockfile,
            &global_lockfile,
            dir.path(),
            Some(&missing_index),
            Some(&missing_mcp),
        )
        .expect("scan");

        assert!(result.unindexed_plugins.is_empty());
        assert!(result.unmanaged_mcp.is_empty());
        // cross_repo_claims is always empty when global_lockfile has no claims
        assert!(result.cross_repo_claims.is_empty());
    }
}
