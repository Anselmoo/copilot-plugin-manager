//! Delegate for `copilot plugin install|uninstall|update` subprocess calls.
//!
//! Plugins are managed through the `copilot` CLI rather than direct file
//! operations. This module wraps those subprocess calls with strong error
//! handling. Skills, agents, MCPs, hooks, and workflows are **not** affected
//! and remain on the direct-management path.
//!
//! # Example
//!
//! ```no_run
//! # use cpm_core::plugin_delegate::PluginDelegate;
//! # #[tokio::main]
//! # async fn main() -> Result<(), cpm_core::CpmError> {
//! PluginDelegate::default().install("my-plugin").await?;
//! PluginDelegate::default().update("my-plugin").await?;
//! PluginDelegate::default().uninstall("my-plugin").await?;
//! # Ok(())
//! # }
//! ```

use tokio::process::Command;
use tracing::{debug, info};

use crate::CpmError;

// ─── PluginDelegate ──────────────────────────────────────────────────────────

/// Wraps `copilot plugin install|uninstall|update` as async subprocess calls.
///
/// Use [`PluginDelegate::default`] for production code (resolves `copilot`
/// from `PATH`) and [`PluginDelegate::with_binary`] in tests to inject a
/// fake binary.
pub struct PluginDelegate {
    copilot_bin: String,
}

impl Default for PluginDelegate {
    /// Creates a delegate that invokes the `copilot` binary found on `PATH`.
    fn default() -> Self {
        Self {
            copilot_bin: "copilot".to_owned(),
        }
    }
}

impl PluginDelegate {
    /// Creates a delegate that invokes the given binary path instead of `copilot`.
    ///
    /// Useful in tests to point at a fake binary.
    pub fn with_binary(bin: impl Into<String>) -> Self {
        Self {
            copilot_bin: bin.into(),
        }
    }

    /// Install a plugin using `copilot plugin install <name>`.
    ///
    /// # Errors
    /// - [`CpmError::CopilotNotFound`] if the `copilot` binary is not found.
    /// - [`CpmError::PluginCommandFailed`] if the command exits non-zero.
    pub async fn install(&self, name: &str) -> Result<(), CpmError> {
        self.run_op("install", name).await
    }

    /// Uninstall a plugin using `copilot plugin uninstall <name>` or
    /// `copilot plugin uninstall <name>@<registry>`.
    ///
    /// # Errors
    /// - [`CpmError::CopilotNotFound`] if the `copilot` binary is not found.
    /// - [`CpmError::PluginCommandFailed`] if the command exits non-zero.
    pub async fn uninstall(&self, name: &str) -> Result<(), CpmError> {
        self.run_op("uninstall", name).await
    }

    /// Update a plugin using `copilot plugin update <name>` or
    /// `copilot plugin update <name>@<registry>`.
    ///
    /// # Errors
    /// - [`CpmError::CopilotNotFound`] if the `copilot` binary is not found.
    /// - [`CpmError::PluginCommandFailed`] if the command exits non-zero.
    pub async fn update(&self, name: &str) -> Result<(), CpmError> {
        self.run_op("update", name).await
    }

    // ── internals ────────────────────────────────────────────────────────────

    async fn run_op(&self, operation: &str, name: &str) -> Result<(), CpmError> {
        debug!(bin = %self.copilot_bin, %operation, plugin = %name, "spawning copilot plugin subcommand");

        let output = Command::new(&self.copilot_bin)
            .args(["plugin", operation, name])
            .output()
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    CpmError::CopilotNotFound
                } else {
                    CpmError::Io(e)
                }
            })?;

        if output.status.success() {
            info!(%operation, plugin = %name, "copilot plugin command succeeded");
            return Ok(());
        }

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let code = output.status.code().unwrap_or(-1);

        Err(CpmError::PluginCommandFailed {
            operation: operation.to_owned(),
            name: name.to_owned(),
            code,
            stdout,
            stderr,
        })
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Write a tiny fake `copilot` executable into `dir` that exits with
    /// `exit_code` and emits `stdout_msg` / `stderr_msg`, then mark it
    /// executable where necessary. Returns the absolute path to the script.
    fn write_fake_copilot(
        dir: &TempDir,
        exit_code: i32,
        stdout_msg: &str,
        stderr_msg: &str,
    ) -> String {
        #[cfg(unix)]
        {
            let path = dir.path().join("copilot");
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\nprintf '%s\\n' '{}' >&2\nexit {}\n",
                stdout_msg, stderr_msg, exit_code,
            );
            fs::write(&path, script).expect("write fake copilot");
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                    .expect("chmod fake copilot");
            }
            path.to_string_lossy().into_owned()
        }

        #[cfg(windows)]
        {
            let path = dir.path().join("copilot.cmd");
            let script = format!(
                "@echo off\r\necho {}\r\necho {} 1>&2\r\nexit /b {}\r\n",
                stdout_msg, stderr_msg, exit_code,
            );
            fs::write(&path, script).expect("write fake copilot");
            path.to_string_lossy().into_owned()
        }
    }

    // ── success cases ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn install_succeeds_on_zero_exit() {
        let dir = TempDir::new().expect("tempdir");
        let bin = write_fake_copilot(&dir, 0, "installed ok", "");
        PluginDelegate::with_binary(&bin)
            .install("my-plugin")
            .await
            .expect("install should succeed");
    }

    #[tokio::test]
    async fn uninstall_succeeds_on_zero_exit() {
        let dir = TempDir::new().expect("tempdir");
        let bin = write_fake_copilot(&dir, 0, "uninstalled ok", "");
        PluginDelegate::with_binary(&bin)
            .uninstall("my-plugin")
            .await
            .expect("uninstall should succeed");
    }

    #[tokio::test]
    async fn update_succeeds_on_zero_exit() {
        let dir = TempDir::new().expect("tempdir");
        let bin = write_fake_copilot(&dir, 0, "updated ok", "");
        PluginDelegate::with_binary(&bin)
            .update("my-plugin")
            .await
            .expect("update should succeed");
    }

    // ── non-zero exit ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn install_returns_plugin_command_failed_on_nonzero_exit() {
        let dir = TempDir::new().expect("tempdir");
        let bin = write_fake_copilot(&dir, 1, "nothing", "plugin not found");
        let err = PluginDelegate::with_binary(&bin)
            .install("bad-plugin")
            .await
            .expect_err("install should fail");
        match err {
            CpmError::PluginCommandFailed {
                operation,
                name,
                code,
                stderr,
                ..
            } => {
                assert_eq!(operation, "install");
                assert_eq!(name, "bad-plugin");
                assert_eq!(code, 1);
                assert!(
                    stderr.contains("plugin not found"),
                    "stderr should surface error text, got: {stderr}"
                );
            }
            other => panic!("expected PluginCommandFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn uninstall_returns_plugin_command_failed_on_nonzero_exit() {
        let dir = TempDir::new().expect("tempdir");
        let bin = write_fake_copilot(&dir, 2, "", "no such plugin");
        let err = PluginDelegate::with_binary(&bin)
            .uninstall("gone")
            .await
            .expect_err("uninstall should fail");
        match err {
            CpmError::PluginCommandFailed {
                operation, code, ..
            } => {
                assert_eq!(operation, "uninstall");
                assert_eq!(code, 2);
            }
            other => panic!("expected PluginCommandFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn update_returns_plugin_command_failed_on_nonzero_exit() {
        let dir = TempDir::new().expect("tempdir");
        let bin = write_fake_copilot(&dir, 127, "", "command not found");
        let err = PluginDelegate::with_binary(&bin)
            .update("some-plugin")
            .await
            .expect_err("update should fail");
        match err {
            CpmError::PluginCommandFailed {
                operation, code, ..
            } => {
                assert_eq!(operation, "update");
                assert_eq!(code, 127);
            }
            other => panic!("expected PluginCommandFailed, got {other:?}"),
        }
    }

    // ── missing binary ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn returns_copilot_not_found_for_missing_binary() {
        let err = PluginDelegate::with_binary("/nonexistent/path/to/copilot")
            .install("any-plugin")
            .await
            .expect_err("should fail when binary is missing");
        assert!(
            matches!(err, CpmError::CopilotNotFound),
            "expected CopilotNotFound, got {err:?}"
        );
    }

    #[tokio::test]
    async fn stdout_and_stderr_are_captured_in_failure() {
        let dir = TempDir::new().expect("tempdir");
        let bin = write_fake_copilot(&dir, 3, "out line", "err line");
        let err = PluginDelegate::with_binary(&bin)
            .install("cap-test")
            .await
            .expect_err("should fail");
        match err {
            CpmError::PluginCommandFailed { stdout, stderr, .. } => {
                assert!(stdout.contains("out line"), "stdout not captured: {stdout}");
                assert!(stderr.contains("err line"), "stderr not captured: {stderr}");
            }
            other => panic!("expected PluginCommandFailed, got {other:?}"),
        }
    }
}
