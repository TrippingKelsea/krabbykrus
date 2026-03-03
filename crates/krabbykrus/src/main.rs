use clap::Parser;
use krabbykrus_cli::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command line arguments
    let cli = Cli::parse();
    
    // Run the CLI command (handles tracing init with verbosity support)
    krabbykrus_cli::run(cli).await
}