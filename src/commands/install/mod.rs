use anyhow::Result;

use crate::runtime::Runtime;

use super::config::{Config, ConfigOverrides, InstallOptions};
use super::prune::prune_package_dir;
use super::services::RegistryServices;

mod download;
mod external_links;
mod installer;
mod repo_spec;

pub use installer::Installer;
pub use repo_spec::RepoSpec;

#[tracing::instrument(skip(runtime, overrides, options))]
pub async fn install<R: Runtime + 'static>(
    runtime: R,
    repo_str: &str,
    overrides: ConfigOverrides,
    options: InstallOptions,
) -> Result<()> {
    // Load configuration
    let config = Config::load(&runtime, overrides)?;

    // Build services from config
    let services = RegistryServices::from_config(&config)?;

    // Run installation
    run(&config, runtime, services, repo_str, options).await
}

#[tracing::instrument(skip(config, runtime, services, options))]
pub async fn run<R: Runtime + 'static>(
    config: &Config,
    runtime: R,
    services: RegistryServices,
    repo_str: &str,
    options: InstallOptions,
) -> Result<()> {
    let spec = repo_str.parse::<RepoSpec>()?;

    // Create installer with source from registry
    let installer = Installer::new(
        runtime,
        super::services::build_source(config)?,
        services.downloader,
        services.extractor,
    );

    // Use new install_with_registry method that leverages InstallUseCase
    installer
        .install_with_registry(
            config,
            &services.registry,
            &spec.repo,
            spec.version.as_deref(),
            &options,
        )
        .await?;

    // Prune old versions if requested
    if options.prune {
        prune_package_dir(
            &installer.runtime,
            &config.install_root,
            &spec.repo.owner,
            &spec.repo.repo,
            &spec.repo.to_string(),
        )?;
    }

    Ok(())
}
