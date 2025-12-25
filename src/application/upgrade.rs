//! Upgrade action - orchestrates upgrading installed packages.
//!
//! This action coordinates:
//! - Finding installed packages
//! - Checking for available updates
//! - Delegating to InstallAction for actual installation

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::package::{Meta, PackageRepository};
use crate::provider::{Provider, ProviderFactory, RepoId};
use crate::runtime::Runtime;

use super::install::InstallAction;

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

/// Upgrade action - checks and performs package upgrades
pub struct UpgradeAction<'a, R: Runtime> {
    runtime: &'a R,
    package_repo: PackageRepository<'a, R>,
    provider_factory: &'a ProviderFactory,
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

    /// Get the install action for performing actual installations
    pub fn install_action(&self) -> InstallAction<'a, R> {
        InstallAction::new(
            self.runtime,
            self.provider_factory,
            self.install_root.clone(),
        )
    }

    /// Get the package repository
    pub fn package_repo(&self) -> &PackageRepository<'a, R> {
        &self.package_repo
    }

    /// Find all installed packages
    pub fn find_all_packages(&self) -> Result<Vec<(PathBuf, Meta)>> {
        self.package_repo.find_all_with_meta()
    }

    /// Check if a package has an available update
    pub fn check_update<'m>(&self, meta: &'m Meta, include_prerelease: bool) -> UpdateCheck<'m> {
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

    /// Filter packages by repository names
    pub fn filter_packages<'m>(
        &self,
        packages: &'m [(PathBuf, Meta)],
        filter_repos: &[RepoId],
    ) -> Vec<&'m (PathBuf, Meta)> {
        if filter_repos.is_empty() {
            packages.iter().collect()
        } else {
            packages
                .iter()
                .filter(|(_, meta)| {
                    if let Ok(repo) = meta.name.parse::<RepoId>() {
                        filter_repos.contains(&repo)
                    } else {
                        false
                    }
                })
                .collect()
        }
    }

    /// Resolve the source for a package from its metadata
    pub fn resolve_source(&self, meta: &Meta) -> Arc<dyn Provider> {
        self.provider_factory.provider_for_meta(meta)
    }

    /// Parse repository strings into RepoIds
    pub fn parse_repo_filters(&self, repos: &[String]) -> Vec<RepoId> {
        repos
            .iter()
            .filter_map(|r| r.parse::<RepoId>().ok())
            .collect()
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

    #[test]
    fn test_parse_repo_filters() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let factory = make_test_factory();
        let action = UpgradeAction::new(&runtime, &factory, "/test".into());

        let repos = vec![
            "owner1/repo1".to_string(),
            "owner2/repo2".to_string(),
            "invalid".to_string(), // Should be filtered out
        ];
        let filters = action.parse_repo_filters(&repos);

        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].owner, "owner1");
        assert_eq!(filters[1].owner, "owner2");
    }
}
