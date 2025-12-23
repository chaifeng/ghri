use anyhow::Result;

use crate::archive::ArchiveExtractor;
use crate::download::Downloader;
use crate::github::{GetReleases, GitHubRepo};
use crate::package::{Meta, find_all_packages};
use crate::runtime::Runtime;

use super::config::{Config, ConfigOverrides, InstallOptions, UpgradeOptions};
use super::install::Installer;
use super::prune::prune_package_dir;
use super::services::Services;

#[tracing::instrument(skip(runtime, overrides, repos, options))]
pub async fn upgrade<R: Runtime + 'static>(
    runtime: R,
    overrides: ConfigOverrides,
    repos: Vec<String>,
    options: UpgradeOptions,
) -> Result<()> {
    let config = Config::load(&runtime, overrides)?;
    let services = Services::from_config(&config)?;
    run_upgrade(&config, runtime, services, repos, options).await
}

#[tracing::instrument(skip(config, runtime, services, repos, options))]
async fn run_upgrade<R: Runtime + 'static, G: GetReleases, E: ArchiveExtractor, D: Downloader>(
    config: &Config,
    runtime: R,
    services: Services<G, D, E>,
    repos: Vec<String>,
    options: UpgradeOptions,
) -> Result<()> {
    let meta_files = find_all_packages(&runtime, &config.install_root)?;
    if meta_files.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    // Parse requested repos for filtering
    let filter_repos: Vec<GitHubRepo> = repos
        .iter()
        .filter_map(|r| r.parse::<GitHubRepo>().ok())
        .collect();

    let installer = Installer::new(
        runtime,
        services.github,
        services.downloader,
        services.extractor,
    );

    let mut upgraded_count = 0;
    let mut skipped_count = 0;

    for meta_path in meta_files {
        let meta = Meta::load(&installer.runtime, &meta_path)?;
        let repo = meta.name.parse::<GitHubRepo>()?;

        // Skip if not in filter list (when filter is specified)
        if !filter_repos.is_empty() && !filter_repos.contains(&repo) {
            continue;
        }

        // Get the latest version from cached release info
        let latest = if options.pre {
            meta.get_latest_release()
        } else {
            meta.get_latest_stable_release()
        };

        let Some(latest) = latest else {
            println!("   {} no release available", repo);
            skipped_count += 1;
            continue;
        };

        // Skip if already on latest version
        if meta.current_version == latest.version {
            println!("   {} {} is up to date", repo, meta.current_version);
            skipped_count += 1;
            continue;
        }

        println!(
            "   upgrading {} {} -> {}",
            repo, meta.current_version, latest.version
        );

        // Install the new version using saved filters from meta
        let package_dir = config.package_dir(&repo.owner, &repo.repo);
        let install_options = InstallOptions {
            filters: vec![], // Empty filters - installer will use saved filters from meta
            pre: options.pre,
            yes: options.yes,
            prune: false,          // Handle prune separately below
            original_args: vec![], // No original args needed for upgrade
        };
        if let Err(e) = installer
            .install(config, &repo, Some(&latest.version), &install_options)
            .await
        {
            eprintln!("   failed to upgrade {}: {}", repo, e);
        } else {
            upgraded_count += 1;

            // Prune old versions if requested
            if options.prune
                && let Err(e) =
                    prune_package_dir(&installer.runtime, &package_dir, &repo.to_string())
            {
                eprintln!("   warning: failed to prune {}: {}", repo, e);
            }
        }
    }

    println!();
    println!(
        "Upgraded {} package(s), {} already up to date.",
        upgraded_count, skipped_count
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::MockArchiveExtractor;
    use crate::download::mock::MockDownloader;
    use crate::github::{MockGetReleases, Release};
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;
    use std::path::PathBuf;

    // Helper to configure simple home dir and user
    fn configure_runtime_basics(runtime: &mut MockRuntime) {
        #[cfg(not(windows))]
        runtime
            .expect_home_dir()
            .returning(|| Some(PathBuf::from("/home/user")));

        #[cfg(windows)]
        runtime
            .expect_home_dir()
            .returning(|| Some(PathBuf::from("C:\\Users\\user")));

        runtime
            .expect_env_var()
            .with(eq("USER"))
            .returning(|_| Ok("user".to_string()));

        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        runtime.expect_is_privileged().returning(|| false);
    }

    fn test_config() -> Config {
        #[cfg(not(windows))]
        let install_root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let install_root = PathBuf::from("C:\\Users\\user\\.ghri");

        Config {
            install_root,
            api_url: Config::DEFAULT_API_URL.to_string(),
            token: None,
        }
    }

    #[tokio::test]
    async fn test_upgrade_no_packages() {
        // Test that upgrade shows "No packages installed" when directory is empty

        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");

        // --- Setup ---

        // Directory exists: ~/.ghri -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Read dir ~/.ghri -> empty (no packages)
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|_| Ok(vec![]));

        // --- Execute ---

        let config = test_config();
        let services = Services {
            github: MockGetReleases::new(),
            downloader: MockDownloader::new(),
            extractor: MockArchiveExtractor::new(),
        };
        let result = run_upgrade(
            &config,
            runtime,
            services,
            vec![],
            UpgradeOptions {
                yes: true,
                ..Default::default()
            },
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_upgrade_already_latest() {
        // Test that upgrade skips packages that are already on the latest version

        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");

        // --- 1. Find Installed Packages ---

        // Directory exists: ~/.ghri -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Read dir ~/.ghri -> [o] (one owner)
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|p| Ok(vec![p.join("o")]));

        // Is dir checks for owner/repo traversal
        runtime.expect_is_dir().returning(|_| true);

        // Read dir ~/.ghri/o -> [r] (one repo)
        runtime
            .expect_read_dir()
            .with(eq(root.join("o")))
            .returning(|p| Ok(vec![p.join("r")]));

        // File exists: ~/.ghri/o/r/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(root.join("o/r/meta.json")))
            .returning(|_| true);

        // --- 2. Load Current Metadata ---

        // Already on latest version v2
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v2".into(),
            updated_at: "now".into(),
            api_url: "api".into(),
            releases: vec![
                Release {
                    tag_name: "v2".into(),
                    published_at: Some("2024".into()),
                    ..Default::default()
                }
                .into(),
                Release {
                    tag_name: "v1".into(),
                    published_at: Some("2023".into()),
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- Execute ---

        let config = test_config();
        let services = Services {
            github: MockGetReleases::new(),
            downloader: MockDownloader::new(),
            extractor: MockArchiveExtractor::new(),
        };
        let result = run_upgrade(
            &config,
            runtime,
            services,
            vec![],
            UpgradeOptions {
                yes: true,
                ..Default::default()
            },
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_upgrade_filter_specific_packages() {
        // Test that upgrade only upgrades specified packages when filter is provided

        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");

        // --- 1. Find Installed Packages ---

        // Directory exists: ~/.ghri -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Read dir ~/.ghri -> [owner1, owner2] (two owners)
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|p| Ok(vec![p.join("owner1"), p.join("owner2")]));

        // Is dir checks for owner/repo traversal
        runtime.expect_is_dir().returning(|_| true);

        // Read dir ~/.ghri/owner1 -> [repo1]
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner1")))
            .returning(|p| Ok(vec![p.join("repo1")]));

        // Read dir ~/.ghri/owner2 -> [repo2]
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner2")))
            .returning(|p| Ok(vec![p.join("repo2")]));

        // File exists: meta.json for both packages
        runtime
            .expect_exists()
            .with(eq(root.join("owner1/repo1/meta.json")))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(root.join("owner2/repo2/meta.json")))
            .returning(|_| true);

        // --- 2. Load Current Metadata ---

        // Only owner1/repo1 should be checked (filter applied)
        let meta1 = Meta {
            name: "owner1/repo1".into(),
            current_version: "v1".into(),
            updated_at: "old".into(),
            api_url: "api".into(),
            releases: vec![
                Release {
                    tag_name: "v1".into(), // Already at latest
                    published_at: Some("2024".into()),
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };
        let meta2 = Meta {
            name: "owner2/repo2".into(),
            current_version: "v1".into(),
            updated_at: "old".into(),
            api_url: "api".into(),
            releases: vec![
                Release {
                    tag_name: "v1".into(),
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };
        let meta1_json = serde_json::to_string(&meta1).unwrap();
        let meta2_json = serde_json::to_string(&meta2).unwrap();

        runtime
            .expect_read_to_string()
            .with(eq(root.join("owner1/repo1/meta.json")))
            .returning(move |_| Ok(meta1_json.clone()));
        runtime
            .expect_read_to_string()
            .with(eq(root.join("owner2/repo2/meta.json")))
            .returning(move |_| Ok(meta2_json.clone()));

        // --- Execute ---

        let config = test_config();
        let services = Services {
            github: MockGetReleases::new(),
            downloader: MockDownloader::new(),
            extractor: MockArchiveExtractor::new(),
        };
        // Only upgrade owner1/repo1, skip owner2/repo2
        let result = run_upgrade(
            &config,
            runtime,
            services,
            vec!["owner1/repo1".to_string()],
            UpgradeOptions {
                yes: true,
                ..Default::default()
            },
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_upgrade_function() {
        // Test that upgrade() function works with empty install root

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup ---

        // Install root doesn't exist -> no packages to upgrade
        runtime.expect_exists().returning(|_| false);

        // --- Execute ---

        upgrade(
            runtime,
            ConfigOverrides::default(),
            vec![],
            UpgradeOptions {
                yes: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
}
