use anyhow::Result;
use std::sync::{Arc, Mutex};

use crate::application::{InstallAction, UpgradeAction};
use crate::cleanup::CleanupContext;
use crate::provider::RepoId;
use crate::runtime::Runtime;

use super::config::{Config, InstallOptions, UpgradeOptions};
use super::install::{DefaultReleaseInstaller, run_install};
use super::prune::prune_package_dir;
use super::services::Services;

#[tracing::instrument(skip(runtime, config, repos, options))]
pub async fn upgrade<R: Runtime + 'static>(
    runtime: R,
    config: Config,
    repos: Vec<String>,
    options: UpgradeOptions,
) -> Result<()> {
    let services = Services::from_config(&config)?;
    run_upgrade(&config, runtime, services, repos, options).await
}

#[tracing::instrument(skip(config, runtime, services, repos, options))]
async fn run_upgrade<R: Runtime + 'static>(
    config: &Config,
    runtime: R,
    services: Services,
    repos: Vec<String>,
    options: UpgradeOptions,
) -> Result<()> {
    // First, use UpgradeAction to find packages and check for updates
    let packages_to_upgrade: Vec<_> = {
        let action = UpgradeAction::new(
            &runtime,
            &services.provider_factory,
            config.install_root.clone(),
        );

        // Find all installed packages
        let packages = action.find_all_packages()?;

        if packages.is_empty() {
            println!("No packages installed.");
            return Ok(());
        }

        // Parse and filter repositories
        let filter_repos = action.parse_repo_filters(&repos);

        // Collect packages that need upgrading
        packages
            .into_iter()
            .filter_map(|(_, meta)| {
                let repo = match meta.name.parse::<RepoId>() {
                    Ok(r) => r,
                    Err(_) => return None,
                };

                // Skip if not in filter list (when filter is specified)
                if !filter_repos.is_empty() && !filter_repos.contains(&repo) {
                    return None;
                }

                // Check for available update
                let update_check = action.check_update(&meta, options.pre);

                match update_check.latest_version {
                    Some(latest) if update_check.has_update => {
                        Some((repo, meta.current_version.clone(), latest))
                    }
                    Some(_) => {
                        println!("   {} {} is up to date", repo, meta.current_version);
                        None
                    }
                    None => {
                        println!("   {} no release available", repo);
                        None
                    }
                }
            })
            .collect()
    };

    if packages_to_upgrade.is_empty() {
        println!("\nAll packages are up to date.");
        return Ok(());
    }

    // Wrap runtime in Arc for shared ownership
    let runtime = Arc::new(runtime);

    // Create InstallAction for orchestration
    let action = InstallAction::new(
        runtime.as_ref(),
        &services.provider_factory,
        config.install_root.clone(),
    );

    // Create release installer
    let release_installer = DefaultReleaseInstaller::new(
        Arc::clone(&runtime),
        Arc::new(services.downloader),
        Arc::new(services.extractor),
        Arc::new(Mutex::new(CleanupContext::new())),
    );

    let mut upgraded_count = 0;
    let total = packages_to_upgrade.len();

    for (repo, current_version, latest_version) in packages_to_upgrade {
        println!(
            "   upgrading {} {} -> {}",
            repo, current_version, latest_version
        );

        // Install the new version using saved filters from meta
        let install_options = InstallOptions {
            filters: vec![], // Empty filters - installer will use saved filters from meta
            pre: options.pre,
            yes: options.yes,
            prune: false,          // Handle prune separately below
            original_args: vec![], // No original args needed for upgrade
        };

        // Use run_install for unified installation path
        // Format: owner/repo@version
        let repo_str = format!("{}@{}", repo, latest_version);
        if let Err(e) = run_install(
            config,
            Arc::clone(&runtime),
            &action,
            &release_installer,
            &repo_str,
            install_options,
        )
        .await
        {
            eprintln!("   failed to upgrade {}: {}", repo, e);
        } else {
            upgraded_count += 1;

            // Prune old versions if requested
            if options.prune
                && let Err(e) = prune_package_dir(
                    runtime.as_ref(),
                    &config.install_root,
                    &repo.owner,
                    &repo.repo,
                    &repo.to_string(),
                )
            {
                eprintln!("   warning: failed to prune {}: {}", repo, e);
            }
        }
    }

    println!();
    println!(
        "Upgraded {} package(s), {} already up to date.",
        upgraded_count,
        total - upgraded_count
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use crate::test_utils::{configure_mock_runtime_basics, test_root};

    // Helper to configure simple home dir and user
    fn configure_runtime_basics(runtime: &mut MockRuntime) {
        configure_mock_runtime_basics(runtime);
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
            Config::for_test(test_root()),
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
