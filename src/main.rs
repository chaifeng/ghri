use anyhow::Result;
use clap::Parser;
use ghri::install::install;
use std::path::PathBuf;

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
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Install a package from GitHub
    Install(InstallArgs),

    /// Update release information for all installed packages
    Update(UpdateArgs),
}

#[derive(clap::Args, Debug)]
pub struct InstallArgs {
    /// The GitHub repository in the format "owner/repo"
    #[arg(value_name = "OWNER/REPO")]
    pub repo: String,

    /// Install root directory (overrides defaults; also via GHRI_ROOT)
    #[arg(long = "root", short = 'r', env = "GHRI_ROOT", value_name = "PATH")]
    pub install_root: Option<PathBuf>,

    /// GitHub API URL (defaults to https://api.github.com)
    #[arg(long = "api-url", value_name = "URL")]
    pub api_url: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct UpdateArgs {
    /// Install root directory (overrides defaults; also via GHRI_ROOT)
    #[arg(long = "root", short = 'r', env = "GHRI_ROOT", value_name = "PATH")]
    pub install_root: Option<PathBuf>,

    /// GitHub API URL (defaults to https://api.github.com)
    #[arg(long = "api-url", value_name = "URL")]
    pub api_url: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Install(args) => install(&args.repo, args.install_root, args.api_url).await,
        Commands::Update(args) => ghri::install::update(args.install_root, args.api_url).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_cli_install_parsing() {
        let cli = Cli::try_parse_from(&["ghri", "install", "owner/repo"]).unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.install_root, None);
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_cli_update_parsing() {
        let cli = Cli::try_parse_from(&["ghri", "update"]).unwrap();
        match cli.command {
            Commands::Update(args) => {
                assert_eq!(args.install_root, None);
            }
            _ => panic!("Expected Update command"),
        }
    }

    #[test]
    fn test_cli_install_root_parsing() {
        let cli =
            Cli::try_parse_from(&["ghri", "install", "owner/repo", "--root", "/tmp"]).unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.install_root, Some(PathBuf::from("/tmp")));
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_cli_no_subcommand_fails() {
        let result = Cli::try_parse_from(&["ghri", "owner/repo"]);
        assert!(result.is_err());
    }
}
