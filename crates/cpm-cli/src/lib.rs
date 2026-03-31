use clap::Parser;
use miette::IntoDiagnostic;

mod bootstrap;
pub mod commands;
pub mod progress;

pub fn maybe_delegate_to_source_checkout() -> miette::Result<Option<i32>> {
    bootstrap::maybe_delegate_to_source_checkout()
}

pub async fn run_cli() -> miette::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = commands::Cli::parse();
    cli.run().await.into_diagnostic()
}
