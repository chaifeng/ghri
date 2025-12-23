use anyhow::Result;
use std::path::PathBuf;

use crate::{
    archive::Extractor,
    github::{GetReleases, RepoSpec},
    runtime::Runtime,
};

use super::config::Config;
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
    let config = Config::new(runtime, install_root.clone(), api_url)?;
    let spec = repo_str.parse::<RepoSpec>()?;

    run(repo_str, config, filters, pre, yes).await?;

    // Prune old versions if requested
    if prune {
        let root = install_root.unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".ghri"))
                .expect("Could not determine home directory")
        });
        let package_dir = root.join(&spec.repo.owner).join(&spec.repo.repo);
        let rt = crate::runtime::RealRuntime;
        prune_package_dir(&rt, &package_dir, &spec.repo.to_string())?;
    }

    Ok(())
}

#[tracing::instrument(skip(config, filters))]
pub async fn run<R: Runtime + 'static, G: GetReleases, E: Extractor>(
    repo_str: &str,
    config: Config<R, G, E>,
    filters: Vec<String>,
    pre: bool,
    yes: bool,
) -> Result<()> {
    let spec = repo_str.parse::<RepoSpec>()?;
    let installer = Installer::new(
        config.runtime,
        config.github,
        config.http_client,
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
        .await
}
