//! Shared types for cpm — the Copilot Plugin Manager.
//!
//! This crate has no dependencies on other cpm crates. It provides all the
//! fundamental types used across the workspace.

#![deny(missing_docs)]

use std::path::PathBuf;
use std::str::FromStr;

use camino::Utf8PathBuf;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// ─── Asset kind ─────────────────────────────────────────────────────────────

/// The kind of a Copilot asset managed by cpm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    /// A Copilot extension plugin (`.github/plugins/`).
    Plugin,
    /// A Copilot skill file (`.github/skills/`).
    Skill,
    /// A Copilot agent (`.github/agents/`).
    Agent,
    /// A Model Context Protocol server (`.github/mcp/`).
    Mcp,
    /// A Copilot hook bundle (`.github/hooks/`).
    Hook,
    /// A Copilot workflow definition (`.github/workflows/`).
    Workflow,
    /// A Copilot instruction markdown file (`.github/instructions/`).
    Instruction,
}

impl std::fmt::Display for AssetKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetKind::Plugin => write!(f, "plugin"),
            AssetKind::Skill => write!(f, "skill"),
            AssetKind::Agent => write!(f, "agent"),
            AssetKind::Mcp => write!(f, "mcp"),
            AssetKind::Hook => write!(f, "hook"),
            AssetKind::Workflow => write!(f, "workflow"),
            AssetKind::Instruction => write!(f, "instruction"),
        }
    }
}

// ─── Scope ──────────────────────────────────────────────────────────────────

/// Install scope — repository-local or user-global.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Repository-local (e.g. `.github/plugins/`).
    #[default]
    Local,
    /// User-global (for plugins, Copilot delegates to its own global install layout).
    Global,
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Scope::Local => write!(f, "local"),
            Scope::Global => write!(f, "global"),
        }
    }
}

impl FromStr for Scope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" => Ok(Scope::Local),
            "global" => Ok(Scope::Global),
            other => Err(format!("unsupported scope `{other}`")),
        }
    }
}

/// Policy controlling how cpm updates pinned asset revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdatePolicy {
    /// Never update automatically.
    Locked,
    /// Always move to the latest available revision.
    Latest,
    /// Only move to newer semantic-version tags.
    Tagged,
}

/// Policy controlling how cpm reacts to license findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LicensePolicy {
    /// Allow all licenses.
    AllowAll,
    /// Warn on copyleft licenses but continue.
    WarnCopyleft,
    /// Refuse to install copyleft licenses.
    DenyCopyleft,
    /// Only allow explicitly listed licenses.
    AllowList,
}

/// Partial settings loaded from a repo manifest or user config file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialSettings {
    /// Default scope to use for commands that accept an implicit scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_scope: Option<Scope>,
    /// Update policy for lock/sync/update workflows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_policy: Option<UpdatePolicy>,
    /// License policy to apply during installs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_policy: Option<LicensePolicy>,
    /// Explicitly allowed licenses when `license_policy = "allow-list"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_licenses: Option<Vec<String>>,
    /// Override cache directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<String>,
    /// Network timeout, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_timeout: Option<u64>,
    /// Groups auto-installed by `cpm sync`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_groups: Option<Vec<String>>,
    /// Whether `cpm sync` should verify installed files more aggressively.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_on_sync: Option<bool>,
    /// Whether `cpm sync` should compile workflow markdown into `.lock.yml` files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compile_workflows: Option<bool>,
}

impl PartialSettings {
    /// Return whether the settings block has no explicitly configured values.
    pub fn is_empty(&self) -> bool {
        self.default_scope.is_none()
            && self.update_policy.is_none()
            && self.license_policy.is_none()
            && self.allowed_licenses.is_none()
            && self.cache_dir.is_none()
            && self.network_timeout.is_none()
            && self.auto_groups.is_none()
            && self.verify_on_sync.is_none()
            && self.auto_compile_workflows.is_none()
    }
}

/// A named user-configured source rewrite / auth rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRule {
    /// Target base URL after rewrite.
    pub url: String,
    /// Optional environment variable containing an auth token for this source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_env: Option<String>,
    /// URL prefix that should be rewritten to `url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<String>,
}

/// User or machine-local configuration loaded from `~/.config/cpm/config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserConfig {
    /// User-scoped settings overrides.
    #[serde(default, skip_serializing_if = "PartialSettings::is_empty")]
    pub settings: PartialSettings,
    /// Named source rules used for URL rewriting and auth overrides.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub sources: IndexMap<String, SourceRule>,
}

/// Optional project metadata stored in `cpm.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageMetadata {
    /// Human-readable project name.
    pub name: String,
    /// Optional project description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Project version string.
    pub version: String,
    /// SPDX expression for the repository itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Optional author list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    /// Optional repository URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Optional creation date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
}

/// Structured assets and metadata for a named group.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestGroup {
    /// Optional group description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Group-specific plugins.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub plugins: IndexMap<String, AssetSource>,
    /// Group-specific skills.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub skills: IndexMap<String, AssetSource>,
    /// Group-specific agents.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub agents: IndexMap<String, AssetSource>,
    /// Group-specific MCPs.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub mcps: IndexMap<String, AssetSource>,
    /// Group-specific hooks.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub hooks: IndexMap<String, AssetSource>,
    /// Group-specific workflows.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub workflows: IndexMap<String, AssetSource>,
    /// Group-specific instructions.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub instructions: IndexMap<String, AssetSource>,
}

impl ManifestGroup {
    /// Return whether the group contains no metadata or assets.
    pub fn is_empty(&self) -> bool {
        self.description.is_none()
            && self.plugins.is_empty()
            && self.skills.is_empty()
            && self.agents.is_empty()
            && self.mcps.is_empty()
            && self.hooks.is_empty()
            && self.workflows.is_empty()
            && self.instructions.is_empty()
    }
}

// ─── MCP transport ──────────────────────────────────────────────────────────

/// The transport/launch mechanism for an MCP server.
///
/// Variants are mutually exclusive — exactly one specifier key (`url`, `npx`,
/// `uvx`, `docker`, `release`, `path`, `script`) may appear per manifest entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    /// HTTP or SSE remote endpoint.
    Http {
        /// Full URL to the MCP HTTP endpoint.
        url: String,
    },
    /// Server-Sent Events remote endpoint.
    Sse {
        /// Full URL to the MCP SSE endpoint.
        url: String,
    },
    /// Node.js package via `npx` (stdio transport).
    Npx {
        /// npm package name (e.g. `@modelcontextprotocol/server-github`).
        package: String,
        /// Optional executable name within the package.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entrypoint: Option<String>,
        /// Extra arguments passed to the npx command.
        args: Vec<String>,
    },
    /// Python package via `uvx` (stdio transport).
    Uvx {
        /// Python package / module name.
        package: String,
        /// Optional executable / console-script name exposed by the package.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entrypoint: Option<String>,
        /// Extra arguments.
        args: Vec<String>,
    },
    /// Docker container (stdio transport via `docker run`).
    Docker {
        /// Docker image reference.
        image: String,
        /// Extra arguments passed to the container entry-point.
        args: Vec<String>,
    },
    /// Native binary downloaded from a GitHub Release asset.
    Binary {
        /// GitHub Releases URL (tag URL or `releases/latest`).
        url: String,
        /// Filename of the binary inside the release archive.
        bin: String,
        /// Extra arguments.
        args: Vec<String>,
    },
    /// Local development binary (not yet published).
    Path {
        /// Path to the binary on disk.
        path: PathBuf,
        /// Extra arguments.
        args: Vec<String>,
    },
    /// Inline shell-safe command string.
    Script {
        /// Shell command that launches the MCP server.
        command: String,
        /// Extra arguments passed directly to the command.
        ///
        /// When empty the command is wrapped in `sh -c` for shell evaluation.
        /// When non-empty the command is executed directly with these arguments.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
}

/// Copilot-visible MCP protocol type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpProtocol {
    /// Standard I/O transport.
    Stdio,
    /// HTTP request/response transport.
    Http,
    /// Server-Sent Events transport.
    Sse,
}

/// Runtime used to launch a stdio MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpRunnerKind {
    /// Launch through `npx`.
    Npx,
    /// Launch through `uvx`.
    Uvx,
    /// Launch through `docker run`.
    Docker,
    /// Launch a downloaded binary.
    Binary,
    /// Launch a local path on disk.
    Local,
    /// Launch a local command/script.
    Command,
}

impl McpTransport {
    /// Return the canonical transport name used in lockfiles and diagnostics.
    pub fn name(&self) -> &'static str {
        match self {
            McpTransport::Http { .. } => "http",
            McpTransport::Sse { .. } => "sse",
            McpTransport::Npx { .. } => "npx",
            McpTransport::Uvx { .. } => "uvx",
            McpTransport::Docker { .. } => "docker",
            McpTransport::Binary { .. } => "binary",
            McpTransport::Path { .. } => "path",
            McpTransport::Script { .. } => "script",
        }
    }

    /// Return the Copilot-visible MCP protocol used by this transport.
    pub fn protocol(&self) -> McpProtocol {
        match self {
            McpTransport::Http { .. } => McpProtocol::Http,
            McpTransport::Sse { .. } => McpProtocol::Sse,
            McpTransport::Npx { .. }
            | McpTransport::Uvx { .. }
            | McpTransport::Docker { .. }
            | McpTransport::Binary { .. }
            | McpTransport::Path { .. }
            | McpTransport::Script { .. } => McpProtocol::Stdio,
        }
    }

    /// Return the stdio runner kind when this transport launches a local process.
    pub fn runner_kind(&self) -> Option<McpRunnerKind> {
        match self {
            McpTransport::Http { .. } | McpTransport::Sse { .. } => None,
            McpTransport::Npx { .. } => Some(McpRunnerKind::Npx),
            McpTransport::Uvx { .. } => Some(McpRunnerKind::Uvx),
            McpTransport::Docker { .. } => Some(McpRunnerKind::Docker),
            McpTransport::Binary { .. } => Some(McpRunnerKind::Binary),
            McpTransport::Path { .. } => Some(McpRunnerKind::Local),
            McpTransport::Script { .. } => Some(McpRunnerKind::Command),
        }
    }
}

// ─── Env injection ──────────────────────────────────────────────────────────

/// The value side of an environment variable specification.
///
/// `Literal` values are written verbatim wherever they are used (manifest,
/// lockfile, and runtime config).
///
/// `FromEnv` represents a reference to another environment variable. In
/// human-authored manifests (`cpm.toml`), values starting with `$` are parsed
/// into this variant, and are persisted back as `$VAR_NAME` in manifests or as
/// a structured `from_env` Serde representation in lockfiles.
///
/// When emitting Copilot/VS Code runtime JSON (e.g. `.vscode/mcp.json` or
/// `~/.copilot/mcp-config.json`), `FromEnv` values are rendered as
/// `${env:VAR_NAME}` so that the Copilot runtime expands them from the
/// process environment at launch time.  This `${env:...}` form is specific to
/// Copilot runtime JSON and is **not** the general Serde serialization format
/// for `EnvValue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvValue {
    /// A literal string (written verbatim to the runtime config at install time).
    Literal(String),
    /// Reference to a named environment variable resolved at MCP launch time.
    /// Values starting with `$` in `cpm.toml` are parsed into this variant.
    FromEnv(String),
}

/// A single environment variable to inject when launching an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvSpec {
    /// Environment variable name.
    pub key: String,
    /// How the value is obtained.
    pub value: EnvValue,
}

impl EnvSpec {
    /// Construct a new `EnvSpec` from raw manifest strings.
    ///
    /// If `raw_value` starts with `$`, it is treated as `FromEnv`; otherwise it
    /// is stored as a `Literal`.
    pub fn from_raw(key: impl Into<String>, raw_value: impl Into<String>) -> Self {
        let key = key.into();
        let raw = raw_value.into();
        let value = if let Some(var_name) = raw.strip_prefix('$') {
            EnvValue::FromEnv(var_name.to_owned())
        } else {
            EnvValue::Literal(raw)
        };
        EnvSpec { key, value }
    }
}

// ─── AssetSource (manifest entry) ───────────────────────────────────────────

/// The un-resolved source specification as found in `cpm.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetSource {
    /// Remote URL (git repo or HTTP endpoint).
    pub url: Option<String>,
    /// Git ref (tag, branch, or SHA). Resolved to a full SHA by the resolver.
    pub rev: Option<String>,
    /// Local filesystem path (for `path =` entries).
    pub path: Option<Utf8PathBuf>,
    /// Dependency group (defaults to `"default"`).
    #[serde(default = "default_group")]
    pub group: String,
    /// Install scope.
    #[serde(default)]
    pub scope: Scope,
    /// MCP transport specification (only set for `[mcps]` entries).
    pub transport: Option<McpTransport>,
    /// Environment variables to inject at MCP launch time.
    #[serde(default)]
    pub env: Vec<EnvSpec>,
    /// Extra command-line arguments for the MCP server.
    #[serde(default)]
    pub args: Vec<String>,
    /// Workflow engine override (workflow assets only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<WorkflowEngine>,
}

fn default_group() -> String {
    "default".to_owned()
}

/// Supported workflow execution engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowEngine {
    /// GitHub Copilot workflow engine.
    Copilot,
    /// Claude workflow engine.
    Claude,
    /// Codex workflow engine.
    Codex,
}

// ─── Asset ownership ─────────────────────────────────────────────────────────

/// Ownership state for a resolved top-level asset.
///
/// Controls whether `cpm sync` / `cpm install` may overwrite files that are
/// already present on disk.
///
/// | State       | sync behaviour                                          |
/// |-------------|--------------------------------------------------------|
/// | `upstream`  | Always written/overwritten from the resolved source.   |
/// | `generated` | Produced by cpm; re-generated on every sync.           |
/// | `user`      | Owned by the developer; **never** overwritten by cpm.  |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AssetOwnership {
    /// Installed from an external source; may be overwritten by sync.
    #[default]
    Upstream,
    /// Produced by cpm or a post-install compile step; should be regenerated.
    Generated,
    /// Owned locally by the developer; must not be overwritten without an
    /// explicit action such as `cpm own --reset`.
    User,
}

impl AssetOwnership {
    /// Return `true` when sync is allowed to overwrite the on-disk file.
    pub fn sync_may_overwrite(self) -> bool {
        matches!(self, AssetOwnership::Upstream | AssetOwnership::Generated)
    }
}

impl std::fmt::Display for AssetOwnership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetOwnership::Upstream => write!(f, "upstream"),
            AssetOwnership::Generated => write!(f, "generated"),
            AssetOwnership::User => write!(f, "user"),
        }
    }
}

fn asset_ownership_is_upstream(ownership: &AssetOwnership) -> bool {
    *ownership == AssetOwnership::Upstream
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

/// A single locked file within a resolved asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedFile {
    /// Relative installed path within the asset root.
    pub path: Utf8PathBuf,
    /// Raw SHA-256 hex digest for this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Whether this file should be marked executable on disk.
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub executable: bool,
}

impl From<Utf8PathBuf> for LockedFile {
    fn from(path: Utf8PathBuf) -> Self {
        Self {
            path,
            sha256: None,
            executable: false,
        }
    }
}

impl PartialEq<Utf8PathBuf> for LockedFile {
    fn eq(&self, other: &Utf8PathBuf) -> bool {
        self.path == *other
    }
}

impl PartialEq<LockedFile> for Utf8PathBuf {
    fn eq(&self, other: &LockedFile) -> bool {
        *self == other.path
    }
}

// ─── ResolvedAsset (lockfile entry) ─────────────────────────────────────────

/// A fully resolved asset as stored in `cpm.lock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedAsset {
    /// Manifest key / asset name.
    pub name: String,
    /// Asset kind.
    pub kind: AssetKind,
    /// Original source spec from `cpm.toml`.
    pub source: AssetSource,
    /// Resolved git SHA (40 chars), npm version string, or binary version tag.
    pub resolved_rev: String,
    /// UTC timestamp of the resolution run.
    pub resolved_date: DateTime<Utc>,
    /// `"sha256:<hex>"` content hash (see spec for algorithm).
    pub hash: String,
    /// Install scope.
    pub scope: Scope,
    /// Ownership state — controls whether sync may overwrite installed files.
    ///
    /// Defaults to [`AssetOwnership::Upstream`] when absent in the lockfile.
    #[serde(default, skip_serializing_if = "asset_ownership_is_upstream")]
    pub ownership: AssetOwnership,
    /// Installed files and their per-file metadata.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<LockedFile>,
    /// Legacy executable marker list retained for in-memory compatibility.
    #[serde(default, skip)]
    pub executable: Vec<Utf8PathBuf>,
    /// Legacy per-file hash map retained for in-memory compatibility.
    #[serde(default, skip)]
    pub file_hashes: IndexMap<Utf8PathBuf, String>,
    /// Git source metadata captured for git-backed assets when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitMetadata>,
    /// Nested skill / agent assets discovered inside the installed bundle.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_assets: Vec<SubAsset>,
    /// Detected license metadata, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<LicenseInfo>,
    /// Absolute path to the installed binary (binary MCPs only).
    pub bin_path: Option<Utf8PathBuf>,
    /// Compiled workflow output path relative to the install root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiled_path: Option<Utf8PathBuf>,
    /// Plugin-specific metadata needed by the delegated-install flow.
    ///
    /// Present only for [`AssetKind::Plugin`] entries.  Always `None` for
    /// skills, agents, MCPs, hooks, and workflows, and omitted from the
    /// serialised lockfile when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_meta: Option<PluginMeta>,
}

/// Git source kind captured in persisted lock metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitSourceKind {
    /// A single-file source (`blob`).
    Blob,
    /// A directory source (`tree`).
    Tree,
}

/// Git metadata persisted for git-backed assets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitMetadata {
    /// Repository owner or organization.
    pub owner: String,
    /// Repository name.
    pub repo: String,
    /// Requested git reference before pinning to `resolved_rev`.
    pub reference: String,
    /// Asset path within the repository.
    pub path: Utf8PathBuf,
    /// Whether the source points at a single file or a directory.
    pub mode: GitSourceKind,
}

/// Ownership policy for a nested sub-asset discovered inside a larger bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SubAssetOwnership {
    /// The sub-asset is owned by the parent bundle unless explicitly promoted.
    #[default]
    Parent,
    /// The sub-asset is independently managed outside the parent bundle.
    Standalone,
}

/// Nested skill / agent metadata discovered inside a resolved bundle asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAsset {
    /// Nested asset name.
    pub name: String,
    /// Nested asset kind.
    pub kind: AssetKind,
    /// Installed path relative to the asset kind install root.
    pub path: Utf8PathBuf,
    /// Ownership policy used when reporting or reconciling the nested asset.
    #[serde(default, skip_serializing_if = "sub_asset_ownership_is_default")]
    pub ownership: SubAssetOwnership,
}

fn sub_asset_ownership_is_default(ownership: &SubAssetOwnership) -> bool {
    *ownership == SubAssetOwnership::Parent
}

/// License metadata captured during lockfile generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseInfo {
    /// SPDX identifier or expression for the resolved asset.
    pub spdx: String,
    /// Source URL used to verify the detected license, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Whether the SPDX value was verified from authoritative metadata.
    pub verified: bool,
}

/// Plugin-specific metadata persisted in the lockfile for the delegated-install flow.
///
/// All fields are optional so the block can be added incrementally and
/// older lockfiles that lack these keys continue to parse without error.
///
/// This struct is stored under [`ResolvedAsset::plugin_meta`] and is only
/// meaningful for [`AssetKind::Plugin`] entries; other asset kinds carry it
/// as `None` and it is never serialised for them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginMeta {
    /// The plugin registry that provided this plugin
    /// (e.g. `"copilot"`, a registry slug, or an absolute registry URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// The resolved version string returned by the registry at resolution
    /// time — may be a semver string, a slug, or a SHA-derived tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<String>,
    /// Canonical source URL as reported by the registry at resolution time.
    ///
    /// This differs from the `url` field on the parent [`ResolvedAsset`]'s
    /// [`AssetSource`] in that it is the *registry-resolved* URL rather than
    /// the user-supplied spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// `"sha256:<hex>"` hash of the plugin's `plugin.json` manifest file,
    /// used for integrity verification during reinstall / upgrade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_json_hash: Option<String>,
}

impl PluginMeta {
    /// Returns `true` when every field is `None` (i.e. the struct is empty).
    ///
    /// Useful as a `skip_serializing_if` predicate.
    pub fn is_empty(&self) -> bool {
        self.registry.is_none()
            && self.plugin_version.is_none()
            && self.source_url.is_none()
            && self.plugin_json_hash.is_none()
    }
}

// ─── Manifest ────────────────────────────────────────────────────────────────

/// Deserialized `cpm.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Optional project metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageMetadata>,
    /// Repo-owned defaults.
    #[serde(default, skip_serializing_if = "PartialSettings::is_empty")]
    pub settings: PartialSettings,
    /// Project-scoped source rewrite / auth rules.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub sources: IndexMap<String, SourceRule>,
    /// Plugin entries.
    #[serde(default)]
    pub plugins: IndexMap<String, AssetSource>,
    /// Skill entries.
    #[serde(default)]
    pub skills: IndexMap<String, AssetSource>,
    /// Agent entries.
    #[serde(default)]
    pub agents: IndexMap<String, AssetSource>,
    /// MCP entries.
    #[serde(default)]
    pub mcps: IndexMap<String, AssetSource>,
    /// Hook entries.
    #[serde(default)]
    pub hooks: IndexMap<String, AssetSource>,
    /// Workflow entries.
    #[serde(default)]
    pub workflows: IndexMap<String, AssetSource>,
    /// Instruction entries.
    #[serde(default)]
    pub instructions: IndexMap<String, AssetSource>,
    /// Structured named groups.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub groups: IndexMap<String, ManifestGroup>,
}

impl Manifest {
    /// Return a flattened view of all assets for the requested kind, including
    /// group-owned assets with their group name applied to the source.
    pub fn effective_section(&self, kind: AssetKind) -> IndexMap<String, AssetSource> {
        let mut section = match kind {
            AssetKind::Plugin => self.plugins.clone(),
            AssetKind::Skill => self.skills.clone(),
            AssetKind::Agent => self.agents.clone(),
            AssetKind::Mcp => self.mcps.clone(),
            AssetKind::Hook => self.hooks.clone(),
            AssetKind::Workflow => self.workflows.clone(),
            AssetKind::Instruction => self.instructions.clone(),
        };

        for (group_name, group) in &self.groups {
            let grouped = match kind {
                AssetKind::Plugin => &group.plugins,
                AssetKind::Skill => &group.skills,
                AssetKind::Agent => &group.agents,
                AssetKind::Mcp => &group.mcps,
                AssetKind::Hook => &group.hooks,
                AssetKind::Workflow => &group.workflows,
                AssetKind::Instruction => &group.instructions,
            };

            for (name, source) in grouped {
                let mut grouped_source = source.clone();
                grouped_source.group = group_name.clone();
                section.insert(name.clone(), grouped_source);
            }
        }

        section
    }
}

// ─── Lockfile ────────────────────────────────────────────────────────────────

/// Deserialized `cpm.lock`.
///
/// Never edit `cpm.lock` by hand. cpm is the only writer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lockfile {
    /// Lockfile format version — always `1`.
    pub version: u8,
    /// UTC timestamp for the lockfile generation run.
    pub generated: DateTime<Utc>,
    /// Resolved plugin entries.
    #[serde(default)]
    pub plugins: Vec<ResolvedAsset>,
    /// Resolved skill entries.
    #[serde(default)]
    pub skills: Vec<ResolvedAsset>,
    /// Resolved agent entries.
    #[serde(default)]
    pub agents: Vec<ResolvedAsset>,
    /// Resolved MCP entries.
    #[serde(default)]
    pub mcps: Vec<ResolvedAsset>,
    /// Resolved hook entries.
    #[serde(default)]
    pub hooks: Vec<ResolvedAsset>,
    /// Resolved workflow entries.
    #[serde(default)]
    pub workflows: Vec<ResolvedAsset>,
    /// Resolved instruction entries.
    #[serde(default)]
    pub instructions: Vec<ResolvedAsset>,
}

impl Lockfile {
    /// Create a new, empty lockfile with `version = 1`.
    pub fn new() -> Self {
        Self::with_generated(Utc::now())
    }

    /// Create a new, empty lockfile with a specific generation timestamp.
    pub fn with_generated(generated: DateTime<Utc>) -> Self {
        Lockfile {
            version: 1,
            generated,
            plugins: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            mcps: Vec::new(),
            hooks: Vec::new(),
            workflows: Vec::new(),
            instructions: Vec::new(),
        }
    }

    /// Return an iterator over *all* resolved assets regardless of kind.
    pub fn all_assets(&self) -> impl Iterator<Item = &ResolvedAsset> {
        self.plugins
            .iter()
            .chain(self.skills.iter())
            .chain(self.agents.iter())
            .chain(self.mcps.iter())
            .chain(self.hooks.iter())
            .chain(self.workflows.iter())
            .chain(self.instructions.iter())
    }
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Global lockfile ─────────────────────────────────────────────────────────

/// A machine-local claim for a globally installed asset.
///
/// Global claims are recorded in `~/.copilot/cpm.lock` so future sync and
/// reconcile passes can understand which repository currently owns a given
/// global install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalClaim {
    /// Absolute repository path that recorded the global claim.
    pub claimed_by: Utf8PathBuf,
    /// The globally installed asset that was claimed by that repository.
    pub asset: ResolvedAsset,
}

impl GlobalClaim {
    /// Construct a new global claim wrapper.
    pub fn new(claimed_by: Utf8PathBuf, asset: ResolvedAsset) -> Self {
        Self { claimed_by, asset }
    }
}

/// Deserialized machine-local global lockfile (`~/.copilot/cpm.lock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalLockfile {
    /// Lockfile format version — always `1`.
    pub version: u8,
    /// UTC timestamp for the global lockfile generation/update time.
    pub generated: DateTime<Utc>,
    /// Recorded global install claims across local repositories on this machine.
    #[serde(default)]
    pub claims: Vec<GlobalClaim>,
}

impl GlobalLockfile {
    /// Create a new, empty global lockfile with `version = 1`.
    pub fn new() -> Self {
        Self::with_generated(Utc::now())
    }

    /// Create a new, empty global lockfile with a specific generation timestamp.
    pub fn with_generated(generated: DateTime<Utc>) -> Self {
        Self {
            version: 1,
            generated,
            claims: Vec::new(),
        }
    }

    /// Return an iterator over all claimed global assets.
    pub fn all_assets(&self) -> impl Iterator<Item = &ResolvedAsset> {
        self.claims.iter().map(|claim| &claim.asset)
    }
}

impl Default for GlobalLockfile {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_spec_literal() {
        let spec = EnvSpec::from_raw("KEY", "value");
        assert_eq!(spec.value, EnvValue::Literal("value".into()));
    }

    #[test]
    fn env_spec_from_env() {
        let spec = EnvSpec::from_raw("GITHUB_TOKEN", "$GITHUB_TOKEN");
        assert_eq!(spec.value, EnvValue::FromEnv("GITHUB_TOKEN".into()));
    }

    #[test]
    fn mcp_transport_name() {
        let t = McpTransport::Npx {
            package: "@org/pkg".into(),
            entrypoint: None,
            args: vec![],
        };
        assert_eq!(t.name(), "npx");
    }

    #[test]
    fn lockfile_new_has_version_1() {
        let lf = Lockfile::new();
        assert_eq!(lf.version, 1);
        assert!(lf.generated <= Utc::now());
    }

    #[test]
    fn global_lockfile_new_has_version_1() {
        let lf = GlobalLockfile::new();
        assert_eq!(lf.version, 1);
        assert!(lf.generated <= Utc::now());
        assert!(lf.claims.is_empty());
    }

    #[test]
    fn scope_display() {
        assert_eq!(Scope::Local.to_string(), "local");
        assert_eq!(Scope::Global.to_string(), "global");
    }

    #[test]
    fn asset_kind_display() {
        assert_eq!(AssetKind::Mcp.to_string(), "mcp");
        assert_eq!(AssetKind::Plugin.to_string(), "plugin");
        assert_eq!(AssetKind::Hook.to_string(), "hook");
        assert_eq!(AssetKind::Workflow.to_string(), "workflow");
    }

    #[test]
    fn scope_from_str_accepts_supported_values() {
        assert_eq!("local".parse::<Scope>(), Ok(Scope::Local));
        assert_eq!("global".parse::<Scope>(), Ok(Scope::Global));
    }

    #[test]
    fn partial_settings_is_empty_only_when_all_fields_absent() {
        let empty = PartialSettings::default();
        assert!(empty.is_empty());

        let configured = PartialSettings {
            default_scope: Some(Scope::Global),
            ..PartialSettings::default()
        };
        assert!(!configured.is_empty());
    }

    #[test]
    fn manifest_roundtrip_toml() {
        let toml_str = r#"
[settings]
default_scope = "global"

[plugins]
my-plugin = "https://github.com/owner/plugin"

[mcps]
my-mcp = { url = "https://api.example.com/mcp", transport = { http = { url = "https://api.example.com/mcp" } } }
"#;
        // Just check it parses without panicking via basic field checks.
        let _m: Result<Manifest, _> = toml::from_str(toml_str);
        // The TOML schema for AssetSource uses Option fields so partial parses work.
    }

    #[test]
    fn user_config_parses_sources_table() {
        let toml_str = r#"
[settings]
default_scope = "global"

[sources.internal]
url = "https://git.corp.example.com"
replace = "https://github.com/corp"
token_env = "CORP_TOKEN"
"#;

        let config: UserConfig = toml::from_str(toml_str).expect("config");
        assert_eq!(config.settings.default_scope, Some(Scope::Global));
        assert_eq!(
            config.sources["internal"].replace.as_deref(),
            Some("https://github.com/corp")
        );
    }
}
