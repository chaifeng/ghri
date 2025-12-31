//! Upgrade action - orchestrates upgrading installed packages.
//!
//! This action coordinates:
//! - Finding installed packages
//! - Checking for available updates
//! - Returning packages that need upgrading

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use log::warn;

use crate::domain::model::Meta;
use crate::domain::service::PackageRepository;
use crate::provider::{Provider, ProviderFactory, RepoId};
use crate::runtime::Runtime;

/// Result of checking for an update
#[derive(Debug)]
pub struct UpdateCheck<'a> {
    /// The package metadata
    pub meta: &'a Meta,
    /// The latest available version (if update available)
    pub latest_version: Option<String>,
    /// Whether an update is available
    pub has_update: bool,
}

/// Information about a package that needs upgrading
#[derive(Debug, Clone)]
pub struct UpgradeCandidate {
    /// Repository identifier
    pub repo: RepoId,
    /// Current installed version
    pub current_version: String,
    /// Latest available version
    pub latest_version: String,
    /// Package metadata
    pub meta: Meta,
}

/// Result of checking all packages for upgrades
#[derive(Debug)]
pub struct UpgradeCheckResult {
    /// Packages that have updates available
    pub upgradable: Vec<UpgradeCandidate>,
    /// Packages that are already up to date
    pub up_to_date: Vec<(RepoId, String)>,
    /// Packages with no releases available
    pub no_releases: Vec<RepoId>,
}

/// Upgrade action - checks and performs package upgrades
pub struct UpgradeAction<'a, R: Runtime> {
    #[allow(dead_code)]
    runtime: &'a R,
    package_repo: PackageRepository<'a, R>,
    provider_factory: &'a ProviderFactory,
    #[allow(dead_code)]
    install_root: PathBuf,
}

impl<'a, R: Runtime> UpgradeAction<'a, R> {
    /// Create a new upgrade action
    pub fn new(
        runtime: &'a R,
        provider_factory: &'a ProviderFactory,
        install_root: PathBuf,
    ) -> Self {
        Self {
            runtime,
            package_repo: PackageRepository::new(runtime, install_root.clone()),
            provider_factory,
            install_root,
        }
    }

    /// Check all packages for available upgrades
    ///
    /// Returns categorized results: upgradable, up-to-date, no-releases
    pub fn check_all(
        &self,
        repo_filters: &[String],
        include_prerelease: bool,
    ) -> Result<UpgradeCheckResult> {
        let packages = self.package_repo.find_all_with_meta()?;

        // Parse filter repos
        let filter_repos: Vec<RepoId> = repo_filters
            .iter()
            .filter_map(|r| r.parse::<RepoId>().ok())
            .collect();

        let mut result = UpgradeCheckResult {
            upgradable: Vec::new(),
            up_to_date: Vec::new(),
            no_releases: Vec::new(),
        };

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

            // Check for available update
            let check = self.check_update(&meta, include_prerelease);

            match check.latest_version {
                Some(latest) if check.has_update => {
                    result.upgradable.push(UpgradeCandidate {
                        repo,
                        current_version: meta.current_version.clone(),
                        latest_version: latest,
                        meta,
                    });
                }
                Some(version) => {
                    result.up_to_date.push((repo, version));
                }
                None => {
                    result.no_releases.push(repo);
                }
            }
        }

        Ok(result)
    }

    /// Check if a package has an available update
    fn check_update<'m>(&self, meta: &'m Meta, include_prerelease: bool) -> UpdateCheck<'m> {
        let latest = if include_prerelease {
            meta.get_latest_release()
        } else {
            meta.get_latest_stable_release()
        };

        match latest {
            Some(release) => {
                let has_update = meta.current_version != release.tag;
                UpdateCheck {
                    meta,
                    latest_version: Some(release.tag.clone()),
                    has_update,
                }
            }
            None => UpdateCheck {
                meta,
                latest_version: None,
                has_update: false,
            },
        }
    }

    /// Resolve the source for a package from its metadata
    pub fn resolve_source(&self, meta: &Meta) -> Arc<dyn Provider> {
        self.provider_factory.provider_for_meta(meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpClient;
    use crate::provider::Release;
    use crate::runtime::MockRuntime;

    fn make_test_factory() -> ProviderFactory {
        let http_client = HttpClient::new(reqwest::Client::new());
        ProviderFactory::new(http_client, "https://api.github.com")
    }

    fn make_test_meta(current: &str, releases: Vec<(&str, bool)>) -> Meta {
        Meta {
            name: "owner/repo".into(),
            api_url: "https://api.github.com".into(),
            current_version: current.into(),
            releases: releases
                .into_iter()
                .map(|(v, pre)| Release {
                    tag: v.into(),
                    prerelease: pre,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_check_update_has_update() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let factory = make_test_factory();
        let action = UpgradeAction::new(&runtime, &factory, "/test".into());

        let meta = make_test_meta("v1.0.0", vec![("v2.0.0", false), ("v1.0.0", false)]);
        let check = action.check_update(&meta, false);

        assert!(check.has_update);
        assert_eq!(check.latest_version, Some("v2.0.0".into()));
    }

    #[test]
    fn test_check_update_already_latest() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let factory = make_test_factory();
        let action = UpgradeAction::new(&runtime, &factory, "/test".into());

        let meta = make_test_meta("v2.0.0", vec![("v2.0.0", false), ("v1.0.0", false)]);
        let check = action.check_update(&meta, false);

        assert!(!check.has_update);
        assert_eq!(check.latest_version, Some("v2.0.0".into()));
    }

    #[test]
    fn test_check_update_prerelease_available() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let factory = make_test_factory();
        let action = UpgradeAction::new(&runtime, &factory, "/test".into());

        let meta = make_test_meta("v1.0.0", vec![("v2.0.0-rc1", true), ("v1.0.0", false)]);

        // Without prerelease flag - no update (stable is v1.0.0)
        let check = action.check_update(&meta, false);
        assert!(!check.has_update);

        // With prerelease flag - has update
        let check = action.check_update(&meta, true);
        assert!(check.has_update);
        assert_eq!(check.latest_version, Some("v2.0.0-rc1".into()));
    }

    #[test]
    fn test_check_update_no_releases() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let factory = make_test_factory();
        let action = UpgradeAction::new(&runtime, &factory, "/test".into());

        let meta = make_test_meta("v1.0.0", vec![]);
        let check = action.check_update(&meta, false);

        assert!(!check.has_update);
        assert!(check.latest_version.is_none());
    }
}
