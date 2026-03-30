use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use miette::{IntoDiagnostic, WrapErr};

const SOURCE_CHECKOUT_DELEGATE_SKIP_ENV: &str = "CPM_SKIP_SOURCE_CHECKOUT_DELEGATE";

#[derive(Debug, Clone, PartialEq, Eq)]
struct DelegateSpec {
    repo_root: PathBuf,
    cargo_args: Vec<OsString>,
}

pub fn maybe_delegate_to_source_checkout() -> miette::Result<Option<i32>> {
    let current_dir = env::current_dir().into_diagnostic()?;
    let current_executable = env::current_exe().ok();
    let args: Vec<OsString> = env::args_os().skip(1).collect();

    let Some(delegate) = resolve_source_checkout_delegate(
        &args,
        &current_dir,
        current_executable.as_deref(),
        env::var_os(SOURCE_CHECKOUT_DELEGATE_SKIP_ENV).is_some(),
    ) else {
        return Ok(None);
    };

    let status = Command::new("cargo")
        .args(&delegate.cargo_args)
        .env(SOURCE_CHECKOUT_DELEGATE_SKIP_ENV, "1")
        .status()
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "source checkout detected at {}, but `cargo run` could not be launched",
                delegate.repo_root.display()
            )
        })?;

    Ok(Some(status.code().unwrap_or(1)))
}

fn resolve_source_checkout_delegate(
    args: &[OsString],
    current_dir: &Path,
    current_executable: Option<&Path>,
    skip_delegate: bool,
) -> Option<DelegateSpec> {
    if skip_delegate {
        return None;
    }

    let repo_root = find_repo_root(current_dir, current_executable)?;
    if current_executable
        .is_some_and(|executable| is_workspace_target_binary(executable, &repo_root))
    {
        return None;
    }

    Some(DelegateSpec {
        repo_root: repo_root.clone(),
        cargo_args: cargo_delegate_args(&repo_root, args),
    })
}

fn find_repo_root(current_dir: &Path, current_executable: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = vec![current_dir.to_path_buf()];
    if let Some(executable) = current_executable {
        candidates.push(executable.to_path_buf());
    }

    for candidate in candidates {
        let current = if candidate.is_dir() {
            candidate
        } else {
            candidate.parent()?.to_path_buf()
        };
        for parent in std::iter::once(current.as_path()).chain(current.ancestors().skip(1)) {
            let repo_root = parent.to_path_buf();
            if repo_root.join("Cargo.toml").is_file()
                && repo_root.join("crates/cpm-cli/Cargo.toml").is_file()
            {
                return Some(repo_root);
            }
        }
    }

    None
}

fn is_workspace_target_binary(current_executable: &Path, repo_root: &Path) -> bool {
    current_executable.starts_with(repo_root.join("target"))
}

fn cargo_delegate_args(repo_root: &Path, args: &[OsString]) -> Vec<OsString> {
    let mut cargo_args = vec![
        OsString::from("run"),
        OsString::from("--quiet"),
        OsString::from("--manifest-path"),
        repo_root.join("Cargo.toml").into_os_string(),
        OsString::from("-p"),
        OsString::from("cpm-cli"),
        OsString::from("--bin"),
        OsString::from("cpm"),
        OsString::from("--"),
    ];
    cargo_args.extend(args.iter().cloned());
    cargo_args
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_workspace(root: &Path) {
        std::fs::create_dir_all(root.join("crates/cpm-cli")).expect("mkdir workspace");
        std::fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write root cargo");
        std::fs::write(
            root.join("crates/cpm-cli/Cargo.toml"),
            "[package]\nname = \"cpm-cli\"\nversion = \"0.0.0\"\n",
        )
        .expect("write cli cargo");
    }

    #[test]
    fn delegates_installed_binary_from_source_checkout() {
        let tempdir = TempDir::new().expect("tempdir");
        let repo_root = tempdir.path().join("repo");
        write_workspace(&repo_root);
        let args = vec![OsString::from("--version")];

        let delegate = resolve_source_checkout_delegate(
            &args,
            &repo_root,
            Some(&repo_root.join(".venv/bin/cpm")),
            false,
        )
        .expect("delegate");

        assert_eq!(delegate.repo_root, repo_root);
        assert_eq!(
            delegate.cargo_args,
            vec![
                OsString::from("run"),
                OsString::from("--quiet"),
                OsString::from("--manifest-path"),
                repo_root.join("Cargo.toml").into_os_string(),
                OsString::from("-p"),
                OsString::from("cpm-cli"),
                OsString::from("--bin"),
                OsString::from("cpm"),
                OsString::from("--"),
                OsString::from("--version"),
            ]
        );
    }

    #[test]
    fn skips_delegate_for_workspace_target_binary() {
        let tempdir = TempDir::new().expect("tempdir");
        let repo_root = tempdir.path().join("repo");
        write_workspace(&repo_root);

        let delegate = resolve_source_checkout_delegate(
            &[],
            tempdir.path(),
            Some(&repo_root.join("target/debug/cpm")),
            false,
        );

        assert!(delegate.is_none());
    }

    #[test]
    fn skips_delegate_when_guard_env_is_present() {
        let tempdir = TempDir::new().expect("tempdir");
        let repo_root = tempdir.path().join("repo");
        write_workspace(&repo_root);

        let delegate = resolve_source_checkout_delegate(
            &[],
            &repo_root,
            Some(&repo_root.join(".venv/bin/cpm")),
            true,
        );

        assert!(delegate.is_none());
    }
}
