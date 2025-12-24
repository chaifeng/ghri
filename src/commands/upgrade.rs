use anyhow::Result;

use crate::package::PackageRepository;
use crate::runtime::Runtime;
use crate::source::RepoId;

use super::config::{Config, ConfigOverrides, InstallOptions, UpgradeOptions};
use super::install::Installer;
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
    let pkg_repo = PackageRepository::new(&runtime, config.install_root.clone());
    let packages = pkg_repo.find_all_with_meta()?;

    if packages.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    // Parse requested repos for filtering
    let filter_repos: Vec<RepoId> = repos
        .iter()
        .filter_map(|r| r.parse::<RepoId>().ok())
        .collect();

    // Create installer with the source from registry
    let installer = Installer::new(
        runtime,
        super::services::build_source(config)?,
        services.downloader,
        services.extractor,
    );

    let mut upgraded_count = 0;
    let mut skipped_count = 0;

    for (_meta_path, meta) in packages {
        let repo = meta.name.parse::<RepoId>()?;

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
                && let Err(e) = prune_package_dir(
                    &installer.runtime,
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
        upgraded_count, skipped_count
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
