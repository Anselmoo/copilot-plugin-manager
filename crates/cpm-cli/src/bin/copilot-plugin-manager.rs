#[tokio::main]
async fn main() -> miette::Result<()> {
    cpm_cli::run_cli().await
}
