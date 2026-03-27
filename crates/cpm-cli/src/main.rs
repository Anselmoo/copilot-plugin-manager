//! `cpm` — Copilot Plugin Manager CLI entry point.

use clap::Parser;
use miette::IntoDiagnostic;

mod commands;
mod progress;

use commands::Cli;

#[tokio::main]
async fn main() -> miette::Result<()> {
    // Initialise tracing from `RUST_LOG` (defaults to `warn`).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    cli.run().await.into_diagnostic()
}
