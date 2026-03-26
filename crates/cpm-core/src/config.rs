//! Runtime settings and user-config loading.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use cpm_types::{
    LicensePolicy, Manifest, PartialSettings, Scope, SourceRule, UpdatePolicy, UserConfig,
};
use indexmap::IndexMap;

use crate::CpmError;

/// Fully resolved settings after applying defaults and precedence rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveSettings {
    /// Effective default scope.
    pub default_scope: Scope,
    /// Effective update policy.
    pub update_policy: UpdatePolicy,
    /// Effective license policy.
    pub license_policy: LicensePolicy,
    /// Allowed licenses for allow-list mode.
    pub allowed_licenses: Vec<String>,
    /// Effective cache directory.
    pub cache_dir: PathBuf,
    /// Effective request timeout, in seconds.
    pub network_timeout: u64,
    /// Groups installed automatically during sync.
    pub auto_groups: Vec<String>,
    /// Whether sync should verify installs more aggressively.
    pub verify_on_sync: bool,
    /// Whether workflow markdown should be compiled into `.lock.yml` outputs.
    pub auto_compile_workflows: bool,
}

impl Default for EffectiveSettings {
    fn default() -> Self {
        Self {
            default_scope: Scope::Local,
            update_policy: UpdatePolicy::Locked,
            license_policy: LicensePolicy::WarnCopyleft,
            allowed_licenses: Vec::new(),
            cache_dir: default_cache_dir(),
            network_timeout: 30,
            auto_groups: vec!["default".to_owned()],
            verify_on_sync: false,
            auto_compile_workflows: false,
        }
    }
}

/// Runtime config bundle used by CLI commands and materialization flows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// Effective settings.
    pub settings: EffectiveSettings,
    /// Effective source rewrite / auth rules.
    pub source_rules: IndexMap<String, SourceRule>,
    /// Loaded user config document.
    pub user_config: UserConfig,
    /// User config path consulted for overrides.
    pub user_config_path: PathBuf,
}

/// Rewritten network target for a URL fetch or runtime launch config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkTarget {
    /// Final URL to use.
    pub url: String,
    /// Token override sourced from a matching rule, if any.
    pub token: Option<String>,
}

/// Return the default user config path.
pub fn default_user_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("cpm")
        .join("config.toml")
}

/// Load user config from the default path.
pub fn load_user_config() -> Result<(UserConfig, PathBuf), CpmError> {
    let path = default_user_config_path();
    let config = load_user_config_from(&path)?;
    Ok((config, path))
}

/// Load user config from an explicit path.
pub fn load_user_config_from(path: &Path) -> Result<UserConfig, CpmError> {
    if !path.exists() {
        return Ok(UserConfig::default());
    }

    let contents = std::fs::read_to_string(path)?;
    toml::from_str(&contents).map_err(|err| CpmError::Parse {
        file: path.display().to_string(),
        msg: err.to_string(),
    })
}

/// Load runtime config for the provided manifest.
pub fn load_runtime_config(manifest: &Manifest) -> Result<RuntimeConfig, CpmError> {
    let (user_config, user_config_path) = load_user_config()?;
    let settings = resolve_settings(&manifest.settings, &user_config.settings)?;
    let source_rules = merge_source_rules(&manifest.sources, &user_config.sources);
    Ok(RuntimeConfig {
        settings,
        source_rules,
        user_config,
        user_config_path,
    })
}

/// Merge repo and user source rules, allowing user config to override a
/// same-named project rule while preserving project-defined defaults.
pub fn merge_source_rules(
    repo_sources: &IndexMap<String, SourceRule>,
    user_sources: &IndexMap<String, SourceRule>,
) -> IndexMap<String, SourceRule> {
    let mut merged = repo_sources.clone();
    for (name, rule) in user_sources {
        merged.insert(name.clone(), rule.clone());
    }
    merged
}

/// Resolve effective settings using precedence:
/// CLI flags > environment variables > repo settings > user config > built-in defaults.
pub fn resolve_settings(
    repo_settings: &PartialSettings,
    user_settings: &PartialSettings,
) -> Result<EffectiveSettings, CpmError> {
    let defaults = EffectiveSettings::default();

    Ok(EffectiveSettings {
        default_scope: parse_scope_env("CPM_DEFAULT_SCOPE")?
            .or(repo_settings.default_scope)
            .or(user_settings.default_scope)
            .unwrap_or(defaults.default_scope),
        update_policy: parse_update_policy_env("CPM_UPDATE_POLICY")?
            .or(repo_settings.update_policy)
            .or(user_settings.update_policy)
            .unwrap_or(defaults.update_policy),
        license_policy: parse_license_policy_env("CPM_LICENSE_POLICY")?
            .or(repo_settings.license_policy)
            .or(user_settings.license_policy)
            .unwrap_or(defaults.license_policy),
        allowed_licenses: env_list("CPM_ALLOWED_LICENSES")
            .or_else(|| repo_settings.allowed_licenses.clone())
            .or_else(|| user_settings.allowed_licenses.clone())
            .unwrap_or(defaults.allowed_licenses),
        cache_dir: std::env::var_os("CPM_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| repo_settings.cache_dir.as_deref().map(expand_path_like))
            .or_else(|| user_settings.cache_dir.as_deref().map(expand_path_like))
            .unwrap_or(defaults.cache_dir),
        network_timeout: parse_u64_env("CPM_NETWORK_TIMEOUT")?
            .or(repo_settings.network_timeout)
            .or(user_settings.network_timeout)
            .unwrap_or(defaults.network_timeout),
        auto_groups: env_list("CPM_AUTO_GROUPS")
            .or_else(|| repo_settings.auto_groups.clone())
            .or_else(|| user_settings.auto_groups.clone())
            .unwrap_or(defaults.auto_groups),
        verify_on_sync: parse_bool_env("CPM_VERIFY_ON_SYNC")?
            .or(repo_settings.verify_on_sync)
            .or(user_settings.verify_on_sync)
            .unwrap_or(defaults.verify_on_sync),
        auto_compile_workflows: parse_bool_env("CPM_AUTO_COMPILE_WORKFLOWS")?
            .or(repo_settings.auto_compile_workflows)
            .or(user_settings.auto_compile_workflows)
            .unwrap_or(defaults.auto_compile_workflows),
    })
}

/// Build a reqwest client honoring the effective timeout setting.
pub fn build_http_client(
    user_agent: String,
    settings: &EffectiveSettings,
) -> Result<reqwest::Client, CpmError> {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(Duration::from_secs(settings.network_timeout))
        .build()
        .map_err(CpmError::from)
}

/// Apply the first matching source rewrite to the provided URL.
pub fn rewrite_source_url(url: &str, sources: &IndexMap<String, SourceRule>) -> NetworkTarget {
    for rule in sources.values() {
        let Some(prefix) = rule.replace.as_deref() else {
            continue;
        };
        let Some(suffix) = url.strip_prefix(prefix) else {
            continue;
        };

        let token = rule
            .token_env
            .as_deref()
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.is_empty());

        return NetworkTarget {
            url: join_url(&rule.url, suffix),
            token,
        };
    }

    NetworkTarget {
        url: url.to_owned(),
        token: None,
    }
}

fn parse_scope_env(name: &str) -> Result<Option<Scope>, CpmError> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<Scope>()
            .map(Some)
            .map_err(|reason| CpmError::InvalidConfig {
                key: name.to_owned(),
                reason,
            }),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(CpmError::InvalidConfig {
            key: name.to_owned(),
            reason: err.to_string(),
        }),
    }
}

fn parse_update_policy_env(name: &str) -> Result<Option<UpdatePolicy>, CpmError> {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "locked" => Ok(Some(UpdatePolicy::Locked)),
            "latest" => Ok(Some(UpdatePolicy::Latest)),
            "tagged" => Ok(Some(UpdatePolicy::Tagged)),
            other => Err(CpmError::InvalidConfig {
                key: name.to_owned(),
                reason: format!("unsupported update policy `{other}`"),
            }),
        },
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(CpmError::InvalidConfig {
            key: name.to_owned(),
            reason: err.to_string(),
        }),
    }
}

fn parse_license_policy_env(name: &str) -> Result<Option<LicensePolicy>, CpmError> {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "allow-all" => Ok(Some(LicensePolicy::AllowAll)),
            "warn-copyleft" => Ok(Some(LicensePolicy::WarnCopyleft)),
            "deny-copyleft" => Ok(Some(LicensePolicy::DenyCopyleft)),
            "allow-list" => Ok(Some(LicensePolicy::AllowList)),
            other => Err(CpmError::InvalidConfig {
                key: name.to_owned(),
                reason: format!("unsupported license policy `{other}`"),
            }),
        },
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(CpmError::InvalidConfig {
            key: name.to_owned(),
            reason: err.to_string(),
        }),
    }
}

fn parse_u64_env(name: &str) -> Result<Option<u64>, CpmError> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|err| CpmError::InvalidConfig {
                key: name.to_owned(),
                reason: err.to_string(),
            }),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(CpmError::InvalidConfig {
            key: name.to_owned(),
            reason: err.to_string(),
        }),
    }
}

fn parse_bool_env(name: &str) -> Result<Option<bool>, CpmError> {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(Some(true)),
            "0" | "false" | "no" | "off" => Ok(Some(false)),
            other => Err(CpmError::InvalidConfig {
                key: name.to_owned(),
                reason: format!("unsupported boolean value `{other}`"),
            }),
        },
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(CpmError::InvalidConfig {
            key: name.to_owned(),
            reason: err.to_string(),
        }),
    }
}

fn env_list(name: &str) -> Option<Vec<String>> {
    std::env::var(name).ok().map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    })
}

fn default_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp/.cpm-cache"))
        .join("cpm")
}

fn expand_path_like(value: &str) -> PathBuf {
    if value == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(value));
    }

    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }

    PathBuf::from(value)
}

fn join_url(base: &str, suffix: &str) -> String {
    if base.ends_with('/') && suffix.starts_with('/') {
        format!("{}{}", base.trim_end_matches('/'), suffix)
    } else if !base.ends_with('/') && !suffix.is_empty() && !suffix.starts_with('/') {
        format!("{base}/{suffix}")
    } else {
        format!("{base}{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn repo_settings_override_user_settings() {
        let repo = PartialSettings {
            default_scope: Some(Scope::Global),
            network_timeout: Some(5),
            ..PartialSettings::default()
        };
        let user = PartialSettings {
            default_scope: Some(Scope::Local),
            network_timeout: Some(30),
            ..PartialSettings::default()
        };

        let effective = resolve_settings(&repo, &user).expect("settings");
        assert_eq!(effective.default_scope, Scope::Global);
        assert_eq!(effective.network_timeout, 5);
    }

    #[test]
    fn environment_overrides_repo_and_user_settings() {
        let repo = PartialSettings {
            default_scope: Some(Scope::Local),
            ..PartialSettings::default()
        };
        let user = PartialSettings {
            default_scope: Some(Scope::Global),
            ..PartialSettings::default()
        };

        std::env::set_var("CPM_DEFAULT_SCOPE", "global");
        let effective = resolve_settings(&repo, &user).expect("settings");
        std::env::remove_var("CPM_DEFAULT_SCOPE");

        assert_eq!(effective.default_scope, Scope::Global);
    }

    #[test]
    fn source_rules_rewrite_matching_urls() {
        let mut sources = IndexMap::new();
        sources.insert(
            "corp".to_owned(),
            SourceRule {
                url: "https://mirror.example.com".to_owned(),
                token_env: None,
                replace: Some("https://github.com/corp".to_owned()),
            },
        );

        let target =
            rewrite_source_url("https://github.com/corp/repo/blob/main/README.md", &sources);

        assert_eq!(
            target.url,
            "https://mirror.example.com/repo/blob/main/README.md"
        );
        assert_eq!(target.token, None);
    }

    #[test]
    fn merge_source_rules_allows_user_override_of_same_named_rule() {
        let mut repo_sources = IndexMap::new();
        repo_sources.insert(
            "internal".to_owned(),
            SourceRule {
                url: "https://repo.example.com".to_owned(),
                token_env: None,
                replace: Some("https://github.com/acme".to_owned()),
            },
        );
        let mut user_sources = IndexMap::new();
        user_sources.insert(
            "internal".to_owned(),
            SourceRule {
                url: "https://user.example.com".to_owned(),
                token_env: Some("ACME_TOKEN".to_owned()),
                replace: Some("https://github.com/acme".to_owned()),
            },
        );

        let merged = merge_source_rules(&repo_sources, &user_sources);
        let rule = merged.get("internal").expect("merged rule");
        assert_eq!(rule.url, "https://user.example.com");
        assert_eq!(rule.token_env.as_deref(), Some("ACME_TOKEN"));
    }

    #[test]
    fn load_user_config_returns_default_when_file_missing() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        let config = load_user_config_from(&path).expect("config");
        assert_eq!(config, UserConfig::default());
    }
}
