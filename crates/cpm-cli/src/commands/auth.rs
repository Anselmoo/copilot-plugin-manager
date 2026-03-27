//! `cpm auth` — manage authentication tokens.

use std::{io::IsTerminal, process::Command};

use clap::{Args, Subcommand};
use cpm_core::auth::{self, AuthStatus};
use cpm_core::CpmError;

const GITHUB_TOKEN_URL: &str = "https://github.com/settings/personal-access-tokens/new";

/// Arguments for `cpm auth`.
#[derive(Debug, Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommand,
}

/// `cpm auth` subcommands.
#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Store a token in the system keyring.
    Login(LoginArgs),
    /// Remove the stored token.
    Logout,
    /// Show whether a token is available.
    Status,
}

/// Arguments for `cpm auth login`.
#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Open the GitHub token creation page in your browser first.
    #[arg(long, alias = "web")]
    pub open: bool,
}

pub async fn run(cmd: AuthCommand) -> Result<(), CpmError> {
    match cmd {
        AuthCommand::Login(args) => {
            if let Some((token, source)) = token_from_environment() {
                auth::login(token.trim())?;
                println!("✓ Token from {source} stored in the system keyring.");
                return Ok(());
            }

            if args.open {
                if let Err(err) = open_browser(GITHUB_TOKEN_URL) {
                    eprintln!("! Could not open browser automatically: {err}");
                }
            }

            print_login_help();
            if !std::io::stdin().is_terminal() {
                return Err(CpmError::InvalidConfig {
                    key: "token".to_owned(),
                    reason: format!(
                        "no token provided in CPM_TOKEN or GITHUB_TOKEN, and stdin is not interactive. Visit {GITHUB_TOKEN_URL} and re-run with `CPM_TOKEN=... uv run cpm auth login` or use a terminal prompt"
                    ),
                });
            }

            let token = rpassword::prompt_password("GitHub token: ").map_err(|err| {
                CpmError::InvalidConfig {
                    key: "token".to_owned(),
                    reason: format!("failed to read token securely: {err}"),
                }
            })?;
            let token = token.trim();
            if token.is_empty() {
                return Err(CpmError::InvalidConfig {
                    key: "token".to_owned(),
                    reason: "no token provided".to_owned(),
                });
            }

            auth::login(token)?;
            println!("✓ Token stored in the system keyring.");
        }
        AuthCommand::Logout => {
            auth::logout()?;
            println!("✓ Token removed.");
        }
        AuthCommand::Status => match auth::status() {
            AuthStatus::Authenticated => println!("✓ Authenticated (token available)."),
            AuthStatus::Unauthenticated => {
                println!("✗ No token found (unauthenticated — 60 req/h limit applies).")
            }
        },
    }
    Ok(())
}

fn token_from_environment() -> Option<(String, &'static str)> {
    for (key, label) in [("CPM_TOKEN", "CPM_TOKEN"), ("GITHUB_TOKEN", "GITHUB_TOKEN")] {
        if let Ok(token) = std::env::var(key) {
            if !token.trim().is_empty() {
                return Some((token, label));
            }
        }
    }
    None
}

fn print_login_help() {
    eprintln!("No GitHub token found in CPM_TOKEN or GITHUB_TOKEN.");
    eprintln!("Create one here:");
    eprintln!("  {GITHUB_TOKEN_URL}");
    eprintln!("Use `uv run cpm auth login --open` to open that page automatically.");
    eprintln!("For private repositories, grant read access to the repos you need.");
    eprintln!("You can either paste the token below or re-run with:");
    eprintln!("  CPM_TOKEN=ghp_your_token uv run cpm auth login");
}

fn open_browser(url: &str) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "browser command exited with status {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn token_from_environment_prefers_cpm_token() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::set_var("CPM_TOKEN", "cpm-token");
            std::env::set_var("GITHUB_TOKEN", "github-token");
        }
        assert_eq!(
            token_from_environment(),
            Some(("cpm-token".to_owned(), "CPM_TOKEN"))
        );
        unsafe {
            std::env::remove_var("CPM_TOKEN");
            std::env::remove_var("GITHUB_TOKEN");
        }
    }

    #[test]
    fn token_from_environment_falls_back_to_github_token() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::remove_var("CPM_TOKEN");
            std::env::set_var("GITHUB_TOKEN", "github-token");
        }
        assert_eq!(
            token_from_environment(),
            Some(("github-token".to_owned(), "GITHUB_TOKEN"))
        );
        unsafe {
            std::env::remove_var("GITHUB_TOKEN");
        }
    }
}
