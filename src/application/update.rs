//! Update action - orchestrates refreshing package metadata.
//!
//! This action coordinates:
//! - Finding installed packages
//! - Filtering by user-specified repositories
//! - Fetching fresh metadata from sources
//! - Checking for available updates

use std::path::PathBuf;

use anyhow::Result;
use log::warn;

use crate::domain::model::{Meta, VersionResolver};
use crate::domain::service::PackageRepository;
use crate::provider::{Provider, ProviderFactory, RepoId};
use crate::runtime::Runtime;

/// Result of updating a single package's metadata
#[derive(Debug)]
pub struct UpdateResult {
    /// Repository identifier
    pub repo: RepoId,
    /// Current installed version
    pub current_version: String,
    /// Latest available version (if any)
    pub latest_version: Option<String>,
    /// Whether an update is available
    pub has_update: bool,
}

/// Update action - refreshes package metadata
pub struct UpdateAction<'a, R: Runtime> {
    #[allow(dead_code)]
    runtime: &'a R,
    package_repo: PackageRepository<'a, R>,
    provider_factory: &'a ProviderFactory,
}

impl<'a, R: Runtime> UpdateAction<'a, R> {
    /// Create a new update action
    pub fn new(
        runtime: &'a R,
        provider_factory: &'a ProviderFactory,
        install_root: PathBuf,
    ) -> Self {
        Self {
            runtime,
            package_repo: PackageRepository::new(runtime, install_root),
            provider_factory,
        }
    }

    /// Update metadata for all packages (or filtered by repos)
    ///
    /// Returns a list of update results for display
    pub async fn update_all(&self, repo_filters: &[String]) -> Result<Vec<UpdateResult>> {
        let packages = self.package_repo.find_all_with_meta()?;

        if packages.is_empty() {
            return Ok(vec![]);
        }

        // Parse filter repos
        let filter_repos: Vec<RepoId> = repo_filters
            .iter()
            .filter_map(|r| r.parse::<RepoId>().ok())
            .collect();

        let mut results = Vec::new();

        for (_meta_path, meta) in packages {
            let repo = match meta.name.parse::<RepoId>() {
                Ok(r) => r,
                Err(e) => {
                    warn!("Invalid repo name in meta: {}", e);
                    continue;
                }
            };

            // Skip if not in filter list (when filter is specified)
            if !filter_repos.is_empty() && !filter_repos.contains(&repo) {
                continue;
            }

            // Update this package
            match self.update_package(&repo, &meta).await {
                Ok(result) => results.push(result),
                Err(e) => {
                    warn!("Failed to update {}: {}", repo, e);
                }
            }
        }

        Ok(results)
    }

    /// Update metadata for a single package
    async fn update_package(&self, repo: &RepoId, meta: &Meta) -> Result<UpdateResult> {
        // Resolve source from package metadata
        let source = self.provider_factory.provider_for_meta(meta);

        // Fetch new metadata using saved API URL
        let new_meta = self
            .fetch_meta(repo, source.as_ref(), &meta.api_url, &meta.current_version)
            .await?;

        // Merge with existing metadata
        let mut final_meta = meta.clone();
        if final_meta.merge(new_meta.clone()) {
            final_meta.updated_at = new_meta.updated_at.clone();
        }

        // Save updated metadata
        self.package_repo
            .save(&repo.owner, &repo.repo, &final_meta)?;

        // Check if update is available
        let latest = VersionResolver::check_update(
            &final_meta.releases,
            &meta.current_version,
            false, // don't include prereleases
        );

        Ok(UpdateResult {
            repo: repo.clone(),
            current_version: meta.current_version.clone(),
            latest_version: latest.map(|r| r.tag.clone()),
            has_update: latest.is_some(),
        })
    }

    /// Fetch fresh metadata from source
    async fn fetch_meta(
        &self,
        repo: &RepoId,
        source: &dyn Provider,
        api_url: &str,
        current_version: &str,
    ) -> Result<Meta> {
        use anyhow::Context;

        let repo_info = source
            .get_repo_metadata_at(repo, api_url)
            .await
            .context("Failed to fetch repository metadata")?;
        let releases = source
            .get_releases_at(repo, api_url)
            .await
            .context("Failed to fetch releases")?;

        Ok(Meta::from(
            repo.clone(),
            repo_info,
            releases,
            current_version,
            api_url,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpClient;
    use crate::runtime::MockRuntime;

    fn make_test_factory() -> ProviderFactory {
        let http_client = HttpClient::new(reqwest::Client::new());
        ProviderFactory::new(http_client, "https://api.github.com")
    }

    #[test]
    fn test_update_action_creation() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let factory = make_test_factory();
        let _action = UpdateAction::new(&runtime, &factory, "/test".into());
    }
}
