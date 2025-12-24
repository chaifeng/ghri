use anyhow::Result;
use std::sync::{Arc, Mutex};

use crate::application::{InstallUseCase, UpgradeUseCase};
use crate::cleanup::CleanupContext;
use crate::runtime::Runtime;
use crate::source::RepoId;

use super::config::{Config, ConfigOverrides, InstallOptions, UpgradeOptions};
use super::install::{DefaultReleaseInstaller, run_install};
use super::prune::prune_package_dir;
use super::services::RegistryServices;

#[tracing::instrument(skip(runtime, overrides, repos, options))]
pub async fn upgrade<R: Runtime + 'static>(
    runtime: R,
    overrides: ConfigOverrides,
    repos: Vec<String>,
    options: UpgradeOptions,
) -> Result<()> {
    let config = Config::load(&runtime, overrides)?;
    let services = RegistryServices::from_config(&config)?;
    run_upgrade(&config, runtime, services, repos, options).await
}

#[tracing::instrument(skip(config, runtime, services, repos, options))]
async fn run_upgrade<R: Runtime + 'static>(
    config: &Config,
    runtime: R,
    services: RegistryServices,
    repos: Vec<String>,
    options: UpgradeOptions,
) -> Result<()> {
    // First, use UpgradeUseCase to find packages and check for updates
    let packages_to_upgrade: Vec<_> = {
        let use_case =
            UpgradeUseCase::new(&runtime, &services.registry, config.install_root.clone());

        // Find all installed packages
        let packages = use_case.find_all_packages()?;

        if packages.is_empty() {
            println!("No packages installed.");
            return Ok(());
        }

        // Parse and filter repositories
        let filter_repos = use_case.parse_repo_filters(&repos);

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
                let update_check = use_case.check_update(&meta, options.pre);

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

    // Create InstallUseCase for orchestration
    let use_case = InstallUseCase::new(
        runtime.as_ref(),
        &services.registry,
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
            &use_case,
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
