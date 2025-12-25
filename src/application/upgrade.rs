//! Upgrade use case - orchestrates upgrading installed packages.
//!
//! This use case coordinates:
//! - Finding installed packages
//! - Checking for available updates
//! - Delegating to InstallUseCase for actual installation

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::package::{Meta, PackageRepository};
use crate::provider::{Provider, ProviderRegistry, RepoId};
use crate::runtime::Runtime;

use super::install::InstallUseCase;

/// Options for the upgrade use case
#[derive(Debug, Clone, Default)]
pub struct UpgradeOptions {
    /// Allow upgrading to pre-release versions
    pub pre: bool,
    /// Skip confirmation prompts
    pub yes: bool,
}

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

/// Upgrade use case - checks and performs package upgrades
pub struct UpgradeUseCase<'a, R: Runtime> {
    runtime: &'a R,
    package_repo: PackageRepository<'a, R>,
    provider_registry: &'a ProviderRegistry,
    install_root: PathBuf,
}

impl<'a, R: Runtime> UpgradeUseCase<'a, R> {
    /// Create a new upgrade use case
    pub fn new(
        runtime: &'a R,
        provider_registry: &'a ProviderRegistry,
        install_root: PathBuf,
    ) -> Self {
        Self {
            runtime,
            package_repo: PackageRepository::new(runtime, install_root.clone()),
            provider_registry,
            install_root,
        }
    }

    /// Get the install use case for performing actual installations
    pub fn install_use_case(&self) -> InstallUseCase<'a, R> {
        InstallUseCase::new(
            self.runtime,
            self.provider_registry,
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
                let has_update = meta.current_version != release.version;
                UpdateCheck {
                    meta,
                    latest_version: Some(release.version.clone()),
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
    pub fn resolve_source(&self, meta: &Meta) -> Result<&Arc<dyn Provider>> {
        self.provider_registry.resolve_from_meta(meta)
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
    use crate::package::MetaRelease;
    use crate::provider::{MockProvider, ProviderKind};
    use crate::runtime::MockRuntime;
    use std::sync::Arc;

    fn make_test_registry() -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        let mut mock = MockProvider::new();
        mock.expect_kind().return_const(ProviderKind::GitHub);
        mock.expect_api_url()
            .return_const("https://api.github.com".to_string());
        registry.register(Arc::new(mock));
        registry
    }

    fn make_test_meta(current: &str, releases: Vec<(&str, bool)>) -> Meta {
        Meta {
            name: "owner/repo".into(),
            api_url: "https://api.github.com".into(),
            current_version: current.into(),
            releases: releases
                .into_iter()
                .map(|(v, pre)| MetaRelease {
                    version: v.into(),
                    is_prerelease: pre,
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

        let registry = make_test_registry();
        let use_case = UpgradeUseCase::new(&runtime, &registry, "/test".into());

        let meta = make_test_meta("v1.0.0", vec![("v2.0.0", false), ("v1.0.0", false)]);
        let check = use_case.check_update(&meta, false);

        assert!(check.has_update);
        assert_eq!(check.latest_version, Some("v2.0.0".into()));
    }

    #[test]
    fn test_check_update_already_latest() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = UpgradeUseCase::new(&runtime, &registry, "/test".into());

        let meta = make_test_meta("v2.0.0", vec![("v2.0.0", false), ("v1.0.0", false)]);
        let check = use_case.check_update(&meta, false);

        assert!(!check.has_update);
        assert_eq!(check.latest_version, Some("v2.0.0".into()));
    }

    #[test]
    fn test_check_update_prerelease_available() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = UpgradeUseCase::new(&runtime, &registry, "/test".into());

        let meta = make_test_meta("v1.0.0", vec![("v2.0.0-rc1", true), ("v1.0.0", false)]);

        // Without prerelease flag - no update (stable is v1.0.0)
        let check = use_case.check_update(&meta, false);
        assert!(!check.has_update);

        // With prerelease flag - has update
        let check = use_case.check_update(&meta, true);
        assert!(check.has_update);
        assert_eq!(check.latest_version, Some("v2.0.0-rc1".into()));
    }

    #[test]
    fn test_check_update_no_releases() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = UpgradeUseCase::new(&runtime, &registry, "/test".into());

        let meta = make_test_meta("v1.0.0", vec![]);
        let check = use_case.check_update(&meta, false);

        assert!(!check.has_update);
        assert!(check.latest_version.is_none());
    }

    #[test]
    fn test_parse_repo_filters() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = UpgradeUseCase::new(&runtime, &registry, "/test".into());

        let repos = vec![
            "owner1/repo1".to_string(),
            "owner2/repo2".to_string(),
            "invalid".to_string(), // Should be filtered out
        ];
        let filters = use_case.parse_repo_filters(&repos);

        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].owner, "owner1");
        assert_eq!(filters[1].owner, "owner2");
    }
}
