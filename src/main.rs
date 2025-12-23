use anyhow::Result;
use clap::Parser;
use ghri::commands::{ConfigOverrides, InstallOptions, UpgradeOptions, install};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

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

    /// Upgrade packages to the latest version
    Upgrade(UpgradeArgs),

    /// List all installed packages
    List(ListArgs),

    /// Link a package's current version to a destination path
    Link(LinkArgs),

    /// Remove a link rule and its symlink
    Unlink(UnlinkArgs),

    /// Show link rules for a package
    Links(LinksArgs),

    /// Remove a package or specific version
    Remove(RemoveArgs),

    /// Show detailed information about a package
    Show(ShowArgs),

    /// Remove unused versions, keeping only the current version
    Prune(PruneArgs),
}

#[derive(clap::Args, Debug)]
pub struct InstallArgs {
    /// The GitHub repository in the format "owner/repo" or "owner/repo@version"
    #[arg(value_name = "OWNER/REPO[@VERSION]")]
    pub repo: String,

    /// GitHub API URL (overrides defaults; also via GHRI_API_URL)
    #[arg(long = "api-url", env = "GHRI_API_URL", value_name = "URL")]
    pub api_url: Option<String>,

    /// Filter assets by glob pattern (can be specified multiple times)
    /// Example: --filter "*aarch64*" --filter "*macos*"
    #[arg(long = "filter", short = 'f', value_name = "PATTERN")]
    pub filters: Vec<String>,

    /// Allow installing pre-release versions when no version is specified
    #[arg(long = "pre")]
    pub pre: bool,

    /// Skip confirmation prompt
    #[arg(long = "yes", short = 'y')]
    pub yes: bool,

    /// Remove other versions after successful installation
    #[arg(long = "prune")]
    pub prune: bool,
}

#[derive(clap::Args, Debug)]
pub struct UpdateArgs {
    /// Packages to update (default: all installed packages)
    #[arg(value_name = "OWNER/REPO")]
    pub repos: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct UpgradeArgs {
    /// Packages to upgrade (default: all installed packages)
    #[arg(value_name = "OWNER/REPO")]
    pub repos: Vec<String>,

    /// Allow upgrading to pre-release versions
    #[arg(long = "pre")]
    pub pre: bool,

    /// Skip confirmation prompt
    #[arg(long = "yes", short = 'y')]
    pub yes: bool,

    /// Remove other versions after successful upgrade
    #[arg(long = "prune")]
    pub prune: bool,
}

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

#[derive(clap::Args, Debug)]
pub struct UnlinkArgs {
    /// Repository specification: "owner/repo" or "owner/repo:path" to filter by path
    #[arg(value_name = "OWNER/REPO[:PATH]")]
    pub repo: String,

    /// Destination path of the symlink to remove (optional)
    #[arg(value_name = "DEST")]
    pub dest: Option<PathBuf>,

    /// Remove all link rules for the package
    #[arg(long, short)]
    pub all: bool,
}

#[derive(clap::Args, Debug)]
pub struct LinksArgs {
    /// The GitHub repository in the format "owner/repo"
    #[arg(value_name = "OWNER/REPO")]
    pub repo: String,
}

#[derive(clap::Args, Debug)]
pub struct RemoveArgs {
    /// The GitHub repository in the format "owner/repo" or "owner/repo@version"
    #[arg(value_name = "OWNER/REPO[@VERSION]")]
    pub repo: String,

    /// Force removal without confirmation
    #[arg(long, short)]
    pub force: bool,

    /// Skip confirmation prompt
    #[arg(long = "yes", short = 'y')]
    pub yes: bool,
}

#[derive(clap::Args, Debug)]
pub struct ShowArgs {
    /// The GitHub repository in the format "owner/repo"
    #[arg(value_name = "OWNER/REPO")]
    pub repo: String,
}

#[derive(clap::Args, Debug)]
pub struct PruneArgs {
    /// Packages to prune (default: all installed packages)
    #[arg(value_name = "OWNER/REPO")]
    pub repos: Vec<String>,

    /// Skip confirmation prompt
    #[arg(long = "yes", short = 'y')]
    pub yes: bool,
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
            install(
                runtime,
                &args.repo,
                ConfigOverrides {
                    install_root: cli.install_root,
                    api_url: args.api_url,
                },
                InstallOptions {
                    filters: args.filters,
                    pre: args.pre,
                    yes: args.yes,
                    prune: args.prune,
                },
            )
            .await?
        }
        Commands::Update(args) => {
            ghri::commands::update(
                runtime,
                ConfigOverrides {
                    install_root: cli.install_root,
                    api_url: None,
                },
                args.repos,
            )
            .await?
        }
        Commands::Upgrade(args) => {
            ghri::commands::upgrade(
                runtime,
                ConfigOverrides {
                    install_root: cli.install_root,
                    api_url: None,
                },
                args.repos,
                UpgradeOptions {
                    pre: args.pre,
                    yes: args.yes,
                    prune: args.prune,
                },
            )
            .await?
        }
        Commands::List(_args) => ghri::commands::list(runtime, cli.install_root)?,
        Commands::Link(args) => {
            ghri::commands::link(runtime, &args.repo, args.dest, cli.install_root)?
        }
        Commands::Unlink(args) => {
            ghri::commands::unlink(runtime, &args.repo, args.dest, args.all, cli.install_root)?
        }
        Commands::Links(args) => ghri::commands::links(runtime, &args.repo, cli.install_root)?,
        Commands::Remove(args) => {
            ghri::commands::remove(runtime, &args.repo, args.force, args.yes, cli.install_root)?
        }
        Commands::Show(args) => ghri::commands::show(runtime, &args.repo, cli.install_root)?,
        Commands::Prune(args) => {
            ghri::commands::prune(runtime, args.repos, args.yes, cli.install_root)?
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_cli_install_parsing() {
        let cli = Cli::try_parse_from(["ghri", "install", "owner/repo"]).unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.api_url, None);
                assert!(!args.prune);
            }
            _ => panic!("Expected Install command"),
        }
        assert_eq!(cli.install_root, None);
    }

    #[test]
    fn test_cli_update_parsing() {
        let cli = Cli::try_parse_from(["ghri", "update"]).unwrap();
        assert_eq!(cli.install_root, None);
    }

    #[test]
    fn test_cli_upgrade_parsing() {
        let cli = Cli::try_parse_from(["ghri", "upgrade"]).unwrap();
        match cli.command {
            Commands::Upgrade(args) => {
                assert!(args.repos.is_empty());
                assert!(!args.pre);
                assert!(!args.yes);
                assert!(!args.prune);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn test_cli_upgrade_with_repos() {
        let cli = Cli::try_parse_from(["ghri", "upgrade", "owner/repo"]).unwrap();
        match cli.command {
            Commands::Upgrade(args) => {
                assert_eq!(args.repos, vec!["owner/repo"]);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn test_cli_upgrade_with_pre_flag() {
        let cli = Cli::try_parse_from(["ghri", "upgrade", "--pre"]).unwrap();
        match cli.command {
            Commands::Upgrade(args) => {
                assert!(args.pre);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn test_cli_upgrade_with_yes_flag() {
        let cli = Cli::try_parse_from(["ghri", "upgrade", "-y"]).unwrap();
        match cli.command {
            Commands::Upgrade(args) => {
                assert!(args.yes);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn test_cli_upgrade_with_prune_flag() {
        let cli = Cli::try_parse_from(["ghri", "upgrade", "--prune"]).unwrap();
        match cli.command {
            Commands::Upgrade(args) => {
                assert!(args.prune);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn test_cli_install_with_prune_flag() {
        let cli = Cli::try_parse_from(["ghri", "install", "owner/repo", "--prune"]).unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert!(args.prune);
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_cli_install_root_parsing() {
        let cli = Cli::try_parse_from(["ghri", "install", "owner/repo", "--root", "/tmp"]).unwrap();
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
        let cli = Cli::try_parse_from([
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
        let result = Cli::try_parse_from(["ghri", "update", "--api-url", "https://example.com"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_global_root_parsing() {
        let cli = Cli::try_parse_from(["ghri", "--root", "/tmp", "update"]).unwrap();
        assert_eq!(cli.install_root, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn test_cli_no_subcommand_fails() {
        let result = Cli::try_parse_from(["ghri", "owner/repo"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_install_with_version() {
        let cli = Cli::try_parse_from(["ghri", "install", "owner/repo@v1.0.0"]).unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "owner/repo@v1.0.0");
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_cli_install_with_version_no_v() {
        let cli = Cli::try_parse_from(["ghri", "install", "bach-sh/bach@0.7.2"]).unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "bach-sh/bach@0.7.2");
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_cli_install_with_single_filter() {
        // Test: ghri install owner/repo --filter "*aarch64*"
        let cli = Cli::try_parse_from(["ghri", "install", "owner/repo", "--filter", "*aarch64*"])
            .unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.filters, vec!["*aarch64*"]);
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_cli_install_with_multiple_filters() {
        // Test: ghri install owner/repo --filter "*aarch64*" --filter "*macos*"
        let cli = Cli::try_parse_from([
            "ghri",
            "install",
            "owner/repo",
            "--filter",
            "*aarch64*",
            "--filter",
            "*macos*",
        ])
        .unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.filters, vec!["*aarch64*", "*macos*"]);
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_cli_install_with_short_filter() {
        // Test: ghri install owner/repo -f "*linux*"
        let cli = Cli::try_parse_from(["ghri", "install", "owner/repo", "-f", "*linux*"]).unwrap();
        match cli.command {
            Commands::Install(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.filters, vec!["*linux*"]);
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn test_cli_link_parsing() {
        let cli =
            Cli::try_parse_from(["ghri", "link", "owner/repo", "/usr/local/bin/tool"]).unwrap();
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
        let cli =
            Cli::try_parse_from(["ghri", "--root", "/tmp", "link", "owner/repo", "/dest"]).unwrap();
        match cli.command {
            Commands::Link(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.dest, PathBuf::from("/dest"));
            }
            _ => panic!("Expected Link command"),
        }
        assert_eq!(cli.install_root, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn test_cli_links_parsing() {
        let cli = Cli::try_parse_from(["ghri", "links", "owner/repo"]).unwrap();
        match cli.command {
            Commands::Links(args) => {
                assert_eq!(args.repo, "owner/repo");
            }
            _ => panic!("Expected Links command"),
        }
    }

    #[test]
    fn test_cli_unlink_with_dest() {
        let cli =
            Cli::try_parse_from(["ghri", "unlink", "owner/repo", "/usr/local/bin/tool"]).unwrap();
        match cli.command {
            Commands::Unlink(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.dest, Some(PathBuf::from("/usr/local/bin/tool")));
                assert!(!args.all);
            }
            _ => panic!("Expected Unlink command"),
        }
    }

    #[test]
    fn test_cli_unlink_all() {
        let cli = Cli::try_parse_from(["ghri", "unlink", "owner/repo", "--all"]).unwrap();
        match cli.command {
            Commands::Unlink(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert_eq!(args.dest, None);
                assert!(args.all);
            }
            _ => panic!("Expected Unlink command"),
        }
    }

    #[test]
    fn test_cli_unlink_short_all() {
        let cli = Cli::try_parse_from(["ghri", "unlink", "-a", "owner/repo"]).unwrap();
        match cli.command {
            Commands::Unlink(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert!(args.all);
            }
            _ => panic!("Expected Unlink command"),
        }
    }

    #[test]
    fn test_cli_remove_parsing() {
        let cli = Cli::try_parse_from(["ghri", "remove", "owner/repo"]).unwrap();
        match cli.command {
            Commands::Remove(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert!(!args.force);
            }
            _ => panic!("Expected Remove command"),
        }
    }

    #[test]
    fn test_cli_remove_with_version() {
        let cli = Cli::try_parse_from(["ghri", "remove", "owner/repo@v1.0.0"]).unwrap();
        match cli.command {
            Commands::Remove(args) => {
                assert_eq!(args.repo, "owner/repo@v1.0.0");
                assert!(!args.force);
            }
            _ => panic!("Expected Remove command"),
        }
    }

    #[test]
    fn test_cli_remove_force() {
        let cli = Cli::try_parse_from(["ghri", "remove", "--force", "owner/repo"]).unwrap();
        match cli.command {
            Commands::Remove(args) => {
                assert_eq!(args.repo, "owner/repo");
                assert!(args.force);
            }
            _ => panic!("Expected Remove command"),
        }
    }

    #[test]
    fn test_cli_remove_short_force() {
        let cli = Cli::try_parse_from(["ghri", "remove", "-f", "owner/repo@v1"]).unwrap();
        match cli.command {
            Commands::Remove(args) => {
                assert_eq!(args.repo, "owner/repo@v1");
                assert!(args.force);
            }
            _ => panic!("Expected Remove command"),
        }
    }

    #[test]
    fn test_cli_show_parsing() {
        let cli = Cli::try_parse_from(["ghri", "show", "owner/repo"]).unwrap();
        match cli.command {
            Commands::Show(args) => {
                assert_eq!(args.repo, "owner/repo");
            }
            _ => panic!("Expected Show command"),
        }
    }

    #[test]
    fn test_cli_show_with_root() {
        let cli =
            Cli::try_parse_from(["ghri", "--root", "/tmp/test", "show", "owner/repo"]).unwrap();
        assert_eq!(cli.install_root, Some(PathBuf::from("/tmp/test")));
        match cli.command {
            Commands::Show(args) => {
                assert_eq!(args.repo, "owner/repo");
            }
            _ => panic!("Expected Show command"),
        }
    }

    #[test]
    fn test_cli_prune_parsing() {
        let cli = Cli::try_parse_from(["ghri", "prune"]).unwrap();
        match cli.command {
            Commands::Prune(args) => {
                assert!(args.repos.is_empty());
                assert!(!args.yes);
            }
            _ => panic!("Expected Prune command"),
        }
    }

    #[test]
    fn test_cli_prune_with_repos() {
        let cli = Cli::try_parse_from(["ghri", "prune", "owner/repo"]).unwrap();
        match cli.command {
            Commands::Prune(args) => {
                assert_eq!(args.repos, vec!["owner/repo"]);
            }
            _ => panic!("Expected Prune command"),
        }
    }

    #[test]
    fn test_cli_prune_with_yes() {
        let cli = Cli::try_parse_from(["ghri", "prune", "-y"]).unwrap();
        match cli.command {
            Commands::Prune(args) => {
                assert!(args.yes);
            }
            _ => panic!("Expected Prune command"),
        }
    }

    #[test]
    fn test_cli_prune_with_multiple_repos() {
        let cli =
            Cli::try_parse_from(["ghri", "prune", "owner1/repo1", "owner2/repo2", "-y"]).unwrap();
        match cli.command {
            Commands::Prune(args) => {
                assert_eq!(args.repos, vec!["owner1/repo1", "owner2/repo2"]);
                assert!(args.yes);
            }
            _ => panic!("Expected Prune command"),
        }
    }
}
