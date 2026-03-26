//! Asset source inference and normalization helpers.

use std::path::Path;
use std::time::Duration;

use camino::Utf8PathBuf;
use cpm_types::{AssetKind, SourceRule};
use indexmap::IndexMap;
use reqwest::{header, Url};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::config::rewrite_source_url;
use crate::fetcher::{atomic_write, cache_dir};
use crate::CpmError;

/// A normalized asset source that can be written into `cpm.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedAssetSource {
    /// Manifest key to use for the asset.
    pub name: String,
    /// Canonical remote URL, when the source is remote.
    pub url: Option<String>,
    /// Canonical local asset tree or file path, when the source is local.
    pub path: Option<Utf8PathBuf>,
}

/// Parsed GitHub asset source information for canonical `github.com` tree/blob URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubSource {
    /// Repository owner or organization.
    pub owner: String,
    /// Repository name.
    pub repo: String,
    /// Git ref embedded in the URL.
    pub git_ref: String,
    /// Asset path inside the repository.
    pub path: String,
    /// Whether the URL points at a single file or a directory tree.
    pub mode: GitHubSourceMode,
}

/// Canonical GitHub URL mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHubSourceMode {
    /// A single-file `blob` URL.
    Blob,
    /// A directory `tree` URL.
    Tree,
}

/// Infer an asset kind from its path-like source.
pub fn infer_kind_from_source(source: &str) -> Option<AssetKind> {
    let segments = inference_segments(source);

    let mut inferred = None;
    for segment in segments {
        inferred = match segment.as_str() {
            "plugins" => Some(AssetKind::Plugin),
            "skills" => Some(AssetKind::Skill),
            "agents" => Some(AssetKind::Agent),
            "hooks" => Some(AssetKind::Hook),
            "workflows" => Some(AssetKind::Workflow),
            "instructions" => Some(AssetKind::Instruction),
            _ => inferred,
        };
    }

    if inferred.is_some() {
        return inferred;
    }

    let last = last_segment(source)?;
    if last == "SKILL.md" {
        return Some(AssetKind::Skill);
    }
    if last == "hooks.json" {
        return Some(AssetKind::Hook);
    }
    if last.ends_with(".agent.md") {
        return Some(AssetKind::Agent);
    }
    if last.ends_with(".instructions.md") {
        return Some(AssetKind::Instruction);
    }
    if last.ends_with(".md") {
        return Some(AssetKind::Workflow);
    }

    None
}

/// Normalize a source string into a concrete asset tree URL or local asset path.
pub fn normalize_asset_source(
    kind: AssetKind,
    source: &str,
) -> Result<NormalizedAssetSource, CpmError> {
    if let Ok(url) = Url::parse(source) {
        return normalize_url_source(kind, source, &url);
    }

    normalize_path_source(kind, source)
}

/// Resolve a GitHub-backed source to an immutable commit SHA when possible.
///
/// If `explicit_rev` is provided, it takes precedence over the ref embedded in
/// the source URL. Non-GitHub sources keep the provided `explicit_rev`
/// unchanged.
pub async fn resolve_pinned_rev(
    client: &reqwest::Client,
    token: Option<&str>,
    source: Option<&str>,
    explicit_rev: Option<&str>,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<Option<String>, CpmError> {
    resolve_pinned_rev_with_api_base(
        client,
        token,
        source,
        explicit_rev,
        "https://api.github.com",
        source_rules,
    )
    .await
}

/// Parse a canonical GitHub `blob` or `tree` URL into structured repository information.
pub fn parse_github_source(source: &str) -> Option<GitHubSource> {
    let url = Url::parse(source).ok()?;
    if url.host_str()? != "github.com" {
        return None;
    }

    let segments = source_segments(source);
    if segments.len() < 5 {
        return None;
    }

    let mode = match segments[2].as_str() {
        "blob" => GitHubSourceMode::Blob,
        "tree" => GitHubSourceMode::Tree,
        _ => return None,
    };

    let inner: Vec<&str> = segments[3..].iter().map(|s| s.as_str()).collect();
    let (git_ref, path) = split_git_ref_and_path_any_catalog(&inner);
    if git_ref.is_empty() || path.is_empty() {
        return None;
    }

    Some(GitHubSource {
        owner: segments[0].clone(),
        repo: segments[1].clone(),
        git_ref,
        path,
        mode,
    })
}

fn normalize_url_source(
    kind: AssetKind,
    source: &str,
    url: &Url,
) -> Result<NormalizedAssetSource, CpmError> {
    match url.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(CpmError::UnsupportedUrl {
                url: source.to_owned(),
            });
        }
    }

    match url.host_str() {
        Some("github.com") => normalize_github_url(kind, source, url),
        Some("raw.githubusercontent.com") => normalize_raw_github_url(kind, source, url),
        _ => normalize_generic_file_url(kind, source, url),
    }
}

async fn resolve_pinned_rev_with_api_base(
    client: &reqwest::Client,
    token: Option<&str>,
    source: Option<&str>,
    explicit_rev: Option<&str>,
    api_base: &str,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<Option<String>, CpmError> {
    let Some(source) = source else {
        return Ok(explicit_rev.map(ToOwned::to_owned));
    };

    let Some(repo_source) = github_repo_source(source) else {
        return Ok(explicit_rev.map(ToOwned::to_owned));
    };

    let requested_ref = explicit_rev.unwrap_or(&repo_source.git_ref);
    if is_full_commit_sha(requested_ref) {
        return Ok(Some(requested_ref.to_owned()));
    }

    let sha = resolve_github_commit_sha(
        client,
        token,
        api_base,
        &repo_source.owner,
        &repo_source.repo,
        requested_ref,
        source_rules,
    )
    .await?;
    Ok(Some(sha))
}

/// Resolve a package-backed MCP transport to an exact runtime version.
pub async fn resolve_package_transport_version(
    client: &reqwest::Client,
    transport: &cpm_types::McpTransport,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<Option<String>, CpmError> {
    match transport {
        cpm_types::McpTransport::Npx { package, .. } => resolve_cached_text(
            &format!("npm:{package}"),
            Duration::from_secs(300),
            || async move { resolve_npm_version(client, package, source_rules).await },
        )
        .await
        .map(Some),
        cpm_types::McpTransport::Uvx { package, .. } => resolve_cached_text(
            &format!("pypi:{package}"),
            Duration::from_secs(300),
            || async move { resolve_pypi_version(client, package, source_rules).await },
        )
        .await
        .map(Some),
        _ => Ok(None),
    }
}

/// Inferred runner kind for a GitHub repo MCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferredMcpRunner {
    /// Python packaging signals detected (`pyproject.toml`, `setup.py`, or `setup.cfg`).
    Uvx,
    /// npm packaging signals detected (`package.json`).
    Npx,
}

/// Probe a bare GitHub repo URL to infer the MCP runner with high confidence.
///
/// Checks the root directory of the repository for packaging signals:
/// - Python (`pyproject.toml`, `setup.py`, `setup.cfg`) → [`InferredMcpRunner::Uvx`]
/// - npm (`package.json`) → [`InferredMcpRunner::Npx`]
///
/// Returns `None` when the signals are ambiguous or the API request fails, so
/// callers must fall back to an explicit error rather than guessing.
///
/// Only bare GitHub repo URLs (i.e. `https://github.com/owner/repo`) are
/// accepted; tree/blob URLs should be handled via the normal asset-source path.
pub async fn infer_github_repo_mcp_runner(
    client: &reqwest::Client,
    token: Option<&str>,
    repo_url: &str,
    source_rules: &IndexMap<String, SourceRule>,
) -> Option<InferredMcpRunner> {
    let url = Url::parse(repo_url).ok()?;
    if url.host_str()? != "github.com" {
        return None;
    }
    let segments = source_segments(repo_url);
    if segments.len() < 2 {
        return None;
    }
    // Only handle bare repos — not blob/tree/commit/releases paths.
    if segments.len() >= 3 {
        return None;
    }
    let owner = &segments[0];
    let repo = &segments[1];
    infer_runner_from_repo_root(
        client,
        token,
        "https://api.github.com",
        owner,
        repo,
        source_rules,
    )
    .await
}

async fn infer_runner_from_repo_root(
    client: &reqwest::Client,
    token: Option<&str>,
    api_base: &str,
    owner: &str,
    repo: &str,
    source_rules: &IndexMap<String, SourceRule>,
) -> Option<InferredMcpRunner> {
    let url = format!("{api_base}/repos/{owner}/{repo}/contents/");
    let target = rewrite_source_url(&url, source_rules);
    let mut request = client
        .get(&target.url)
        .header(header::USER_AGENT, env!("CARGO_PKG_NAME"))
        .header(header::ACCEPT, "application/vnd.github.v3+json");
    if let Some(t) = target.token.as_deref().or(token) {
        request = request.bearer_auth(t);
    }

    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }

    #[derive(Deserialize)]
    struct Entry {
        name: String,
    }

    let entries: Vec<Entry> = response.json().await.ok()?;
    let file_names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();

    // Python packaging signals take precedence: check pyproject.toml first (most modern).
    let has_pyproject = file_names.contains(&"pyproject.toml");
    let has_setup_py = file_names.contains(&"setup.py");
    let has_setup_cfg = file_names.contains(&"setup.cfg");
    let has_npm = file_names.contains(&"package.json");

    if has_pyproject || has_setup_py || has_setup_cfg {
        return Some(InferredMcpRunner::Uvx);
    }
    if has_npm {
        return Some(InferredMcpRunner::Npx);
    }
    None
}

/// Return the stable pin embedded in a Docker image reference, when present.
///
/// - `ghcr.io/org/server:1.2.3` -> `Some("1.2.3")`
/// - `ghcr.io/org/server@sha256:abcd` -> `Some("sha256:abcd")`
/// - `ghcr.io/org/server:latest` -> `None`
/// - `ghcr.io/org/server` -> `None`
pub fn docker_image_pin(image: &str) -> Option<String> {
    let (without_digest, digest) = split_docker_digest(image);
    if let Some(digest) = digest {
        return Some(digest.to_owned());
    }

    let (_, tag) = split_docker_tag(without_digest);
    tag.filter(|tag| !tag.is_empty() && *tag != "latest")
        .map(ToOwned::to_owned)
}

/// Return the display/name-friendly final segment of a Docker image reference.
///
/// - `ghcr.io/org/server:1.2.3` -> `server`
/// - `docker.io/library/postgres@sha256:abcd` -> `postgres`
pub fn docker_image_name(image: &str) -> String {
    let (without_digest, _) = split_docker_digest(image);
    let (without_tag, _) = split_docker_tag(without_digest);
    without_tag
        .rsplit('/')
        .next()
        .unwrap_or(without_tag)
        .to_owned()
}

fn split_docker_digest(image: &str) -> (&str, Option<&str>) {
    match image.rsplit_once('@') {
        Some((base, digest)) if !digest.is_empty() => (base, Some(digest)),
        _ => (image, None),
    }
}

fn split_docker_tag(image: &str) -> (&str, Option<&str>) {
    let last_slash = image.rfind('/');
    let last_colon = image.rfind(':');
    if let Some(colon) = last_colon {
        if last_slash.is_none_or(|slash| colon > slash) {
            return (&image[..colon], Some(&image[colon + 1..]));
        }
    }
    (image, None)
}

fn normalize_github_url(
    kind: AssetKind,
    source: &str,
    url: &Url,
) -> Result<NormalizedAssetSource, CpmError> {
    let segments: Vec<_> = url
        .path_segments()
        .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
        .unwrap_or_default();

    if segments.len() < 5 {
        return Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: "use a GitHub blob or tree URL that points at a concrete asset".to_owned(),
        });
    }

    let owner = segments[0];
    let repo = segments[1];
    let mode = segments[2];

    if mode != "blob" && mode != "tree" {
        return Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: "use a GitHub blob or tree URL rather than a repository landing page"
                .to_owned(),
        });
    }

    let catalog = plural_dir(kind);
    // segments is Vec<&str> from url.path_segments(); pass the slice directly.
    let (git_ref, path) = split_git_ref_and_path_for_kind(&segments[3..], catalog);

    if git_ref.is_empty() {
        return Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: "use a GitHub blob or tree URL that points at a concrete asset".to_owned(),
        });
    }

    normalize_repo_path(kind, source, owner, repo, &git_ref, &path)
}

fn normalize_raw_github_url(
    kind: AssetKind,
    source: &str,
    url: &Url,
) -> Result<NormalizedAssetSource, CpmError> {
    let segments: Vec<_> = url
        .path_segments()
        .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
        .unwrap_or_default();

    if segments.len() < 4 {
        return Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: "raw GitHub URLs must include owner, repo, ref, and file path".to_owned(),
        });
    }

    let owner = segments[0];
    let repo = segments[1];
    let git_ref = segments[2];
    let path = segments[3..].join("/");

    let normalized = normalize_asset_path(kind, source, &path)?;
    Ok(NormalizedAssetSource {
        name: normalized.name,
        url: Some(format!(
            "https://github.com/{owner}/{repo}/{}/{git_ref}/{path}",
            normalized.mode.as_str(),
            git_ref = git_ref,
            path = normalized.path
        )),
        path: None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHubRepoSource {
    owner: String,
    repo: String,
    git_ref: String,
}

#[derive(Debug, Deserialize)]
struct GitHubCommit {
    sha: String,
}

fn github_repo_source(source: &str) -> Option<GitHubRepoSource> {
    let url = Url::parse(source).ok()?;
    let segments = source_segments(source);

    match url.host_str() {
        Some("github.com") if segments.len() >= 4 => {
            // segments[3..] contains the git ref and path; extract just the ref.
            let inner: Vec<&str> = segments[3..].iter().map(|s| s.as_str()).collect();
            let (git_ref, _) = split_git_ref_and_path_any_catalog(&inner);
            if git_ref.is_empty() {
                return None;
            }
            Some(GitHubRepoSource {
                owner: segments[0].clone(),
                repo: segments[1].clone(),
                git_ref,
            })
        }
        Some("raw.githubusercontent.com") if segments.len() >= 3 => Some(GitHubRepoSource {
            owner: segments[0].clone(),
            repo: segments[1].clone(),
            git_ref: segments[2].clone(),
        }),
        _ => None,
    }
}

async fn resolve_github_commit_sha(
    client: &reqwest::Client,
    token: Option<&str>,
    api_base: &str,
    owner: &str,
    repo: &str,
    git_ref: &str,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<String, CpmError> {
    let cache_key = format!("github-ref:{owner}/{repo}:{git_ref}");
    if let Some(cached) = read_cache_value(&cache_key, Duration::from_secs(300))? {
        return Ok(cached);
    }

    let url = format!(
        "{api_base}/repos/{owner}/{repo}/commits/{git_ref}",
        git_ref = git_ref
    );
    let target = rewrite_source_url(&url, source_rules);
    let mut request = client
        .get(&target.url)
        .header(header::USER_AGENT, env!("CARGO_PKG_NAME"));
    if let Some(token) = target.token.as_deref().or(token) {
        request = request.bearer_auth(token);
    }

    let response = request.send().await?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(CpmError::AuthRequired { url: target.url });
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(CpmError::InvalidSource {
            input: format!("https://github.com/{owner}/{repo} @ {git_ref}"),
            reason: "GitHub ref could not be resolved to a commit".to_owned(),
        });
    }

    let commit: GitHubCommit = response.error_for_status()?.json().await?;
    write_cache_value(&cache_key, &commit.sha)?;
    Ok(commit.sha)
}

#[derive(Debug, Deserialize)]
struct NpmRegistryDocument {
    #[serde(rename = "dist-tags")]
    dist_tags: NpmDistTags,
}

#[derive(Debug, Deserialize)]
struct NpmDistTags {
    latest: String,
}

#[derive(Debug, Deserialize)]
struct PypiProjectDocument {
    info: PypiInfo,
}

#[derive(Debug, Deserialize)]
struct PypiInfo {
    version: String,
}

async fn resolve_npm_version(
    client: &reqwest::Client,
    package: &str,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<String, CpmError> {
    let encoded = package.replace('/', "%2f");
    let url = format!("https://registry.npmjs.org/{encoded}");
    let target = rewrite_source_url(&url, source_rules);
    let document: NpmRegistryDocument = client
        .get(target.url)
        .header(header::USER_AGENT, env!("CARGO_PKG_NAME"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(document.dist_tags.latest)
}

async fn resolve_pypi_version(
    client: &reqwest::Client,
    package: &str,
    source_rules: &IndexMap<String, SourceRule>,
) -> Result<String, CpmError> {
    let url = format!("https://pypi.org/pypi/{package}/json");
    let target = rewrite_source_url(&url, source_rules);
    let response = client
        .get(target.url)
        .header(header::USER_AGENT, env!("CARGO_PKG_NAME"))
        .header(header::ACCEPT, "application/json")
        .send()
        .await?
        .error_for_status()?;
    let bytes = response.bytes().await?;
    if bytes.is_empty() {
        return Err(CpmError::InvalidSource {
            input: package.to_owned(),
            reason: "PyPI returned an empty JSON document while resolving the package version"
                .to_owned(),
        });
    }
    let document: PypiProjectDocument =
        serde_json::from_slice(&bytes).map_err(|err| CpmError::InvalidSource {
            input: package.to_owned(),
            reason: format!("PyPI metadata response was not valid JSON: {err}"),
        })?;
    Ok(document.info.version)
}

async fn resolve_cached_text<F, Fut>(key: &str, ttl: Duration, fetch: F) -> Result<String, CpmError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<String, CpmError>>,
{
    if let Some(cached) = read_cache_value(key, ttl)? {
        return Ok(cached);
    }
    let value = fetch().await?;
    write_cache_value(key, &value)?;
    Ok(value)
}

fn read_cache_value(key: &str, ttl: Duration) -> Result<Option<String>, CpmError> {
    let path = cache_path_for_key(key);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(CpmError::Io(err)),
    };
    let modified = metadata.modified()?;
    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    if age > ttl {
        return Ok(None);
    }
    Ok(Some(std::fs::read_to_string(path)?.trim().to_owned()))
}

fn write_cache_value(key: &str, value: &str) -> Result<(), CpmError> {
    let path = cache_path_for_key(key);
    atomic_write(&path, value.as_bytes())
}

fn cache_path_for_key(key: &str) -> std::path::PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let file = hex::encode(hasher.finalize());
    cache_dir().join("resolution").join(file)
}

fn normalize_generic_file_url(
    kind: AssetKind,
    source: &str,
    url: &Url,
) -> Result<NormalizedAssetSource, CpmError> {
    let path = url.path().trim_start_matches('/');
    let normalized = normalize_explicit_file_path(kind, source, path)?;

    Ok(NormalizedAssetSource {
        name: normalized.name,
        url: Some(source.to_owned()),
        path: None,
    })
}

fn normalize_path_source(kind: AssetKind, source: &str) -> Result<NormalizedAssetSource, CpmError> {
    let path = Path::new(source);
    if !path.exists() {
        return Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: "not a supported URL and local path does not exist".to_owned(),
        });
    }

    let normalized = if path.is_file() {
        normalize_explicit_file_path(kind, source, source)?
    } else {
        normalize_directory_path(kind, source, source)?
    };

    let normalized_path = Path::new(&normalized.path);
    let path_is_valid = match normalized.mode {
        GitHubMode::Blob => normalized_path.is_file(),
        GitHubMode::Tree => normalized_path.is_dir(),
    };
    if !path_is_valid {
        let expected = match normalized.mode {
            GitHubMode::Blob => "file",
            GitHubMode::Tree => "directory",
        };
        return Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: format!("expected local asset {expected} at `{}`", normalized.path),
        });
    }

    let utf8_path = Utf8PathBuf::from_path_buf(normalized_path.to_path_buf()).map_err(|_| {
        CpmError::InvalidSource {
            input: source.to_owned(),
            reason: "local paths must be valid UTF-8".to_owned(),
        }
    })?;

    Ok(NormalizedAssetSource {
        name: normalized.name,
        url: None,
        path: Some(utf8_path),
    })
}

fn normalize_repo_path(
    kind: AssetKind,
    source: &str,
    owner: &str,
    repo: &str,
    git_ref: &str,
    path: &str,
) -> Result<NormalizedAssetSource, CpmError> {
    let normalized = normalize_asset_path(kind, source, path)?;
    Ok(NormalizedAssetSource {
        name: normalized.name,
        url: Some(format!(
            "https://github.com/{owner}/{repo}/{}/{git_ref}/{path}",
            normalized.mode.as_str(),
            git_ref = git_ref,
            path = normalized.path
        )),
        path: None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedPath {
    name: String,
    path: String,
    mode: GitHubMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitHubMode {
    Blob,
    Tree,
}

impl GitHubMode {
    fn as_str(self) -> &'static str {
        match self {
            GitHubMode::Blob => "blob",
            GitHubMode::Tree => "tree",
        }
    }
}

fn normalize_asset_path(
    kind: AssetKind,
    source: &str,
    path: &str,
) -> Result<NormalizedPath, CpmError> {
    if path.is_empty() {
        return Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: format!(
                "points at a repository or catalog root; choose a specific {kind} instead"
            ),
        });
    }

    let trimmed = path.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);

    if last == plural_dir(kind) {
        return Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: format!(
                "points at the `{}` catalog; choose a specific asset under `{}/<name>`",
                plural_dir(kind),
                plural_dir(kind)
            ),
        });
    }

    if is_explicit_file_for_kind(kind, last) {
        return normalize_explicit_file_path(kind, source, trimmed);
    }

    normalize_directory_path(kind, source, trimmed)
}

fn normalize_directory_path(
    kind: AssetKind,
    source: &str,
    path: &str,
) -> Result<NormalizedPath, CpmError> {
    match kind {
        AssetKind::Skill | AssetKind::Plugin => Ok(NormalizedPath {
            name: path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(path)
                .to_owned(),
            path: path.trim_end_matches('/').to_owned(),
            mode: GitHubMode::Tree,
        }),
        AssetKind::Agent => Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: "agent sources must point to a specific `*.agent.md` file".to_owned(),
        }),
        AssetKind::Mcp => Ok(NormalizedPath {
            name: path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(path)
                .to_owned(),
            path: path.trim_end_matches('/').to_owned(),
            mode: GitHubMode::Tree,
        }),
        AssetKind::Hook => Ok(NormalizedPath {
            name: path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(path)
                .to_owned(),
            path: path.trim_end_matches('/').to_owned(),
            mode: GitHubMode::Tree,
        }),
        AssetKind::Workflow => Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: "workflow sources must point to a specific `*.md` file".to_owned(),
        }),
        AssetKind::Instruction => Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason:
                "instruction sources must point to a specific `*.md` or `*.instructions.md` file"
                    .to_owned(),
        }),
    }
}

fn normalize_explicit_file_path(
    kind: AssetKind,
    source: &str,
    path: &str,
) -> Result<NormalizedPath, CpmError> {
    let last = path.rsplit('/').next().unwrap_or(path);
    if !is_explicit_file_for_kind(kind, last) {
        return Err(CpmError::InvalidSource {
            input: source.to_owned(),
            reason: expected_file_hint(kind).to_owned(),
        });
    }

    match kind {
        AssetKind::Skill | AssetKind::Plugin => {
            let directory = path.rsplit_once('/').map(|(dir, _)| dir).ok_or_else(|| {
                CpmError::InvalidSource {
                    input: source.to_owned(),
                    reason: "asset file must live under a named directory".to_owned(),
                }
            })?;
            Ok(NormalizedPath {
                name: infer_name_from_file_path(kind, path)?,
                path: directory.to_owned(),
                mode: GitHubMode::Tree,
            })
        }
        AssetKind::Agent | AssetKind::Mcp => Ok(NormalizedPath {
            name: infer_name_from_file_path(kind, path)?,
            path: path.to_owned(),
            mode: GitHubMode::Blob,
        }),
        AssetKind::Hook => {
            let directory = path.rsplit_once('/').map(|(dir, _)| dir).ok_or_else(|| {
                CpmError::InvalidSource {
                    input: source.to_owned(),
                    reason: "hook files must live under a named directory".to_owned(),
                }
            })?;
            Ok(NormalizedPath {
                name: infer_name_from_file_path(kind, path)?,
                path: directory.to_owned(),
                mode: GitHubMode::Tree,
            })
        }
        AssetKind::Workflow => Ok(NormalizedPath {
            name: infer_name_from_file_path(kind, path)?,
            path: path.to_owned(),
            mode: GitHubMode::Blob,
        }),
        AssetKind::Instruction => Ok(NormalizedPath {
            name: infer_name_from_file_path(kind, path)?,
            path: path.to_owned(),
            mode: GitHubMode::Blob,
        }),
    }
}

fn infer_name_from_file_path(kind: AssetKind, path: &str) -> Result<String, CpmError> {
    let trimmed = path.trim_end_matches('/');
    let segments: Vec<_> = trimmed
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let last = segments.last().copied().unwrap_or_default();

    match kind {
        AssetKind::Skill | AssetKind::Plugin => segments
            .get(segments.len().saturating_sub(2))
            .map(|segment| (*segment).to_owned())
            .ok_or_else(|| CpmError::InvalidSource {
                input: path.to_owned(),
                reason: "asset file must live under a named directory".to_owned(),
            }),
        AssetKind::Agent => last
            .strip_suffix(".agent.md")
            .map(ToOwned::to_owned)
            .ok_or_else(|| CpmError::InvalidSource {
                input: path.to_owned(),
                reason: "agent files must end in `.agent.md`".to_owned(),
            }),
        AssetKind::Mcp => Ok(last.to_owned()),
        AssetKind::Hook => segments
            .get(segments.len().saturating_sub(2))
            .map(|segment| (*segment).to_owned())
            .ok_or_else(|| CpmError::InvalidSource {
                input: path.to_owned(),
                reason: "hook files must live under a named directory".to_owned(),
            }),
        AssetKind::Workflow => last
            .strip_suffix(".md")
            .map(ToOwned::to_owned)
            .ok_or_else(|| CpmError::InvalidSource {
                input: path.to_owned(),
                reason: "workflow files must end in `.md`".to_owned(),
            }),
        AssetKind::Instruction => {
            instruction_name_from_file_name(last).ok_or_else(|| CpmError::InvalidSource {
                input: path.to_owned(),
                reason: "instruction files must end in `.md` or `.instructions.md`".to_owned(),
            })
        }
    }
}

fn is_explicit_file_for_kind(kind: AssetKind, file_name: &str) -> bool {
    match kind {
        AssetKind::Skill => file_name == "SKILL.md",
        AssetKind::Plugin => file_name == "README.md",
        AssetKind::Agent => file_name.ends_with(".agent.md"),
        AssetKind::Mcp => true,
        AssetKind::Hook => file_name == "hooks.json",
        AssetKind::Workflow => file_name.ends_with(".md"),
        AssetKind::Instruction => file_name.ends_with(".md"),
    }
}

fn expected_file_hint(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Skill => {
            "skill sources must point to `SKILL.md` or a directory tree containing it"
        }
        AssetKind::Plugin => {
            "plugin sources must point to `README.md` or a directory tree containing it"
        }
        AssetKind::Agent => "agent sources must point to a specific `*.agent.md` file",
        AssetKind::Mcp => "MCP sources must point to a concrete launch target",
        AssetKind::Hook => {
            "hook sources must point to `hooks.json` or a directory tree containing it"
        }
        AssetKind::Workflow => "workflow sources must point to a specific `*.md` file",
        AssetKind::Instruction => {
            "instruction sources must point to a specific `*.md` or `*.instructions.md` file"
        }
    }
}

fn plural_dir(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Plugin => "plugins",
        AssetKind::Skill => "skills",
        AssetKind::Agent => "agents",
        AssetKind::Mcp => "mcps",
        AssetKind::Hook => "hooks",
        AssetKind::Workflow => "workflows",
        AssetKind::Instruction => "instructions",
    }
}

/// Known asset catalog directory names used to locate the split between the
/// git ref and the in-repo path in a GitHub URL.
const ASSET_CATALOG_DIRS: &[&str] = &[
    "plugins",
    "skills",
    "agents",
    "hooks",
    "workflows",
    "mcps",
    "instructions",
];

fn instruction_name_from_file_name(file_name: &str) -> Option<String> {
    file_name
        .strip_suffix(".instructions.md")
        .or_else(|| file_name.strip_suffix(".md"))
        .map(ToOwned::to_owned)
}

/// Split the path components that follow `owner/repo/mode/` in a GitHub URL
/// into `(git_ref, asset_path)`, using a specific catalog dir as the boundary.
///
/// The split point is the first segment equal to `catalog` that is **not** the
/// very first segment (so the git ref has at least one component).  Falls back
/// to `split_git_ref_and_path_any_catalog` when the catalog is not found.
fn split_git_ref_and_path_for_kind(segments: &[&str], catalog: &str) -> (String, String) {
    for (i, &seg) in segments.iter().enumerate() {
        if i >= 1 && seg == catalog {
            return (segments[..i].join("/"), segments[i..].join("/"));
        }
    }
    split_git_ref_and_path_any_catalog(segments)
}

/// Split the path components that follow `owner/repo/mode/` in a GitHub URL
/// into `(git_ref, asset_path)`, scanning for any known asset catalog dir.
///
/// The split point is the first segment matching a known catalog name that is
/// **not** the very first segment.  Falls back to single-segment-ref behaviour
/// when no catalog segment is found.
fn split_git_ref_and_path_any_catalog(segments: &[&str]) -> (String, String) {
    for (i, &seg) in segments.iter().enumerate() {
        if i >= 1 && ASSET_CATALOG_DIRS.contains(&seg) {
            return (segments[..i].join("/"), segments[i..].join("/"));
        }
    }
    // No catalog found: fall back to treating segments[0] as the full git ref.
    match segments {
        [] => (String::new(), String::new()),
        [single] => ((*single).to_owned(), String::new()),
        [first, rest @ ..] => ((*first).to_owned(), rest.join("/")),
    }
}

fn source_segments(source: &str) -> Vec<String> {
    if let Ok(url) = Url::parse(source) {
        return url
            .path_segments()
            .map(|segments| {
                segments
                    .filter(|segment| !segment.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
    }

    source
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn last_segment(source: &str) -> Option<String> {
    source_segments(source).into_iter().next_back()
}

fn is_full_commit_sha(candidate: &str) -> bool {
    candidate.len() == 40 && candidate.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn inference_segments(source: &str) -> Vec<String> {
    if let Ok(url) = Url::parse(source) {
        let segments = source_segments(source);
        return match url.host_str() {
            Some("github.com") if segments.len() >= 4 => {
                // Determine the asset path portion by stripping the git ref.
                let inner: Vec<&str> = segments[3..].iter().map(|s| s.as_str()).collect();
                let (_, path) = split_git_ref_and_path_any_catalog(&inner);
                if path.is_empty() {
                    Vec::new()
                } else {
                    path.split('/').map(ToOwned::to_owned).collect()
                }
            }
            Some("raw.githubusercontent.com") if segments.len() >= 3 => segments[3..].to_vec(),
            Some("github.com") | Some("raw.githubusercontent.com") => Vec::new(),
            _ => segments,
        };
    }

    source_segments(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[test]
    fn infers_kind_from_nested_skill_path() {
        let source = "https://github.com/github/awesome-copilot/tree/main/plugins/context-engineering/skills/aws-cdk-python-setup";
        assert_eq!(infer_kind_from_source(source), Some(AssetKind::Skill));
    }

    #[test]
    fn does_not_infer_kind_from_repo_name_alone() {
        let source = "https://github.com/anthropics/skills/tree/main/tooling/pdf";
        assert_eq!(infer_kind_from_source(source), None);
    }

    #[test]
    fn normalizes_skill_directory_to_tree_url() {
        let normalized = normalize_asset_source(
            AssetKind::Skill,
            "https://github.com/anthropics/skills/tree/main/skills/pdf",
        )
        .expect("normalize");

        assert_eq!(normalized.name, "pdf");
        assert_eq!(
            normalized.url.as_deref(),
            Some("https://github.com/anthropics/skills/tree/main/skills/pdf")
        );
    }

    #[test]
    fn normalizes_skill_file_to_tree_url() {
        let normalized = normalize_asset_source(
            AssetKind::Skill,
            "https://github.com/anthropics/skills/blob/main/skills/pdf/SKILL.md",
        )
        .expect("normalize");

        assert_eq!(normalized.name, "pdf");
        assert_eq!(
            normalized.url.as_deref(),
            Some("https://github.com/anthropics/skills/tree/main/skills/pdf")
        );
    }

    #[test]
    fn normalizes_plugin_directory_to_tree_url() {
        let normalized = normalize_asset_source(
            AssetKind::Plugin,
            "https://github.com/github/awesome-copilot/tree/main/plugins/cast-imaging",
        )
        .expect("normalize");

        assert_eq!(normalized.name, "cast-imaging");
        assert_eq!(
            normalized.url.as_deref(),
            Some("https://github.com/github/awesome-copilot/tree/main/plugins/cast-imaging")
        );
    }

    #[test]
    fn accepts_agent_markdown_file() {
        let normalized = normalize_asset_source(
            AssetKind::Agent,
            "https://github.com/github/awesome-copilot/blob/main/agents/4.1-Beast.agent.md",
        )
        .expect("normalize");

        assert_eq!(normalized.name, "4.1-Beast");
        assert_eq!(
            normalized.url.as_deref(),
            Some("https://github.com/github/awesome-copilot/blob/main/agents/4.1-Beast.agent.md")
        );
    }

    #[test]
    fn infers_instruction_kind_from_instructions_url() {
        let source =
            "https://github.com/github/awesome-copilot/blob/main/instructions/shell.instructions.md";
        assert_eq!(infer_kind_from_source(source), Some(AssetKind::Instruction));
    }

    #[test]
    fn normalizes_instruction_file_url() {
        let normalized = normalize_asset_source(
            AssetKind::Instruction,
            "https://github.com/github/awesome-copilot/blob/main/instructions/shell.instructions.md",
        )
        .expect("normalize");

        assert_eq!(normalized.name, "shell");
        assert_eq!(
            normalized.url.as_deref(),
            Some(
                "https://github.com/github/awesome-copilot/blob/main/instructions/shell.instructions.md",
            )
        );
    }

    #[test]
    fn rejects_collection_roots() {
        let err = normalize_asset_source(
            AssetKind::Plugin,
            "https://github.com/github/awesome-copilot/tree/main/plugins",
        )
        .expect_err("collection root should fail");

        assert!(matches!(err, CpmError::InvalidSource { .. }));
    }

    #[test]
    fn converts_raw_github_urls_to_canonical_tree_urls() {
        let normalized = normalize_asset_source(
            AssetKind::Skill,
            "https://raw.githubusercontent.com/anthropics/skills/main/skills/pdf/SKILL.md",
        )
        .expect("normalize");

        assert_eq!(
            normalized.url.as_deref(),
            Some("https://github.com/anthropics/skills/tree/main/skills/pdf")
        );
    }

    #[tokio::test]
    async fn resolves_github_url_ref_to_commit_sha() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/anthropics/skills/commits/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sha": "0123456789abcdef0123456789abcdef01234567"
            })))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let resolved = resolve_pinned_rev_with_api_base(
            &client,
            None,
            Some("https://github.com/anthropics/skills/tree/main/skills/pdf"),
            None,
            &server.uri(),
            &source_rules,
        )
        .await
        .expect("resolve");

        assert_eq!(
            resolved.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
    }

    #[tokio::test]
    async fn explicit_rev_override_is_resolved_to_commit_sha() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/github/awesome-copilot/commits/release-2025"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sha": "89abcdef0123456789abcdef0123456789abcdef"
            })))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let resolved = resolve_pinned_rev_with_api_base(
            &client,
            None,
            Some("https://github.com/github/awesome-copilot/tree/main/plugins/cast-imaging"),
            Some("release-2025"),
            &server.uri(),
            &source_rules,
        )
        .await
        .expect("resolve");

        assert_eq!(
            resolved.as_deref(),
            Some("89abcdef0123456789abcdef0123456789abcdef")
        );
    }

    #[tokio::test]
    async fn full_commit_sha_override_skips_resolution() {
        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let resolved = resolve_pinned_rev_with_api_base(
            &client,
            None,
            Some("https://github.com/anthropics/skills/tree/main/skills/pdf"),
            Some("0123456789abcdef0123456789abcdef01234567"),
            "http://127.0.0.1:9",
            &source_rules,
        )
        .await
        .expect("resolve");

        assert_eq!(
            resolved.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
    }

    #[test]
    fn docker_helpers_extract_name_and_stable_pin() {
        assert_eq!(
            docker_image_name("ghcr.io/github/github-mcp-server:1.2.3"),
            "github-mcp-server"
        );
        assert_eq!(
            docker_image_name("docker.io/library/postgres@sha256:deadbeef"),
            "postgres"
        );
        assert_eq!(
            docker_image_pin("ghcr.io/github/github-mcp-server:1.2.3"),
            Some("1.2.3".to_owned())
        );
        assert_eq!(
            docker_image_pin("ghcr.io/github/github-mcp-server@sha256:deadbeef"),
            Some("sha256:deadbeef".to_owned())
        );
        assert_eq!(
            docker_image_pin("ghcr.io/github/github-mcp-server:latest"),
            None
        );
        assert_eq!(
            docker_image_pin("localhost:5000/org/github-mcp-server"),
            None
        );
    }

    // ── Multi-segment git ref parsing ────────────────────────────────────────

    #[test]
    fn normalizes_skill_with_multi_segment_ref() {
        let normalized = normalize_asset_source(
            AssetKind::Skill,
            "https://github.com/owner/repo/tree/feature/foo/skills/bar",
        )
        .expect("normalize");

        assert_eq!(normalized.name, "bar");
        assert_eq!(
            normalized.url.as_deref(),
            Some("https://github.com/owner/repo/tree/feature/foo/skills/bar")
        );
    }

    #[test]
    fn normalizes_agent_with_multi_segment_ref() {
        let normalized = normalize_asset_source(
            AssetKind::Agent,
            "https://github.com/owner/repo/blob/release/2026/agents/reviewer.agent.md",
        )
        .expect("normalize");

        assert_eq!(normalized.name, "reviewer");
        assert_eq!(
            normalized.url.as_deref(),
            Some("https://github.com/owner/repo/blob/release/2026/agents/reviewer.agent.md")
        );
    }

    #[test]
    fn parse_github_source_multi_segment_ref() {
        let parsed =
            parse_github_source("https://github.com/owner/repo/tree/feature/foo/skills/bar")
                .expect("parse");

        assert_eq!(parsed.owner, "owner");
        assert_eq!(parsed.repo, "repo");
        assert_eq!(parsed.git_ref, "feature/foo");
        assert_eq!(parsed.path, "skills/bar");
        assert!(matches!(parsed.mode, GitHubSourceMode::Tree));
    }

    #[test]
    fn parse_github_source_single_segment_ref_unchanged() {
        let parsed =
            parse_github_source("https://github.com/anthropics/skills/tree/main/skills/pdf")
                .expect("parse");

        assert_eq!(parsed.git_ref, "main");
        assert_eq!(parsed.path, "skills/pdf");
    }

    #[tokio::test]
    async fn resolves_multi_segment_ref_to_commit_sha() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/commits/feature/foo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sha": "aabbccddaabbccddaabbccddaabbccddaabbccdd"
            })))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let resolved = resolve_pinned_rev_with_api_base(
            &client,
            None,
            Some("https://github.com/owner/repo/tree/feature/foo/skills/bar"),
            None,
            &server.uri(),
            &source_rules,
        )
        .await
        .expect("resolve");

        assert_eq!(
            resolved.as_deref(),
            Some("aabbccddaabbccddaabbccddaabbccddaabbccdd")
        );
    }

    // ── GitHub repo MCP runner inference ─────────────────────────────────────

    #[tokio::test]
    async fn infers_uvx_from_pyproject_toml() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/org/python-mcp/contents/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"name": "pyproject.toml", "type": "file"},
                {"name": "src", "type": "dir"},
                {"name": "README.md", "type": "file"},
            ])))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let runner = infer_runner_from_repo_root(
            &client,
            None,
            &server.uri(),
            "org",
            "python-mcp",
            &source_rules,
        )
        .await;

        assert_eq!(runner, Some(InferredMcpRunner::Uvx));
    }

    #[tokio::test]
    async fn infers_uvx_from_setup_py() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/org/py-mcp/contents/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"name": "setup.py", "type": "file"},
                {"name": "README.md", "type": "file"},
            ])))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let runner = infer_runner_from_repo_root(
            &client,
            None,
            &server.uri(),
            "org",
            "py-mcp",
            &source_rules,
        )
        .await;

        assert_eq!(runner, Some(InferredMcpRunner::Uvx));
    }

    #[tokio::test]
    async fn infers_npx_from_package_json() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/org/node-mcp/contents/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"name": "package.json", "type": "file"},
                {"name": "src", "type": "dir"},
            ])))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let runner = infer_runner_from_repo_root(
            &client,
            None,
            &server.uri(),
            "org",
            "node-mcp",
            &source_rules,
        )
        .await;

        assert_eq!(runner, Some(InferredMcpRunner::Npx));
    }

    #[tokio::test]
    async fn returns_none_for_ambiguous_repo() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/org/ambiguous/contents/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"name": "README.md", "type": "file"},
                {"name": "Makefile", "type": "file"},
            ])))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let runner = infer_runner_from_repo_root(
            &client,
            None,
            &server.uri(),
            "org",
            "ambiguous",
            &source_rules,
        )
        .await;

        assert_eq!(runner, None);
    }

    #[tokio::test]
    async fn pyproject_toml_takes_precedence_over_package_json() {
        // A repo with both Python and npm files: Python wins.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/org/multi-lang/contents/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"name": "pyproject.toml", "type": "file"},
                {"name": "package.json", "type": "file"},
            ])))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let source_rules = IndexMap::new();
        let runner = infer_runner_from_repo_root(
            &client,
            None,
            &server.uri(),
            "org",
            "multi-lang",
            &source_rules,
        )
        .await;

        assert_eq!(runner, Some(InferredMcpRunner::Uvx));
    }

    #[test]
    fn infer_github_repo_mcp_runner_ignores_tree_and_blob_urls() {
        // This test is sync: non-bare GitHub URLs must return None without any API call.
        // We just verify the URL classification guards work.
        let source_rules = IndexMap::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::new();
        let result = rt.block_on(infer_github_repo_mcp_runner(
            &client,
            None,
            "https://github.com/owner/repo/tree/main/skills/bar",
            &source_rules,
        ));
        // Tree URL has ≥3 segments → should return None immediately.
        assert_eq!(result, None, "tree URL must not be probed");
    }
}
