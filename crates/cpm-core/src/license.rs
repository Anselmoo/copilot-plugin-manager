//! License detection and policy enforcement helpers.

use std::path::{Path, PathBuf};

use cpm_types::{AssetSource, LicenseInfo, LicensePolicy, ResolvedAsset, SourceRule};
use indexmap::IndexMap;
use reqwest::header;
use tracing::warn;

use crate::{
    config::{rewrite_source_url, EffectiveSettings},
    CpmError,
};

const LICENSE_CANDIDATES: [&str; 5] = [
    "LICENSE",
    "LICENSE.md",
    "LICENSE.txt",
    "COPYING",
    "COPYING.md",
];
const UNKNOWN_LICENSE: &str = "UNKNOWN";

/// Detect the license metadata for an asset source, when practical.
pub async fn detect_license(
    source: &AssetSource,
    resolved_rev: &str,
    repo_root: &Path,
    client: &reqwest::Client,
    token: Option<&str>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<Option<LicenseInfo>, CpmError> {
    if let Some(path) = &source.path {
        return Ok(Some(detect_local_license(path.as_str(), repo_root)?));
    }

    let Some(url) = source.url.as_deref() else {
        return Ok(None);
    };

    let Some(repo) = parse_github_repo(url) else {
        return Ok(None);
    };

    Ok(Some(
        detect_github_license(&repo, resolved_rev, client, token, source_rules).await,
    ))
}

/// Enforce the configured license policy for a resolved asset.
pub fn enforce_license_policy(
    asset: &ResolvedAsset,
    settings: &EffectiveSettings,
) -> Result<(), CpmError> {
    let Some(license) = asset.license.as_ref() else {
        return Ok(());
    };

    match settings.license_policy {
        LicensePolicy::AllowAll => Ok(()),
        LicensePolicy::WarnCopyleft => {
            if is_copyleft_expression(&license.spdx) {
                warn!(
                    "copyleft license detected for {} ({}): {}",
                    asset.name, asset.kind, license.spdx
                );
            }
            Ok(())
        }
        LicensePolicy::DenyCopyleft => {
            if is_copyleft_expression(&license.spdx) {
                return Err(CpmError::LicenseViolation {
                    name: asset.name.clone(),
                    kind: asset.kind,
                    license: license.spdx.clone(),
                    policy: "deny-copyleft".into(),
                });
            }
            Ok(())
        }
        LicensePolicy::AllowList => {
            if license_allowed(&license.spdx, &settings.allowed_licenses) {
                Ok(())
            } else {
                Err(CpmError::LicenseViolation {
                    name: asset.name.clone(),
                    kind: asset.kind,
                    license: license.spdx.clone(),
                    policy: "allow-list".into(),
                })
            }
        }
    }
}

fn detect_local_license(source_path: &str, repo_root: &Path) -> Result<LicenseInfo, CpmError> {
    let source_root = resolve_local_source_path(source_path, repo_root);
    let mut current = if source_root.is_dir() {
        source_root
    } else {
        source_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| repo_root.to_path_buf())
    };

    loop {
        if let Some(found) = detect_license_in_directory(&current)? {
            return Ok(found);
        }
        if current == repo_root {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent.to_path_buf();
    }

    Ok(LicenseInfo {
        spdx: UNKNOWN_LICENSE.into(),
        url: None,
        verified: false,
    })
}

fn resolve_local_source_path(source_path: &str, repo_root: &Path) -> PathBuf {
    let path = PathBuf::from(source_path);
    if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    }
}

fn detect_license_in_directory(directory: &Path) -> Result<Option<LicenseInfo>, CpmError> {
    for candidate in LICENSE_CANDIDATES {
        let path = directory.join(candidate);
        if !path.is_file() {
            continue;
        }

        let text = std::fs::read_to_string(&path)?;
        let (spdx, verified) = parse_license_text(&text);
        return Ok(Some(LicenseInfo {
            spdx,
            url: None,
            verified,
        }));
    }

    Ok(None)
}

async fn detect_github_license(
    repo: &GitHubRepo,
    resolved_rev: &str,
    client: &reqwest::Client,
    token: Option<&str>,
    source_rules: &IndexMap<String, SourceRule>,
) -> LicenseInfo {
    if let Some(license) =
        detect_github_license_via_api(repo, resolved_rev, client, token, source_rules).await
    {
        return license;
    }

    for candidate in LICENSE_CANDIDATES {
        let raw_url = format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}",
            repo.owner, repo.repo, resolved_rev, candidate
        );
        let target = rewrite_source_url(&raw_url, source_rules);
        let mut request = client
            .get(&target.url)
            .header(header::USER_AGENT, env!("CARGO_PKG_NAME"));
        if let Some(token) = target.token.as_deref().or(token) {
            request = request.bearer_auth(token);
        }

        let Ok(response) = request.send().await else {
            continue;
        };
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        let Ok(response) = response.error_for_status() else {
            continue;
        };
        let Ok(text) = response.text().await else {
            continue;
        };
        let (spdx, verified) = parse_license_text(&text);
        return LicenseInfo {
            spdx,
            url: Some(format!(
                "https://github.com/{}/{}/blob/{}/{}",
                repo.owner, repo.repo, resolved_rev, candidate
            )),
            verified,
        };
    }

    LicenseInfo {
        spdx: UNKNOWN_LICENSE.into(),
        url: None,
        verified: false,
    }
}

async fn detect_github_license_via_api(
    repo: &GitHubRepo,
    resolved_rev: &str,
    client: &reqwest::Client,
    token: Option<&str>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Option<LicenseInfo> {
    let api_url = format!(
        "https://api.github.com/repos/{}/{}/license?ref={}",
        repo.owner, repo.repo, resolved_rev
    );
    let target = rewrite_source_url(&api_url, source_rules);
    let mut request = client
        .get(&target.url)
        .header(header::USER_AGENT, env!("CARGO_PKG_NAME"));
    if let Some(token) = target.token.as_deref().or(token) {
        request = request.bearer_auth(token);
    }

    let Ok(response) = request.send().await else {
        return None;
    };
    if !response.status().is_success() {
        return None;
    }

    let Ok(payload) = response.json::<GitHubLicenseResponse>().await else {
        return None;
    };
    let spdx = payload
        .license
        .and_then(|license| normalize_spdx_id(&license.spdx_id))
        .unwrap_or_else(|| UNKNOWN_LICENSE.into());

    Some(LicenseInfo {
        spdx,
        url: payload.html_url,
        verified: true,
    })
}

fn parse_license_text(text: &str) -> (String, bool) {
    if let Some(spdx) = parse_spdx_identifier(text) {
        return (spdx, true);
    }

    let normalized = text.to_ascii_lowercase();
    if normalized.contains("mit license") {
        return ("MIT".into(), false);
    }
    if normalized.contains("apache license") && normalized.contains("version 2.0") {
        return ("Apache-2.0".into(), false);
    }
    if normalized.contains("gnu affero general public license") && normalized.contains("version 3")
    {
        return ("AGPL-3.0-only".into(), false);
    }
    if normalized.contains("gnu lesser general public license") && normalized.contains("version 3")
    {
        return ("LGPL-3.0-only".into(), false);
    }
    if normalized.contains("gnu lesser general public license")
        && normalized.contains("version 2.1")
    {
        return ("LGPL-2.1-only".into(), false);
    }
    if normalized.contains("gnu general public license") && normalized.contains("version 3") {
        return ("GPL-3.0-only".into(), false);
    }
    if normalized.contains("gnu general public license") && normalized.contains("version 2") {
        return ("GPL-2.0-only".into(), false);
    }
    if normalized.contains("mozilla public license") && normalized.contains("2.0") {
        return ("MPL-2.0".into(), false);
    }

    (UNKNOWN_LICENSE.into(), false)
}

fn parse_spdx_identifier(text: &str) -> Option<String> {
    text.lines().take(10).find_map(|line| {
        let marker = "spdx-license-identifier:";
        let lowercase = line.to_ascii_lowercase();
        let index = lowercase.find(marker)?;
        let value = line[index + marker.len()..].trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_owned())
        }
    })
}

fn normalize_spdx_id(spdx_id: &str) -> Option<String> {
    let normalized = spdx_id.trim();
    if normalized.is_empty() || normalized.eq_ignore_ascii_case("noassertion") {
        None
    } else {
        Some(normalized.to_owned())
    }
}

fn license_allowed(spdx: &str, allowed_licenses: &[String]) -> bool {
    let allow_list: Vec<String> = allowed_licenses
        .iter()
        .map(|item| item.trim().to_ascii_uppercase())
        .filter(|item| !item.is_empty())
        .collect();
    if allow_list.is_empty() {
        return false;
    }

    spdx_tokens(spdx)
        .into_iter()
        .any(|token| allow_list.iter().any(|allowed| allowed == &token))
}

fn is_copyleft_expression(spdx: &str) -> bool {
    spdx_tokens(spdx).into_iter().any(|token| {
        token.contains("GPL")
            || token.contains("AGPL")
            || token.contains("LGPL")
            || token.contains("MPL")
            || token.contains("EPL")
            || token.contains("CDDL")
    })
}

fn spdx_tokens(spdx: &str) -> Vec<String> {
    spdx.split(|character: char| {
        character.is_whitespace() || matches!(character, '(' | ')' | '+' | ',')
    })
    .filter_map(|token| {
        let trimmed = token.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("AND")
            || trimmed.eq_ignore_ascii_case("OR")
            || trimmed.eq_ignore_ascii_case("WITH")
        {
            None
        } else {
            Some(trimmed.to_ascii_uppercase())
        }
    })
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHubRepo {
    owner: String,
    repo: String,
}

fn parse_github_repo(url: &str) -> Option<GitHubRepo> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let segments: Vec<_> = parsed
        .path_segments()
        .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
        .unwrap_or_default();

    match parsed.host_str()? {
        "github.com" | "raw.githubusercontent.com" if segments.len() >= 2 => Some(GitHubRepo {
            owner: segments[0].to_owned(),
            repo: segments[1].to_owned(),
        }),
        _ => None,
    }
}

#[derive(Debug, serde::Deserialize)]
struct GitHubLicenseResponse {
    html_url: Option<String>,
    license: Option<GitHubLicensePayload>,
}

#[derive(Debug, serde::Deserialize)]
struct GitHubLicensePayload {
    spdx_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpm_types::{AssetKind, Scope, UpdatePolicy};

    fn test_settings(policy: LicensePolicy, allowed_licenses: Vec<&str>) -> EffectiveSettings {
        EffectiveSettings {
            default_scope: Scope::Local,
            update_policy: UpdatePolicy::Locked,
            license_policy: policy,
            allowed_licenses: allowed_licenses.into_iter().map(str::to_owned).collect(),
            cache_dir: PathBuf::from("/tmp/.cpm-cache"),
            network_timeout: 30,
            auto_groups: vec!["default".into()],
            verify_on_sync: false,
            auto_compile_workflows: false,
        }
    }

    fn resolved_asset(spdx: &str, kind: AssetKind) -> ResolvedAsset {
        ResolvedAsset {
            name: "demo".into(),
            kind,
            source: AssetSource {
                url: Some("https://github.com/example/demo".into()),
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
            ownership: cpm_types::AssetOwnership::Upstream,
            files: vec![],
            executable: vec![],
            file_hashes: Default::default(),
            git: None,
            sub_assets: vec![],
            license: Some(LicenseInfo {
                spdx: spdx.into(),
                url: None,
                verified: true,
            }),
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        }
    }

    #[test]
    fn parses_spdx_identifier_header() {
        let (spdx, verified) = parse_license_text(
            "# Example\nSPDX-License-Identifier: MIT OR Apache-2.0\nrest of file",
        );
        assert_eq!(spdx, "MIT OR Apache-2.0");
        assert!(verified);
    }

    #[test]
    fn allow_list_accepts_any_matching_token() {
        assert!(license_allowed(
            "MIT OR Apache-2.0",
            &["MIT".into(), "BSD-3-Clause".into()]
        ));
    }

    #[test]
    fn deny_copyleft_rejects_gpl_assets() {
        let asset = resolved_asset("GPL-3.0-only", AssetKind::Plugin);
        let settings = test_settings(LicensePolicy::DenyCopyleft, vec![]);

        let err = enforce_license_policy(&asset, &settings).expect_err("GPL should fail");
        assert!(matches!(err, CpmError::LicenseViolation { .. }));
    }

    #[test]
    fn allow_list_rejects_unknown_license() {
        let asset = resolved_asset("UNKNOWN", AssetKind::Skill);
        let settings = test_settings(LicensePolicy::AllowList, vec!["MIT"]);

        let err = enforce_license_policy(&asset, &settings).expect_err("unknown should fail");
        assert!(matches!(err, CpmError::LicenseViolation { .. }));
    }
}
