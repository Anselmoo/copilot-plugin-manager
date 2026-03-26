//! `cpm doctor` — verify installed file hashes.

use clap::Args;
use cpm_core::doctor::run_doctor_with_global_lock;
use cpm_core::project::{load_global_lockfile, load_lockfile};
use cpm_core::CpmError;

/// Arguments for `cpm doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Stop after the first mismatch.
    #[arg(long)]
    pub fail_fast: bool,
}

pub async fn run(args: DoctorArgs) -> Result<(), CpmError> {
    let repo_root = std::env::current_dir()?;
    let lockfile = load_lockfile(std::path::Path::new("cpm.lock"))?;
    let global_lockfile = load_global_lockfile()?;
    let errors =
        run_doctor_with_global_lock(&lockfile, &global_lockfile, &repo_root, args.fail_fast)?;

    if errors.is_empty() {
        println!("✓ All assets match their recorded hashes.");
    } else {
        for err in &errors {
            eprintln!(
                "✗ {}: expected {}, got {} ({})",
                err.name, err.expected, err.actual, err.path
            );
        }
        // Non-zero exit via error.
        return Err(CpmError::HashMismatch {
            name: errors[0].name.clone(),
            expected: errors[0].expected.clone(),
            actual: errors[0].actual.clone(),
        });
    }

    Ok(())
}
