//! Read-only helpers for Copilot plugin discovery.
//!
//! The primary source is `~/.copilot/plugin-index.json`, owned by the Copilot
//! CLI. When that file is absent or incomplete, discovery falls back to older
//! delegated plugin markers under `~/.copilot/plugins/*.installed` and to the
//! current Copilot install layout under
//! `~/.copilot/installed-plugins/<registry>/<plugin>/`. cpm only reads these
//! locations to observe installed plugin state; this module never writes them.

use std::path::{Path, PathBuf};

use camino::Utf8PathBuf;
use indexmap::IndexMap;
use serde::Deserialize;

use crate::fetcher::sha256_file;
use crate::paths::{copilot_home_dir, copilot_state_dir};
use crate::CpmError;

/// A plugin entry read from the Copilot plugin index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPlugin {
    /// Plugin name.
    pub name: Option<String>,
    /// Installed version string, when present in the index.
    pub version: Option<String>,
    /// Resolved revision or commit SHA, when present.
    pub revision: Option<String>,
    /// Source URL or registry location, when present.
    pub source: Option<String>,
    /// Plugin registry name, when present.
    pub registry: Option<String>,
    /// Human-readable description, when present.
    pub description: Option<String>,
    /// Installation path, when present.
    pub path: Option<Utf8PathBuf>,
    /// Whether the plugin is enabled, when present.
    pub enabled: Option<bool>,
    /// Raw timestamp value, when present.
    pub installed_at: Option<String>,
    /// Extra fields preserved from the source document.
    pub extra: IndexMap<String, serde_json::Value>,
}

/// Return the default plugin-index path (`~/.copilot/plugin-index.json`).
pub fn default_plugin_index_path() -> PathBuf {
    copilot_state_dir().join("plugin-index.json")
}

/// Return the default Copilot plugin install directory (`~/.copilot/plugins`).
pub fn default_plugin_install_dir() -> PathBuf {
    default_plugin_index_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("plugins")
}

/// Return the default modern Copilot plugin install root
/// (`~/.copilot/installed-plugins`).
pub fn default_installed_plugins_dir() -> PathBuf {
    default_plugin_index_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("installed-plugins")
}

/// Read all installed plugins from the default plugin-index path.
pub fn read_installed_plugins() -> Result<Vec<InstalledPlugin>, CpmError> {
    read_installed_plugins_from(&default_plugin_index_path())
}

/// Read all installed plugins from a specific plugin-index path.
///
/// Missing files are treated as an empty collection.
pub fn read_installed_plugins_from(path: &Path) -> Result<Vec<InstalledPlugin>, CpmError> {
    let mut plugins = read_plugin_index_entries_from(path)?;
    let config_plugins = read_copilot_config_plugins_from(&config_path_for_index_path(path))?;
    merge_discovered_plugins(&mut plugins, config_plugins);
    let marker_plugins = read_plugin_markers_from(&plugin_install_dir_for_index_path(path))?;
    merge_discovered_plugins(&mut plugins, marker_plugins);
    let bundled_plugins =
        read_installed_plugin_dirs_from(&installed_plugins_dir_for_index_path(path))?;
    merge_discovered_plugins(&mut plugins, bundled_plugins);
    Ok(plugins)
}

/// Find a plugin by name in a plugin collection.
pub fn find_installed_plugin_by_name<'a>(
    plugins: &'a [InstalledPlugin],
    name: &str,
) -> Option<&'a InstalledPlugin> {
    plugins
        .iter()
        .find(|plugin| plugin.name.as_deref() == Some(name))
}

/// Resolve the plugin install root on disk when it can be inferred.
pub fn plugin_install_root(plugin: &InstalledPlugin) -> Option<PathBuf> {
    if let Some(path) = &plugin.path {
        let path = PathBuf::from(path.as_str());
        if path.is_absolute() {
            return Some(path);
        }

        let home = copilot_home_dir()?;
        return Some(home.join(path));
    }

    plugin
        .name
        .as_deref()
        .map(|name| preferred_plugin_install_root(name, plugin.registry.as_deref()))
}

/// Return the delegated plugin install root for a plugin name.
pub fn delegated_plugin_install_root_by_name(name: &str) -> PathBuf {
    default_plugin_install_dir().join(name)
}

/// Return the delegated plugin marker path for a plugin name.
pub fn delegated_plugin_marker_path_by_name(name: &str) -> PathBuf {
    default_plugin_install_dir().join(format!("{name}.installed"))
}

/// Return the installed-plugin request understood by `copilot plugin` commands.
pub fn plugin_request(name: &str, registry: Option<&str>) -> String {
    registry
        .filter(|registry| !registry.trim().is_empty())
        .map(|registry| format!("{name}@{registry}"))
        .unwrap_or_else(|| name.to_owned())
}

/// Return the installed-plugin request for a discovered plugin when it has a name.
pub fn installed_plugin_request(plugin: &InstalledPlugin) -> Option<String> {
    plugin
        .name
        .as_deref()
        .map(|name| plugin_request(name, plugin.registry.as_deref()))
}

/// Return likely install-root candidates for a plugin.
pub fn plugin_install_root_candidates(name: &str, registry: Option<&str>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(registry) = registry.filter(|registry| !registry.trim().is_empty()) {
        candidates.push(default_installed_plugins_dir().join(registry).join(name));
    } else {
        let modern_root = default_installed_plugins_dir();
        if let Ok(entries) = std::fs::read_dir(&modern_root) {
            for entry in entries.flatten() {
                let provider_dir = entry.path();
                if !provider_dir.is_dir() {
                    continue;
                }
                let candidate = provider_dir.join(name);
                if candidate.exists() && !candidates.contains(&candidate) {
                    candidates.push(candidate);
                }
            }
        }
    }

    let legacy = delegated_plugin_install_root_by_name(name);
    if !candidates.contains(&legacy) {
        candidates.push(legacy);
    }
    candidates
}

/// Return the preferred install root for a plugin based on discovery metadata.
pub fn preferred_plugin_install_root(name: &str, registry: Option<&str>) -> PathBuf {
    let candidates = plugin_install_root_candidates(name, registry);
    if let Some(existing) = candidates.iter().find(|candidate| candidate.exists()) {
        return existing.clone();
    }
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| delegated_plugin_install_root_by_name(name))
}

/// Hash the installed plugin manifest when it can be found.
pub fn hash_installed_plugin_manifest(
    plugin: &InstalledPlugin,
) -> Result<Option<String>, CpmError> {
    let Some(root) = plugin_install_root(plugin) else {
        return Ok(None);
    };

    let candidates = if root.extension().and_then(|ext| ext.to_str()) == Some("json") {
        vec![root]
    } else {
        vec![
            root.join(".github/plugin/plugin.json"),
            root.join("plugin.json"),
        ]
    };

    for candidate in candidates {
        if candidate.exists() {
            return Ok(Some(sha256_file(&candidate)?));
        }
    }

    Ok(None)
}

fn read_plugin_index_entries_from(path: &Path) -> Result<Vec<InstalledPlugin>, CpmError> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = std::fs::read_to_string(path)?;
    let value: serde_json::Value =
        serde_json::from_str(&contents).map_err(|err| CpmError::Parse {
            file: path.display().to_string(),
            msg: err.to_string(),
        })?;

    parse_plugin_index_value(value, path)
}

fn plugin_install_dir_for_index_path(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("plugins")
}

fn config_path_for_index_path(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("config.json")
}

fn installed_plugins_dir_for_index_path(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("installed-plugins")
}

fn read_plugin_markers_from(dir: &Path) -> Result<Vec<InstalledPlugin>, CpmError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut plugins = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(name) = file_name.strip_suffix(".installed") else {
            continue;
        };

        let install_root = dir.join(name);
        let path = camino::Utf8PathBuf::from_path_buf(install_root).ok();
        plugins.push(InstalledPlugin {
            name: Some(name.to_owned()),
            version: None,
            revision: None,
            source: None,
            registry: None,
            description: None,
            path,
            enabled: None,
            installed_at: None,
            extra: IndexMap::new(),
        });
    }

    plugins.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(plugins)
}

fn read_copilot_config_plugins_from(path: &Path) -> Result<Vec<InstalledPlugin>, CpmError> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = std::fs::read_to_string(path)?;
    let value: serde_json::Value =
        serde_json::from_str(&contents).map_err(|err| CpmError::Parse {
            file: path.display().to_string(),
            msg: err.to_string(),
        })?;

    match value {
        serde_json::Value::Object(mut entries) => {
            for key in ["installed_plugins", "installedPlugins"] {
                if let Some(collection) = entries.remove(key) {
                    return parse_plugin_collection(collection, path);
                }
            }
            Ok(Vec::new())
        }
        _ => Ok(Vec::new()),
    }
}

fn read_installed_plugin_dirs_from(dir: &Path) -> Result<Vec<InstalledPlugin>, CpmError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut plugins = Vec::new();
    for provider_entry in std::fs::read_dir(dir)? {
        let provider_entry = provider_entry?;
        let provider_dir = provider_entry.path();
        if !provider_dir.is_dir() {
            continue;
        }
        let Some(registry) = provider_dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        for plugin_entry in std::fs::read_dir(&provider_dir)? {
            let plugin_entry = plugin_entry?;
            let plugin_dir = plugin_entry.path();
            if !plugin_dir.is_dir() {
                continue;
            }
            let Some(name) = plugin_dir.file_name().and_then(|entry| entry.to_str()) else {
                continue;
            };

            plugins.push(InstalledPlugin {
                name: Some(name.to_owned()),
                version: None,
                revision: None,
                source: None,
                registry: Some(registry.to_owned()),
                description: None,
                path: camino::Utf8PathBuf::from_path_buf(plugin_dir).ok(),
                enabled: None,
                installed_at: None,
                extra: IndexMap::new(),
            });
        }
    }

    plugins.sort_by(|left, right| {
        installed_plugin_request(left)
            .cmp(&installed_plugin_request(right))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(plugins)
}

fn merge_discovered_plugins(plugins: &mut Vec<InstalledPlugin>, discovered: Vec<InstalledPlugin>) {
    let mut discovered = discovered;
    for plugin in plugins.iter_mut() {
        let Some(name) = plugin.name.as_deref() else {
            continue;
        };
        let Some(position) = discovered.iter().position(|candidate| {
            candidate.name.as_deref() == Some(name)
                && (plugin.registry.is_none() || candidate.registry == plugin.registry)
        }) else {
            continue;
        };
        let candidate = discovered.remove(position);
        let candidate_path = candidate.path.clone();
        let should_replace_path = plugin
            .path
            .as_ref()
            .map(|path| !Path::new(path.as_str()).exists())
            .unwrap_or(true);
        if should_replace_path && candidate_path.is_some() {
            plugin.path = candidate_path;
        }
        if plugin.registry.is_none() {
            plugin.registry = candidate.registry;
        }
    }

    plugins.extend(discovered);
}

fn parse_plugin_index_value(
    value: serde_json::Value,
    path: &Path,
) -> Result<Vec<InstalledPlugin>, CpmError> {
    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(entries) => {
            let entries: Vec<RawInstalledPlugin> =
                serde_json::from_value(serde_json::Value::Array(entries)).map_err(|err| {
                    CpmError::Parse {
                        file: path.display().to_string(),
                        msg: err.to_string(),
                    }
                })?;
            Ok(entries
                .into_iter()
                .map(|entry| entry.into_installed_plugin(None))
                .collect())
        }
        serde_json::Value::Object(mut entries) => {
            for key in ["plugins", "installed", "items", "entries"] {
                if let Some(collection) = entries.remove(key) {
                    return parse_plugin_collection(collection, path);
                }
            }

            let plugin_entries: IndexMap<String, RawInstalledPlugin> = entries
                .into_iter()
                .filter_map(|(key, value)| {
                    if value.is_object() {
                        Some((key, value))
                    } else {
                        None
                    }
                })
                .map(|(key, value)| {
                    serde_json::from_value(value)
                        .map(|entry| (key, entry))
                        .map_err(|err| CpmError::Parse {
                            file: path.display().to_string(),
                            msg: err.to_string(),
                        })
                })
                .collect::<Result<_, _>>()?;

            if plugin_entries.is_empty() {
                return Ok(Vec::new());
            }

            Ok(plugin_entries
                .into_iter()
                .map(|(name, entry)| entry.into_installed_plugin(Some(name)))
                .collect())
        }
        other => Err(CpmError::Parse {
            file: path.display().to_string(),
            msg: format!("plugin index must be a JSON array or object, got {other}"),
        }),
    }
}

fn parse_plugin_collection(
    collection: serde_json::Value,
    path: &Path,
) -> Result<Vec<InstalledPlugin>, CpmError> {
    match collection {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(entries) => {
            let entries: Vec<RawInstalledPlugin> =
                serde_json::from_value(serde_json::Value::Array(entries)).map_err(|err| {
                    CpmError::Parse {
                        file: path.display().to_string(),
                        msg: err.to_string(),
                    }
                })?;
            Ok(entries
                .into_iter()
                .map(|entry| entry.into_installed_plugin(None))
                .collect())
        }
        serde_json::Value::Object(entries) => {
            let entries: IndexMap<String, RawInstalledPlugin> =
                serde_json::from_value(serde_json::Value::Object(entries)).map_err(|err| {
                    CpmError::Parse {
                        file: path.display().to_string(),
                        msg: err.to_string(),
                    }
                })?;

            Ok(entries
                .into_iter()
                .map(|(name, entry)| entry.into_installed_plugin(Some(name)))
                .collect())
        }
        other => Err(CpmError::Parse {
            file: path.display().to_string(),
            msg: format!("plugin index collection must be an array or object, got {other}"),
        }),
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawInstalledPlugin {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default, alias = "rev", alias = "resolved_rev", alias = "resolvedRev")]
    revision: Option<String>,
    #[serde(default, alias = "source_url", alias = "url")]
    source: Option<String>,
    #[serde(default, alias = "marketplace")]
    registry: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, alias = "cache_path", alias = "cachePath")]
    path: Option<Utf8PathBuf>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default, alias = "installed_at", alias = "installedAt")]
    installed_at: Option<String>,
    #[serde(flatten, default)]
    extra: IndexMap<String, serde_json::Value>,
}

impl RawInstalledPlugin {
    fn into_installed_plugin(self, fallback_name: Option<String>) -> InstalledPlugin {
        InstalledPlugin {
            name: self.name.or(fallback_name),
            version: self.version,
            revision: self.revision,
            source: self.source,
            registry: self.registry,
            description: self.description,
            path: self.path,
            enabled: self.enabled,
            installed_at: self.installed_at,
            extra: self.extra,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_empty_collection() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join(".copilot").join("plugin-index.json");

        let plugins = read_installed_plugins_from(&path).expect("read");

        assert!(plugins.is_empty());
    }

    #[test]
    fn default_plugin_index_path_honors_home_env_override() {
        let dir = TempDir::new().expect("tempdir");
        let previous_home = std::env::var_os("HOME");
        let previous_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", dir.path());
        std::env::remove_var("USERPROFILE");

        let path = default_plugin_index_path();

        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match previous_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }

        assert_eq!(path, dir.path().join(".copilot").join("plugin-index.json"));
    }

    #[test]
    fn reads_plugins_from_document_shape() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("plugin-index.json");
        std::fs::write(
            &path,
            r#"
            {
              "generated_at": "2026-03-22T14:05:31Z",
              "plugins": [
                {
                  "name": "partners",
                  "version": "1.2.3",
                  "rev": "abcdef123456",
                  "source_url": "https://github.com/github/awesome-copilot/tree/main/plugins/partners",
                  "registry": "awesome-copilot",
                  "description": "Community plugin",
                  "path": ".copilot/plugins/partners",
                  "enabled": true,
                  "installedAt": "2026-03-22T14:05:31Z",
                  "unexpected_field": "kept"
                },
                {
                  "version": "9.9.9",
                  "source": "https://example.com/plugins/edge",
                  "extra_number": 42
                }
              ]
            }
            "#,
        )
        .expect("write");

        let plugins = read_installed_plugins_from(&path).expect("read");

        assert_eq!(plugins.len(), 2);
        let partners = find_installed_plugin_by_name(&plugins, "partners").expect("partners");
        assert_eq!(partners.version.as_deref(), Some("1.2.3"));
        assert_eq!(partners.revision.as_deref(), Some("abcdef123456"));
        assert_eq!(
            partners.source.as_deref(),
            Some("https://github.com/github/awesome-copilot/tree/main/plugins/partners")
        );
        assert_eq!(partners.registry.as_deref(), Some("awesome-copilot"));
        assert_eq!(partners.enabled, Some(true));
        assert_eq!(
            partners
                .extra
                .get("unexpected_field")
                .and_then(|v| v.as_str()),
            Some("kept")
        );

        let unnamed = plugins
            .iter()
            .find(|plugin| plugin.version.as_deref() == Some("9.9.9"))
            .expect("unnamed plugin");
        assert!(unnamed.name.is_none());
        assert_eq!(
            unnamed.source.as_deref(),
            Some("https://example.com/plugins/edge")
        );
        assert_eq!(
            unnamed.extra.get("extra_number").and_then(|v| v.as_i64()),
            Some(42)
        );
    }

    #[test]
    fn reads_plugins_from_map_shape() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("plugin-index.json");
        std::fs::write(
            &path,
            r#"
            {
              "partners": {
                "version": "1.0.0",
                "enabled": false
              },
              "cast-imaging": {
                "name": "cast-imaging",
                "registry": "awesome-copilot"
              }
            }
            "#,
        )
        .expect("write");

        let plugins = read_installed_plugins_from(&path).expect("read");

        assert_eq!(plugins.len(), 2);
        assert_eq!(
            find_installed_plugin_by_name(&plugins, "partners")
                .and_then(|plugin| plugin.version.as_deref()),
            Some("1.0.0")
        );
        assert_eq!(
            find_installed_plugin_by_name(&plugins, "cast-imaging")
                .and_then(|plugin| plugin.registry.as_deref()),
            Some("awesome-copilot")
        );
    }

    #[test]
    fn reads_plugins_from_copilot_config_when_index_missing() {
        let dir = TempDir::new().expect("tempdir");
        let copilot_dir = dir.path().join(".copilot");
        std::fs::create_dir_all(&copilot_dir).expect("mkdir");
        let cache_path = copilot_dir.join("installed-plugins/awesome-copilot/context-engineering");
        std::fs::write(
            copilot_dir.join("config.json"),
            format!(
                r#"{{
                  "installed_plugins": [
                    {{
                      "name": "context-engineering",
                      "marketplace": "awesome-copilot",
                      "version": "1.0.0",
                      "cache_path": "{}",
                      "enabled": true
                    }}
                  ]
                }}"#,
                cache_path.display()
            ),
        )
        .expect("config");

        let plugins =
            read_installed_plugins_from(&copilot_dir.join("plugin-index.json")).expect("read");

        assert_eq!(plugins.len(), 1);
        let plugin = find_installed_plugin_by_name(&plugins, "context-engineering")
            .expect("context-engineering");
        assert_eq!(plugin.registry.as_deref(), Some("awesome-copilot"));
        assert_eq!(plugin.version.as_deref(), Some("1.0.0"));
        assert_eq!(
            plugin.path.as_ref().map(|path| path.as_str()),
            Some(cache_path.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn falls_back_to_installed_markers_when_index_missing() {
        let dir = TempDir::new().expect("tempdir");
        let copilot_dir = dir.path().join(".copilot");
        let plugins_dir = copilot_dir.join("plugins");
        std::fs::create_dir_all(plugins_dir.join("pptx/.github/plugin")).expect("mkdir");
        std::fs::write(plugins_dir.join("pptx.installed"), "").expect("marker");
        std::fs::write(
            plugins_dir.join("pptx/.github/plugin/plugin.json"),
            br#"{"name":"pptx"}"#,
        )
        .expect("plugin json");

        let plugins =
            read_installed_plugins_from(&copilot_dir.join("plugin-index.json")).expect("read");

        assert_eq!(plugins.len(), 1);
        let plugin = find_installed_plugin_by_name(&plugins, "pptx").expect("plugin");
        assert_eq!(
            plugin.path.as_ref().map(|path| path.as_str()),
            Some(plugins_dir.join("pptx").to_string_lossy().as_ref())
        );
        assert!(hash_installed_plugin_manifest(plugin)
            .expect("hash")
            .unwrap_or_default()
            .starts_with("sha256:"));
    }

    #[test]
    fn discovers_modern_installed_plugin_dirs_when_index_missing() {
        let dir = TempDir::new().expect("tempdir");
        let copilot_dir = dir.path().join(".copilot");
        let plugin_dir = copilot_dir.join("installed-plugins/awesome-copilot/pptx");
        std::fs::create_dir_all(plugin_dir.join(".github/plugin")).expect("mkdir");
        std::fs::write(
            plugin_dir.join(".github/plugin/plugin.json"),
            br#"{"name":"pptx"}"#,
        )
        .expect("plugin json");

        let plugins =
            read_installed_plugins_from(&copilot_dir.join("plugin-index.json")).expect("read");

        assert_eq!(plugins.len(), 1);
        let plugin = find_installed_plugin_by_name(&plugins, "pptx").expect("plugin");
        assert_eq!(plugin.registry.as_deref(), Some("awesome-copilot"));
        assert_eq!(
            plugin.path.as_ref().map(|path| path.as_str()),
            Some(plugin_dir.to_string_lossy().as_ref())
        );
        assert!(hash_installed_plugin_manifest(plugin)
            .expect("hash")
            .unwrap_or_default()
            .starts_with("sha256:"));
    }

    #[test]
    fn marker_backfills_missing_path_from_index_entry() {
        let dir = TempDir::new().expect("tempdir");
        let copilot_dir = dir.path().join(".copilot");
        let plugins_dir = copilot_dir.join("plugins");
        std::fs::create_dir_all(&plugins_dir).expect("mkdir");
        std::fs::write(
            copilot_dir.join("plugin-index.json"),
            r#"[{"name":"pptx","version":"1.0.0"}]"#,
        )
        .expect("index");
        std::fs::write(plugins_dir.join("pptx.installed"), "").expect("marker");

        let plugins =
            read_installed_plugins_from(&copilot_dir.join("plugin-index.json")).expect("read");

        assert_eq!(plugins.len(), 1);
        let plugin = find_installed_plugin_by_name(&plugins, "pptx").expect("plugin");
        assert_eq!(plugin.version.as_deref(), Some("1.0.0"));
        assert_eq!(
            plugin.path.as_ref().map(|path| path.as_str()),
            Some(plugins_dir.join("pptx").to_string_lossy().as_ref())
        );
    }

    #[test]
    fn parse_errors_surface_with_file_context() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("plugin-index.json");
        std::fs::write(&path, "{ invalid json").expect("write");

        let err = read_installed_plugins_from(&path).expect_err("error");

        assert!(matches!(err, CpmError::Parse { .. }));
        if let CpmError::Parse { file, msg } = err {
            assert_eq!(file, path.display().to_string());
            assert!(!msg.is_empty());
        }
    }
}
