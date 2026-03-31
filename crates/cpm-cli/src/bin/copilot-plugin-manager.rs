#[tokio::main]
async fn main() -> miette::Result<()> {
    if let Some(code) = cpm_cli::maybe_delegate_to_source_checkout()? {
        std::process::exit(code);
    }
    cpm_cli::run_cli().await
}
