//! `cpm run` — fetch and run an asset without installing it.

use clap::Args;
use cpm_core::CpmError;

/// Arguments for `cpm run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// URL or package specifier to run.
    pub source: String,

    #[arg(long, group = "kind")]
    pub mcp: bool,
    #[arg(long, group = "kind")]
    pub agent: bool,
}

pub async fn run(args: RunArgs) -> Result<(), CpmError> {
    println!(
        "Running '{}' (mcp={} agent={})",
        args.source, args.mcp, args.agent
    );
    Ok(())
}
