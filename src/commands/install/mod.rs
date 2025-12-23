use anyhow::Result;

use crate::{
    archive::Extractor,
    download::Downloader,
    github::{GetReleases, RepoSpec},
    runtime::Runtime,
};

use super::config::{Config, ConfigOverrides, InstallOptions};
use super::prune::prune_package_dir;
use super::services::Services;

mod download;
mod external_links;
mod installer;

pub use installer::Installer;

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
    let services = Services::from_config(&config)?;

    // Run installation
    run(&config, runtime, services, repo_str, options).await
}

#[tracing::instrument(skip(config, runtime, services, options))]
pub async fn run<R: Runtime + 'static, G: GetReleases, E: Extractor, D: Downloader>(
    config: &Config,
    runtime: R,
    services: Services<G, D, E>,
    repo_str: &str,
    options: InstallOptions,
) -> Result<()> {
    let spec = repo_str.parse::<RepoSpec>()?;

    let installer = Installer::new(
        runtime,
        services.github,
        services.downloader,
        services.extractor,
    );
    installer
        .install(
            config,
            &spec.repo,
            spec.version.as_deref(),
            options.filters,
            options.pre,
            options.yes,
        )
        .await?;

    // Prune old versions if requested
    if options.prune {
        let package_dir = config.package_dir(&spec.repo.owner, &spec.repo.repo);
        prune_package_dir(&installer.runtime, &package_dir, &spec.repo.to_string())?;
    }

    Ok(())
}
