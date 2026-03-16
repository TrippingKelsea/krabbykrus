use clap::Parser;
use rockbot_cli::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command line arguments
    let cli = Cli::parse();

    // Run the CLI command (handles tracing init with verbosity support)
    rockbot_cli::run(cli).await
}
