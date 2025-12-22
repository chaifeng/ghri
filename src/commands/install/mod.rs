use anyhow::Result;
use std::path::PathBuf;

use crate::{
    archive::Extractor,
    github::{GetReleases, RepoSpec},
    runtime::Runtime,
};

use super::config::Config;

mod download;
mod external_links;
mod installer;

pub use installer::Installer;

#[tracing::instrument(skip(runtime, install_root, api_url, filters))]
pub async fn install<R: Runtime + 'static>(
    runtime: R,
    repo_str: &str,
    install_root: Option<PathBuf>,
    api_url: Option<String>,
    filters: Vec<String>,
    pre: bool,
) -> Result<()> {
    let config = Config::new(runtime, install_root, api_url)?;
    run(repo_str, config, filters, pre).await
}

#[tracing::instrument(skip(config, filters))]
pub async fn run<R: Runtime + 'static, G: GetReleases, E: Extractor>(
    repo_str: &str,
    config: Config<R, G, E>,
    filters: Vec<String>,
    pre: bool,
) -> Result<()> {
    let spec = repo_str.parse::<RepoSpec>()?;
    let installer = Installer::new(
        config.runtime,
        config.github,
        config.http_client,
        config.extractor,
    );
    installer.install(&spec.repo, spec.version.as_deref(), config.install_root, filters, pre).await
}
