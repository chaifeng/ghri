use anyhow::Result;
use clap::Parser;
use ghri::install::install;
use std::path::PathBuf;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

/// ghri - GitHub Release Installer
///
/// Download and install binaries from GitHub releases.
///
/// If the GITHUB_TOKEN environment variable is set, it will be used for authentication.
/// This is useful for accessing private repositories or avoiding rate limits.
///
/// Examples:
///   ghri install owner/repo          # Install the latest release
///   ghri install owner/repo@v1.0.0   # Install a specific version
#[derive(Parser, Debug)]
#[command(author, version = env!("GHRI_VERSION"), about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Install root directory (overrides defaults; also via GHRI_ROOT)
    #[arg(
        long = "root",
        short = 'r',
        env = "GHRI_ROOT",
        value_name = "PATH",
        global = true
    )]
    pub install_root: Option<PathBuf>,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Install a package from GitHub
    Install(InstallArgs),

    /// Update release information for all installed packages
    Update(UpdateArgs),

    /// List all installed packages
    List(ListArgs),

    /// Link a package's current version to a destination path
    Link(LinkArgs),
}

#[derive(clap::Args, Debug)]
pub struct InstallArgs {
    /// The GitHub repository in the format "owner/repo" or "owner/repo@version"
    #[arg(value_name = "OWNER/REPO[@VERSION]")]
    pub repo: String,

    /// GitHub API URL (overrides defaults; also via GHRI_API_URL)
    #[arg(long = "api-url", env = "GHRI_API_URL", value_name = "URL")]
    pub api_url: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct UpdateArgs {}

#[derive(clap::Args, Debug)]
pub struct ListArgs {}

#[derive(clap::Args, Debug)]
pub struct LinkArgs {
    /// Repository specification: "owner/repo", "owner/repo@version", "owner/repo:path", or "owner/repo@version:path"
    #[arg(value_name = "OWNER/REPO[@VERSION][:PATH]")]
    pub repo: String,

    /// Destination path for the symlink
    #[arg(value_name = "DEST")]
    pub dest: PathBuf,
}

#[tokio::main]
#[tracing::instrument]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")))
        .init();
    let cli = Cli::parse();
    let runtime = ghri::runtime::RealRuntime;

    match cli.command {
        Commands::Install(args) => {
            install(runtime, &args.repo, cli.install_root, args.api_url).await?
        }
        Commands::Update(_args) => ghri::install::update(runtime, cli.install_root, None).await?,
        Commands::List(_args) => ghri::install::list(runtime, cli.install_root)?,
        Commands::Link(args) => ghri::install::link(runtime, &args.repo, args.dest, cli.install_root)?,
    }
    Ok(())
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
                assert_eq!(args.api_url, None);
            }
            _ => panic!("Expected Install command"),
        }
        assert_eq!(cli.install_root, None);
    }

    #[test]
    fn test_cli_update_parsing() {
        let cli = Cli::try_parse_from(&["ghri", "update"]).unwrap();
        assert_eq!(cli.install_root, None);
    }

    #[test]
    fn test_cli_install_root_parsing() {
        let cli =
            Cli::try_parse_from(&["ghri", "install", "owner/repo", "--root", "/tmp"]).unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.api_url, None);
            }
            _ => panic!("Expected Install command"),
        }
        assert_eq!(cli.install_root, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn test_cli_install_api_url_parsing() {
        let cli = Cli::try_parse_from(&[
            "ghri",
            "install",
            "owner/repo",
            "--api-url",
            "https://github.example.com",
        ])
        .unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.api_url, Some("https://github.example.com".to_string()));
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_cli_update_no_api_url() {
        // This should fail because update doesn't have api-url
        let result = Cli::try_parse_from(&["ghri", "update", "--api-url", "https://example.com"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_global_root_parsing() {
        let cli = Cli::try_parse_from(&["ghri", "--root", "/tmp", "update"]).unwrap();
        assert_eq!(cli.install_root, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn test_cli_no_subcommand_fails() {
        let result = Cli::try_parse_from(&["ghri", "owner/repo"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_install_with_version() {
        let cli = Cli::try_parse_from(&["ghri", "install", "owner/repo@v1.0.0"]).unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "owner/repo@v1.0.0");
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_cli_install_with_version_no_v() {
        let cli = Cli::try_parse_from(&["ghri", "install", "bach-sh/bach@0.7.2"]).unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "bach-sh/bach@0.7.2");
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_cli_link_parsing() {
        let cli = Cli::try_parse_from(&["ghri", "link", "owner/repo", "/usr/local/bin/tool"]).unwrap();
        match cli.command {
            Commands::Link(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.dest, PathBuf::from("/usr/local/bin/tool"));
            }
            _ => panic!("Expected Link command"),
        }
    }

    #[test]
    fn test_cli_link_with_root() {
        let cli = Cli::try_parse_from(&["ghri", "--root", "/tmp", "link", "owner/repo", "/dest"]).unwrap();
        match cli.command {
            Commands::Link(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.dest, PathBuf::from("/dest"));
            }
            _ => panic!("Expected Link command"),
        }
        assert_eq!(cli.install_root, Some(PathBuf::from("/tmp")));
    }
}
