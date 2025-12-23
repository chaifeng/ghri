use anyhow::Result;
use std::path::PathBuf;

use crate::{
    archive::Extractor,
    download::Downloader,
    github::{GetReleases, RepoSpec},
    runtime::Runtime,
};

use super::config::{Config, ConfigOverrides};
use super::prune::prune_package_dir;
use super::services::Services;

mod download;
mod external_links;
mod installer;

pub use installer::Installer;

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(runtime, install_root, api_url, filters))]
pub async fn install<R: Runtime + 'static>(
    runtime: R,
    repo_str: &str,
    install_root: Option<PathBuf>,
    api_url: Option<String>,
    filters: Vec<String>,
    pre: bool,
    yes: bool,
    prune: bool,
) -> Result<()> {
    // Load configuration
    let config = Config::load(
        &runtime,
        ConfigOverrides {
            install_root,
            api_url,
        },
    )?;

    // Build services from config
    let services = Services::from_config(&config)?;

    // Run installation
    run(
        &config, runtime, services, repo_str, filters, pre, yes, prune,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(config, runtime, services, filters))]
pub async fn run<R: Runtime + 'static, G: GetReleases, E: Extractor, D: Downloader>(
    config: &Config,
    runtime: R,
    services: Services<G, D, E>,
    repo_str: &str,
    filters: Vec<String>,
    pre: bool,
    yes: bool,
    prune: bool,
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
            filters,
            pre,
            yes,
        )
        .await?;

    // Prune old versions if requested
    if prune {
        let package_dir = config.package_dir(&spec.repo.owner, &spec.repo.repo);
        prune_package_dir(&installer.runtime, &package_dir, &spec.repo.to_string())?;
    }

    Ok(())
}
