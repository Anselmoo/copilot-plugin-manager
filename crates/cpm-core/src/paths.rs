//! Shared path helpers for Copilot-managed state and portable path rendering.

use std::path::{Path, PathBuf};

/// Return the current user's home directory for Copilot state.
///
/// Prefer explicit environment overrides used by tests and spawned CLI flows
/// before falling back to platform directory discovery.
pub fn copilot_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

/// Return the directory containing machine-global Copilot state.
///
/// Falls back to the platform temp directory when the home directory cannot be
/// resolved, which is safer than a Unix-specific `/tmp` assumption.
pub fn copilot_state_dir() -> PathBuf {
    copilot_home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(".copilot")
}

/// Render a path in the portable slash-normalized form used by JSON output and
/// tests.
///
/// On Windows this also strips the verbatim `\\?\` prefix so user-facing output
/// remains stable and easier to compare.
pub fn portable_path_string(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let simplified = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    simplified.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::portable_path_string;

    #[test]
    fn portable_path_string_normalizes_windows_prefix_and_separators() {
        let path = Path::new(r"\\?\C:\Users\runneradmin\AppData\Local\Temp\demo");
        assert_eq!(
            portable_path_string(path),
            "C:/Users/runneradmin/AppData/Local/Temp/demo"
        );
    }
}
