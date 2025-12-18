use anyhow::Result;
use clap::Parser;
use ghri::install::install;

/// ghri - GitHub Release Installer
///
/// Download and install binaries from GitHub releases.
///
/// If the GITHUB_TOKEN environment variable is set, it will be used for authentication.
/// This is useful for accessing private repositories or avoiding rate limits.
///
/// Examples:
///   ghri owner/repo     # Install the latest release from owner/repo
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Cli {
    /// The GitHub repository in the format "owner/repo"
    #[arg(value_name = "OWNER/REPO")]
    repo: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    install(&cli.repo).await
}
