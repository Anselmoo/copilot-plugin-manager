//! Token resolution and `cpm auth` subcommand backing logic.
//!
//! Token resolution order (first match wins):
//! 1. `CPM_TOKEN` env var
//! 2. `GITHUB_TOKEN` env var
//! 3. System keyring (`cpm auth login` stores here)
//! 4. Unauthenticated (rate-limited to 60 req/h on the GitHub API)
//!
//! Tokens are **never** stored in `cpm.toml` or `cpm.lock`.

use keyring::Entry;
use tracing::{debug, info};

use crate::CpmError;

const KEYRING_SERVICE: &str = "cpm";
const KEYRING_USER: &str = "github_token";

/// Attempt to resolve a GitHub API token from the environment or the system
/// keyring. Returns `None` if no token is available (unauthenticated access).
///
/// Resolution order:
/// 1. `CPM_TOKEN` environment variable
/// 2. `GITHUB_TOKEN` environment variable
/// 3. System keyring entry created by `cpm auth login`
pub fn resolve_token() -> Option<String> {
    if let Ok(tok) = std::env::var("CPM_TOKEN") {
        if !tok.is_empty() {
            debug!("using token from CPM_TOKEN");
            return Some(tok);
        }
    }

    if let Ok(tok) = std::env::var("GITHUB_TOKEN") {
        if !tok.is_empty() {
            debug!("using token from GITHUB_TOKEN");
            return Some(tok);
        }
    }

    match Entry::new(KEYRING_SERVICE, KEYRING_USER) {
        Ok(entry) => match entry.get_password() {
            Ok(tok) if !tok.is_empty() => {
                debug!("using token from system keyring");
                Some(tok)
            }
            Ok(_) | Err(_) => {
                debug!("no token in keyring");
                None
            }
        },
        Err(e) => {
            debug!("keyring unavailable: {e}");
            None
        }
    }
}

/// Store a GitHub token in the system keyring.
pub fn login(token: &str) -> Result<(), CpmError> {
    let entry =
        Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| CpmError::Keyring(e.to_string()))?;
    entry
        .set_password(token)
        .map_err(|e| CpmError::Keyring(e.to_string()))?;
    info!("token stored in system keyring");
    Ok(())
}

/// Remove the stored token from the system keyring.
pub fn logout() -> Result<(), CpmError> {
    let entry =
        Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| CpmError::Keyring(e.to_string()))?;
    match entry.delete_credential() {
        Ok(()) => {
            info!("token removed from system keyring");
            Ok(())
        }
        Err(keyring::Error::NoEntry) => {
            info!("no token to remove");
            Ok(())
        }
        Err(e) => Err(CpmError::Keyring(e.to_string())),
    }
}

/// Return whether a token is currently stored.
pub fn status() -> AuthStatus {
    match resolve_token() {
        Some(_) => AuthStatus::Authenticated,
        None => AuthStatus::Unauthenticated,
    }
}

/// Result of [`status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatus {
    /// A token is available.
    Authenticated,
    /// No token — requests will be rate-limited (60/h on GitHub API).
    Unauthenticated,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env-var tests to prevent races between parallel test threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_token_from_cpm_token() {
        let _guard = ENV_LOCK.lock().expect("lock");
        std::env::remove_var("GITHUB_TOKEN");
        std::env::set_var("CPM_TOKEN", "test_token_123");
        let tok = resolve_token();
        std::env::remove_var("CPM_TOKEN");
        assert_eq!(tok, Some("test_token_123".to_owned()));
    }

    #[test]
    fn resolve_token_from_github_token() {
        let _guard = ENV_LOCK.lock().expect("lock");
        std::env::remove_var("CPM_TOKEN");
        std::env::set_var("GITHUB_TOKEN", "gh_token_456");
        let tok = resolve_token();
        std::env::remove_var("GITHUB_TOKEN");
        assert_eq!(tok, Some("gh_token_456".to_owned()));
    }
}
