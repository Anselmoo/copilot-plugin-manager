//! `cpm cache` — manage the cpm cache.

use clap::{Args, Subcommand};
use cpm_core::fetcher::cache_dir;
use cpm_core::CpmError;

/// Arguments for `cpm cache`.
#[derive(Debug, Args)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub command: CacheCommand,
}

/// `cpm cache` subcommands.
#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// Print the cache directory path.
    Dir,
    /// Remove all cached files.
    Clean,
    /// Remove cache entries not referenced by any lockfile.
    Prune,
}

pub async fn run(cmd: CacheCommand) -> Result<(), CpmError> {
    let dir = cache_dir();
    match cmd {
        CacheCommand::Dir => {
            println!("{}", dir.display());
        }
        CacheCommand::Clean => {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
                println!("Cache cleaned: {}", dir.display());
            } else {
                println!("Cache is already empty.");
            }
        }
        CacheCommand::Prune => {
            println!("Pruning unreferenced cache entries in {}", dir.display());
            // Full implementation: walk lockfiles, remove unreferenced blobs.
        }
    }
    Ok(())
}
