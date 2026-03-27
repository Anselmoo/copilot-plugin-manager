//! Error types for cpm-core.

use cpm_types::AssetKind;
use miette::Diagnostic;
use thiserror::Error;

/// All errors that cpm-core can produce.
#[derive(Debug, Error, Diagnostic)]
pub enum CpmError {
    /// A network-level failure (DNS, TLS, timeouts, …).
    #[error("network error: {0}")]
    #[diagnostic(code(cpm::network), help("Check your internet connection"))]
    Network(#[from] reqwest::Error),

    /// A parse error in a TOML manifest or lockfile.
    #[error("parse error in {file}: {msg}")]
    #[diagnostic(code(cpm::parse))]
    Parse {
        /// The file that failed to parse.
        file: String,
        /// Human-readable description of the problem.
        msg: String,
    },

    /// A content hash mismatch detected by `cpm doctor`.
    #[error("hash mismatch for {name}: expected {expected}, got {actual}")]
    #[diagnostic(
        code(cpm::hash_mismatch),
        help("Run `cpm doctor` to find all mismatches, then `cpm sync` to repair")
    )]
    HashMismatch {
        /// Asset name.
        name: String,
        /// Expected hash from the lockfile.
        expected: String,
        /// Actual hash computed from disk.
        actual: String,
    },

    /// Same name + kind in both `local` and `global` scope.
    #[error("scope conflict: {name} ({kind}) exists in both local and global")]
    #[diagnostic(
        code(cpm::scope_conflict),
        help(
            "Run `cpm demote {name} --{kind}` to remove the global copy, \
             or `cpm remove {name} --{kind} --scope local` to keep only global"
        )
    )]
    ScopeConflict {
        /// Asset name.
        name: String,
        /// Asset kind.
        kind: AssetKind,
    },

    /// A global install is already claimed by another repository at a different
    /// resolved revision or source.
    #[error(
        "global install conflict for {name} ({kind}): {claimed_by} recorded {installed_rev}, \
         current repo wants {requested_rev}"
    )]
    #[diagnostic(
        code(cpm::global_install_conflict),
        help(
            "Run `cpm status` or `cpm overview --with-status` in the claiming repo, \
             or align both repositories to the same pinned global asset"
        )
    )]
    GlobalInstallConflict {
        /// Asset name.
        name: String,
        /// Asset kind.
        kind: AssetKind,
        /// Repository that currently owns the machine-local global install.
        claimed_by: String,
        /// Revision currently recorded in `~/.copilot/cpm.lock`.
        installed_rev: String,
        /// Revision requested by the current repository.
        requested_rev: String,
    },

    /// An asset violated the configured license policy.
    #[error(
        "license policy violation for {name} ({kind}): `{license}` is not allowed by `{policy}`"
    )]
    #[diagnostic(
        code(cpm::license_violation),
        help("Adjust [settings].license_policy / allowed_licenses, or remove the asset")
    )]
    LicenseViolation {
        /// Asset name.
        name: String,
        /// Asset kind.
        kind: AssetKind,
        /// Detected SPDX expression.
        license: String,
        /// Policy that triggered the violation.
        policy: String,
    },

    /// The lockfile is out of date and needs `cpm sync`.
    #[error("lock out of date — run `cpm sync`")]
    #[diagnostic(
        code(cpm::lock_out_of_date),
        help("Run `cpm sync` to update the lockfile")
    )]
    LockOutOfDate,

    /// `cpm lock --check` was invoked before `cpm.lock` had been created.
    #[error("cpm.lock does not exist — run `cpm lock` to generate it")]
    #[diagnostic(
        code(cpm::missing_lockfile),
        help("Run `cpm lock` or `cpm sync` to create cpm.lock")
    )]
    MissingLockfile,

    /// An asset requested by name does not exist in the lockfile.
    #[error("asset `{name}` was not found in cpm.lock")]
    #[diagnostic(
        code(cpm::asset_not_found),
        help("Run `cpm list` to inspect the assets currently recorded in cpm.lock")
    )]
    AssetNotFound {
        /// Asset name.
        name: String,
    },

    /// A URL with an unsupported scheme was encountered.
    #[error("unsupported URL scheme: {url}")]
    #[diagnostic(code(cpm::unsupported_url))]
    UnsupportedUrl {
        /// The problematic URL.
        url: String,
    },

    /// The provided asset source is syntactically valid but does not point at a
    /// single addable asset.
    #[error("invalid asset source `{input}`: {reason}")]
    #[diagnostic(code(cpm::invalid_source))]
    InvalidSource {
        /// The problematic source string.
        input: String,
        /// Why the source is invalid.
        reason: String,
    },

    /// A runtime or config-file setting was invalid.
    #[error("invalid config `{key}`: {reason}")]
    #[diagnostic(code(cpm::invalid_config))]
    InvalidConfig {
        /// Config key or environment variable name.
        key: String,
        /// Why the value is invalid.
        reason: String,
    },

    /// Workflow compilation failed.
    #[error("workflow compilation failed: {msg}")]
    #[diagnostic(
        code(cpm::workflow_compile_failed),
        help("Install the GitHub CLI and the `gh aw` extension, or disable workflow auto-compilation")
    )]
    WorkflowCompileFailed {
        /// Human-readable failure details.
        msg: String,
    },

    /// An I/O error.
    #[error("io: {0}")]
    #[diagnostic(code(cpm::io))]
    Io(#[from] std::io::Error),

    /// Authentication is required for a private resource.
    #[error("GitHub authentication required for {url}")]
    #[diagnostic(
        code(cpm::auth_required),
        help(
            "Public GitHub sources usually work anonymously, but private repositories and higher API limits require a token. Run `cpm auth login`, or set CPM_TOKEN / GITHUB_TOKEN in your shell"
        )
    )]
    AuthRequired {
        /// The URL that requires authentication.
        url: String,
    },

    /// A TOML serialisation error.
    #[error("toml serialisation error: {0}")]
    #[diagnostic(code(cpm::toml_ser))]
    TomlSer(#[from] toml::ser::Error),

    /// A TOML deserialisation error.
    #[error("toml deserialisation error: {0}")]
    #[diagnostic(code(cpm::toml_de))]
    TomlDe(#[from] toml::de::Error),

    /// A JSON serialisation/deserialisation error.
    #[error("json error: {0}")]
    #[diagnostic(code(cpm::json))]
    Json(#[from] serde_json::Error),

    /// Keyring access failure.
    #[error("keyring error: {0}")]
    #[diagnostic(code(cpm::keyring))]
    Keyring(String),

    /// The `copilot` binary was not found in `PATH`.
    #[error("copilot binary not found — is the GitHub Copilot CLI installed?")]
    #[diagnostic(
        code(cpm::copilot_not_found),
        help("Install the GitHub Copilot CLI: https://cli.github.com/")
    )]
    CopilotNotFound,

    /// A `copilot plugin` sub-command exited with a non-zero status.
    #[error("copilot plugin {operation} `{name}` failed (exit {code}): {stderr}")]
    #[diagnostic(code(cpm::plugin_command_failed))]
    PluginCommandFailed {
        /// The sub-command that failed (`install`, `uninstall`, or `update`).
        operation: String,
        /// The plugin name passed to the command.
        name: String,
        /// The exit code returned by the process.
        code: i32,
        /// Captured standard output from the process.
        stdout: String,
        /// Captured standard error from the process.
        stderr: String,
    },
}
