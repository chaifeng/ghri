//! Install use case - orchestrates the package installation flow.
//!
//! This use case coordinates:
//! - Provider resolution (from registry or package metadata)
//! - Version resolution
//! - Download and extraction
//! - Link management
//! - Metadata persistence

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use log::{info, warn};

use crate::package::{LinkManager, Meta, MetaRelease, PackageRepository, VersionResolver};
use crate::provider::{Provider, ProviderRegistry, RepoId};
use crate::runtime::Runtime;

/// Options for the install use case
#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Asset name filters (e.g., ["*linux*", "*x86_64*"])
    pub filters: Vec<String>,
    /// Allow installing pre-release versions
    pub pre: bool,
    /// Skip confirmation prompts
    pub yes: bool,
    /// Original command line arguments (for error messages)
    pub original_args: Vec<String>,
}

/// Result of resolving a version to install
#[derive(Debug)]
#[allow(dead_code)]
pub struct ResolvedInstall {
    /// The resolved release to install
    pub release: MetaRelease,
    /// Target directory for installation
    pub target_dir: PathBuf,
    /// Effective filters to use
    pub filters: Vec<String>,
}

/// Trait for install use case operations
///
/// This trait abstracts the install orchestration logic, enabling:
/// - Dependency injection for testing
/// - Mock implementations for unit tests
/// - Separation of concerns between command layer and business logic
#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait InstallOperations: Send + Sync {
    /// Get or load cached metadata for a package
    fn get_cached_meta(&self, repo: &RepoId) -> Result<Option<Meta>>;

    /// Fetch fresh metadata from source
    async fn fetch_meta(
        &self,
        repo: &RepoId,
        source: &dyn Provider,
        current_version: &str,
    ) -> Result<Meta>;

    /// Fetch fresh metadata using a specific API URL
    async fn fetch_meta_at(
        &self,
        repo: &RepoId,
        source: &dyn Provider,
        api_url: &str,
        current_version: &str,
    ) -> Result<Meta>;

    /// Get or fetch metadata, preferring cache
    async fn get_or_fetch_meta(&self, repo: &RepoId, source: &dyn Provider)
    -> Result<(Meta, bool)>;

    /// Resolve the version to install based on constraints
    /// Returns a cloned MetaRelease to avoid lifetime issues
    fn resolve_version(
        &self,
        meta: &Meta,
        version: Option<String>,
        pre: bool,
    ) -> Result<MetaRelease>;

    /// Get effective filters (user-provided or from saved meta)
    fn effective_filters(&self, options: &InstallOptions, meta: &Meta) -> Vec<String>;

    /// Check if a version is already installed
    fn is_installed(&self, repo: &RepoId, version: &str) -> bool;

    /// Get the version directory path
    fn version_dir(&self, repo: &RepoId, version: &str) -> PathBuf;

    /// Get the package directory path
    fn package_dir(&self, repo: &RepoId) -> PathBuf;

    /// Get the meta.json path for a package
    fn meta_path(&self, repo: &RepoId) -> PathBuf;

    /// Update the 'current' symlink after installation
    fn update_current_link(&self, repo: &RepoId, version: &str) -> Result<()>;

    /// Save metadata after successful installation
    fn save_meta(&self, repo: &RepoId, meta: &Meta) -> Result<()>;

    /// Resolve the source for a package (None = use default source)
    fn resolve_source_for_new(&self) -> Result<Arc<dyn Provider>>;

    /// Resolve source from existing metadata (for update/upgrade)
    fn resolve_source_for_existing(&self, meta: &Meta) -> Result<Arc<dyn Provider>>;
}

/// Install use case - platform-agnostic installation orchestration
pub struct InstallUseCase<'a, R: Runtime> {
    runtime: &'a R,
    package_repo: PackageRepository<'a, R>,
    source_registry: &'a ProviderRegistry,
    link_manager: LinkManager<'a, R>,
    install_root: PathBuf,
}

impl<'a, R: Runtime> InstallUseCase<'a, R> {
    /// Create a new install use case
    pub fn new(
        runtime: &'a R,
        source_registry: &'a ProviderRegistry,
        install_root: PathBuf,
    ) -> Self {
        Self {
            runtime,
            package_repo: PackageRepository::new(runtime, install_root.clone()),
            source_registry,
            link_manager: LinkManager::new(runtime),
            install_root,
        }
    }

    /// Get the package repository
    pub fn package_repo(&self) -> &PackageRepository<'a, R> {
        &self.package_repo
    }

    /// Get or load metadata for a package
    ///
    /// Returns (meta, needs_fetch) where needs_fetch indicates if metadata
    /// was loaded from cache (false) or needs to be fetched (true).
    pub fn get_cached_meta(&self, repo: &RepoId) -> Result<Option<Meta>> {
        self.package_repo.load(&repo.owner, &repo.repo)
    }

    /// Fetch fresh metadata from source
    pub async fn fetch_meta(
        &self,
        repo: &RepoId,
        source: &dyn Provider,
        current_version: &str,
    ) -> Result<Meta> {
        let api_url = source.api_url();
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

    /// Fetch fresh metadata using the API URL from existing metadata
    /// This is used for update operations where we want to use the saved API URL
    pub async fn fetch_meta_at(
        &self,
        repo: &RepoId,
        source: &dyn Provider,
        api_url: &str,
        current_version: &str,
    ) -> Result<Meta> {
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

    /// Get or fetch metadata, preferring cache
    pub async fn get_or_fetch_meta(
        &self,
        repo: &RepoId,
        source: &dyn Provider,
    ) -> Result<(Meta, bool)> {
        // Try to load from cache first
        match self.get_cached_meta(repo)? {
            Some(meta) => Ok((meta, false)),
            None => {
                let meta = self.fetch_meta(repo, source, "").await?;
                Ok((meta, true))
            }
        }
    }

    /// Resolve the version to install based on constraints
    pub fn resolve_version<'m>(
        &self,
        meta: &'m Meta,
        version: Option<&str>,
        pre: bool,
    ) -> Result<&'m MetaRelease> {
        if let Some(ver) = version {
            // Find specific version
            VersionResolver::find_exact(&meta.releases, ver).ok_or_else(|| {
                anyhow::anyhow!(
                    "Version '{}' not found for {}. Available versions: {}",
                    ver,
                    meta.name,
                    meta.releases
                        .iter()
                        .take(5)
                        .map(|r| r.version.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        } else if pre {
            // Latest including pre-releases
            meta.get_latest_release()
                .ok_or_else(|| anyhow::anyhow!("No release found for {}.", meta.name))
        } else {
            // Latest stable only
            meta.get_latest_stable_release().ok_or_else(|| {
                anyhow::anyhow!(
                    "No stable release found for {}. Use --pre for pre-releases.",
                    meta.name
                )
            })
        }
    }

    /// Get effective filters (user-provided or from saved meta)
    pub fn effective_filters(&self, options: &InstallOptions, meta: &Meta) -> Vec<String> {
        if options.filters.is_empty() && !meta.filters.is_empty() {
            info!("Using saved filters from meta: {:?}", meta.filters);
            meta.filters.clone()
        } else {
            options.filters.clone()
        }
    }

    /// Check if a version is already installed
    pub fn is_installed(&self, repo: &RepoId, version: &str) -> bool {
        let target_dir = self.version_dir(repo, version);
        self.runtime.exists(&target_dir)
    }

    /// Get the version directory path
    pub fn version_dir(&self, repo: &RepoId, version: &str) -> PathBuf {
        self.install_root
            .join(&repo.owner)
            .join(&repo.repo)
            .join(version)
    }

    /// Get the package directory path
    pub fn package_dir(&self, repo: &RepoId) -> PathBuf {
        self.install_root.join(&repo.owner).join(&repo.repo)
    }

    /// Update the 'current' symlink after installation
    pub fn update_current_link(&self, repo: &RepoId, version: &str) -> Result<()> {
        let package_dir = self.package_dir(repo);
        self.link_manager.update_current_link(&package_dir, version)
    }

    /// Update external links based on metadata
    ///
    /// Note: This is a simplified implementation. For full atomic update behavior,
    /// use `crate::commands::install::external_links::update_external_links` instead.
    #[allow(dead_code)]
    pub fn update_external_links(&self, meta: &Meta, version_dir: &Path) -> Result<()> {
        if let Some(package_dir) = version_dir.parent() {
            // Check and update valid links
            let (valid_links, _invalid_links) =
                self.link_manager.check_links(&meta.links, package_dir);

            for link_info in valid_links {
                if link_info.status.is_valid() || link_info.status.is_creatable() {
                    // Determine the target path
                    let target = if let Some(ref path) = link_info.path {
                        version_dir.join(path)
                    } else {
                        // Use default target (find executable in version_dir)
                        match self.link_manager.find_default_target(version_dir) {
                            Ok(t) => t,
                            Err(e) => {
                                warn!(
                                    "Failed to find default target for {}: {}. Skipping.",
                                    link_info.dest.display(),
                                    e
                                );
                                continue;
                            }
                        }
                    };

                    if let Err(e) = self.link_manager.create_link(&target, &link_info.dest) {
                        warn!(
                            "Failed to update link {}: {}. Continuing.",
                            link_info.dest.display(),
                            e
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Save metadata after successful installation
    pub fn save_meta(&self, repo: &RepoId, meta: &Meta) -> Result<()> {
        self.package_repo.save(&repo.owner, &repo.repo, meta)
    }

    /// Resolve the source for a package
    ///
    /// For new installs, uses the default source.
    /// For updates/upgrades, resolves from package metadata.
    pub fn resolve_source(&self, meta: Option<&Meta>) -> Result<&Arc<dyn Provider>> {
        match meta {
            Some(m) => self.source_registry.resolve_from_meta(m),
            None => self
                .source_registry
                .get(self.source_registry.default_kind())
                .ok_or_else(|| anyhow::anyhow!("No default source available")),
        }
    }

    /// Resolve source from existing metadata (for update/upgrade)
    pub fn resolve_source_from_meta(&self, meta: &Meta) -> Result<&Arc<dyn Provider>> {
        self.source_registry.resolve_from_meta(meta)
    }
}

// Implement InstallOperations trait for InstallUseCase
#[async_trait]
impl<'a, R: Runtime + 'static> InstallOperations for InstallUseCase<'a, R> {
    fn get_cached_meta(&self, repo: &RepoId) -> Result<Option<Meta>> {
        self.package_repo.load(&repo.owner, &repo.repo)
    }

    async fn fetch_meta(
        &self,
        repo: &RepoId,
        source: &dyn Provider,
        current_version: &str,
    ) -> Result<Meta> {
        let api_url = source.api_url();
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

    async fn fetch_meta_at(
        &self,
        repo: &RepoId,
        source: &dyn Provider,
        api_url: &str,
        current_version: &str,
    ) -> Result<Meta> {
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

    async fn get_or_fetch_meta(
        &self,
        repo: &RepoId,
        source: &dyn Provider,
    ) -> Result<(Meta, bool)> {
        match self.get_cached_meta(repo)? {
            Some(meta) => Ok((meta, false)),
            None => {
                let meta = InstallOperations::fetch_meta(self, repo, source, "").await?;
                Ok((meta, true))
            }
        }
    }

    fn resolve_version(
        &self,
        meta: &Meta,
        version: Option<String>,
        pre: bool,
    ) -> Result<MetaRelease> {
        if let Some(ver) = version {
            VersionResolver::find_exact(&meta.releases, &ver)
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Version '{}' not found for {}. Available versions: {}",
                        ver,
                        meta.name,
                        meta.releases
                            .iter()
                            .take(5)
                            .map(|r| r.version.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
        } else if pre {
            meta.get_latest_release()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("No release found for {}.", meta.name))
        } else {
            meta.get_latest_stable_release().cloned().ok_or_else(|| {
                anyhow::anyhow!(
                    "No stable release found for {}. Use --pre for pre-releases.",
                    meta.name
                )
            })
        }
    }

    fn effective_filters(&self, options: &InstallOptions, meta: &Meta) -> Vec<String> {
        if options.filters.is_empty() && !meta.filters.is_empty() {
            info!("Using saved filters from meta: {:?}", meta.filters);
            meta.filters.clone()
        } else {
            options.filters.clone()
        }
    }

    fn is_installed(&self, repo: &RepoId, version: &str) -> bool {
        let target_dir = self.version_dir(repo, version);
        self.runtime.exists(&target_dir)
    }

    fn version_dir(&self, repo: &RepoId, version: &str) -> PathBuf {
        self.install_root
            .join(&repo.owner)
            .join(&repo.repo)
            .join(version)
    }

    fn package_dir(&self, repo: &RepoId) -> PathBuf {
        self.install_root.join(&repo.owner).join(&repo.repo)
    }

    fn meta_path(&self, repo: &RepoId) -> PathBuf {
        self.package_repo.meta_path(&repo.owner, &repo.repo)
    }

    fn update_current_link(&self, repo: &RepoId, version: &str) -> Result<()> {
        let package_dir = self.package_dir(repo);
        self.link_manager.update_current_link(&package_dir, version)
    }

    fn save_meta(&self, repo: &RepoId, meta: &Meta) -> Result<()> {
        self.package_repo.save(&repo.owner, &repo.repo, meta)
    }

    fn resolve_source_for_new(&self) -> Result<Arc<dyn Provider>> {
        self.source_registry
            .get(self.source_registry.default_kind())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No default source available"))
    }

    fn resolve_source_for_existing(&self, meta: &Meta) -> Result<Arc<dyn Provider>> {
        self.source_registry.resolve_from_meta(meta).cloned()
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

    fn make_test_meta() -> Meta {
        Meta {
            name: "owner/repo".into(),
            api_url: "https://api.github.com".into(),
            current_version: "v1.0.0".into(),
            releases: vec![
                MetaRelease {
                    version: "v2.0.0".into(),
                    is_prerelease: false,
                    ..Default::default()
                },
                MetaRelease {
                    version: "v2.0.0-rc1".into(),
                    is_prerelease: true,
                    ..Default::default()
                },
                MetaRelease {
                    version: "v1.0.0".into(),
                    is_prerelease: false,
                    ..Default::default()
                },
            ],
            filters: vec!["*linux*".into()],
            ..Default::default()
        }
    }

    #[test]
    fn test_resolve_version_exact() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = InstallUseCase::new(&runtime, &registry, "/test".into());
        let meta = make_test_meta();

        let result = use_case.resolve_version(&meta, Some("v1.0.0"), false);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().version, "v1.0.0");
    }

    #[test]
    fn test_resolve_version_latest_stable() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = InstallUseCase::new(&runtime, &registry, "/test".into());
        let meta = make_test_meta();

        let result = use_case.resolve_version(&meta, None, false);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().version, "v2.0.0");
    }

    #[test]
    fn test_resolve_version_latest_with_pre() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = InstallUseCase::new(&runtime, &registry, "/test".into());

        // Meta with only pre-release as latest
        let meta = Meta {
            name: "owner/repo".into(),
            releases: vec![
                MetaRelease {
                    version: "v2.0.0-rc1".into(),
                    is_prerelease: true,
                    ..Default::default()
                },
                MetaRelease {
                    version: "v1.0.0".into(),
                    is_prerelease: false,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        // Without --pre, should get v1.0.0
        let result = use_case.resolve_version(&meta, None, false);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().version, "v1.0.0");

        // With --pre, should get v2.0.0-rc1
        let result = use_case.resolve_version(&meta, None, true);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().version, "v2.0.0-rc1");
    }

    #[test]
    fn test_resolve_version_not_found() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = InstallUseCase::new(&runtime, &registry, "/test".into());
        let meta = make_test_meta();

        let result = use_case.resolve_version(&meta, Some("v999.0.0"), false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_effective_filters_from_options() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = InstallUseCase::new(&runtime, &registry, "/test".into());
        let meta = make_test_meta();

        // User provides filters -> use them
        let options = InstallOptions {
            filters: vec!["*darwin*".into()],
            ..Default::default()
        };
        let filters = use_case.effective_filters(&options, &meta);
        assert_eq!(filters, vec!["*darwin*"]);
    }

    #[test]
    fn test_effective_filters_from_meta() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = InstallUseCase::new(&runtime, &registry, "/test".into());
        let meta = make_test_meta();

        // User provides no filters -> use saved from meta
        let options = InstallOptions::default();
        let filters = use_case.effective_filters(&options, &meta);
        assert_eq!(filters, vec!["*linux*"]);
    }

    #[test]
    fn test_version_dir() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = InstallUseCase::new(&runtime, &registry, "/root".into());

        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let dir = use_case.version_dir(&repo, "v1.0.0");
        assert_eq!(dir, PathBuf::from("/root/owner/repo/v1.0.0"));
    }

    #[test]
    fn test_package_dir() {
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| std::path::PathBuf::from("/tmp"));

        let registry = make_test_registry();
        let use_case = InstallUseCase::new(&runtime, &registry, "/root".into());

        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let dir = use_case.package_dir(&repo);
        assert_eq!(dir, PathBuf::from("/root/owner/repo"));
    }
}
