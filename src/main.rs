use anyhow::Result;
use clap::Parser;
use ghri::install::install;

/// Command line arguments
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// The GitHub repository in the format "owner/repo"
    #[arg(value_name = "OWNER/REPO")]
    repo: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    install(&cli.repo).await
}