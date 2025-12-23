use anyhow::Result;
use std::path::PathBuf;

use crate::{
    archive::Extractor,
    download::Downloader,
    github::{GetReleases, RepoSpec},
    runtime::Runtime,
};

use super::config::Config;
use super::paths::default_install_root;
use super::prune::prune_package_dir;

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
    let config = Config::new(runtime, install_root, api_url)?;
    run(repo_str, config, filters, pre, yes, prune).await
}

#[tracing::instrument(skip(config, filters))]
pub async fn run<R: Runtime + 'static, G: GetReleases, E: Extractor, D: Downloader>(
    repo_str: &str,
    config: Config<R, G, E, D>,
    filters: Vec<String>,
    pre: bool,
    yes: bool,
    prune: bool,
) -> Result<()> {
    let spec = repo_str.parse::<RepoSpec>()?;

    // Get install root for prune (before config is consumed)
    let root = match &config.install_root {
        Some(path) => path.clone(),
        None => default_install_root(&config.runtime)?,
    };

    let installer = Installer::new(
        config.runtime,
        config.github,
        config.downloader,
        config.extractor,
    );
    installer
        .install(
            &spec.repo,
            spec.version.as_deref(),
            config.install_root,
            filters,
            pre,
            yes,
        )
        .await?;

    // Prune old versions if requested
    if prune {
        let package_dir = root.join(&spec.repo.owner).join(&spec.repo.repo);
        prune_package_dir(&installer.runtime, &package_dir, &spec.repo.to_string())?;
    }

    Ok(())
}
