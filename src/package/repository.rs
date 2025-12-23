//! Package repository for managing installed packages.
//!
//! This module provides a unified interface for all package management operations,
//! consolidating functionality that was previously scattered across commands.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::runtime::Runtime;

use super::{Meta, find_all_packages};

/// Repository for managing locally installed packages.
///
/// Provides a unified interface for:
/// - Loading and saving package metadata
/// - Finding installed packages
/// - Managing package directories and versions
pub struct PackageRepository<'a, R: Runtime> {
    runtime: &'a R,
    install_root: PathBuf,
}

impl<'a, R: Runtime> PackageRepository<'a, R> {
    /// Create a new package repository with the given runtime and install root.
    pub fn new(runtime: &'a R, install_root: PathBuf) -> Self {
        Self {
            runtime,
            install_root,
        }
    }

    /// Get the install root directory.
    pub fn install_root(&self) -> &Path {
        &self.install_root
    }

    /// Get the package directory for a given owner/repo.
    ///
    /// Returns: `<install_root>/<owner>/<repo>`
    pub fn package_dir(&self, owner: &str, repo: &str) -> PathBuf {
        self.install_root.join(owner).join(repo)
    }

    /// Get the version directory for a specific version of a package.
    ///
    /// Returns: `<install_root>/<owner>/<repo>/<version>`
    pub fn version_dir(&self, owner: &str, repo: &str, version: &str) -> PathBuf {
        self.package_dir(owner, repo).join(version)
    }

    /// Get the meta.json path for a package.
    ///
    /// Returns: `<install_root>/<owner>/<repo>/meta.json`
    pub fn meta_path(&self, owner: &str, repo: &str) -> PathBuf {
        self.package_dir(owner, repo).join("meta.json")
    }

    /// Get the 'current' symlink path for a package.
    ///
    /// Returns: `<install_root>/<owner>/<repo>/current`
    pub fn current_link(&self, owner: &str, repo: &str) -> PathBuf {
        self.package_dir(owner, repo).join("current")
    }

    /// Check if a package is installed (has meta.json).
    pub fn is_installed(&self, owner: &str, repo: &str) -> bool {
        self.runtime.exists(&self.meta_path(owner, repo))
    }

    /// Check if a specific version is installed.
    pub fn is_version_installed(&self, owner: &str, repo: &str, version: &str) -> bool {
        self.runtime.is_dir(&self.version_dir(owner, repo, version))
    }

    /// Load package metadata.
    ///
    /// Returns `None` if the package is not installed.
    pub fn load(&self, owner: &str, repo: &str) -> Result<Option<Meta>> {
        let meta_path = self.meta_path(owner, repo);
        if !self.runtime.exists(&meta_path) {
            return Ok(None);
        }
        Meta::load(self.runtime, &meta_path).map(Some)
    }

    /// Load package metadata, returning an error if not installed.
    pub fn load_required(&self, owner: &str, repo: &str) -> Result<Meta> {
        self.load(owner, repo)?
            .ok_or_else(|| anyhow::anyhow!("Package {}/{} is not installed", owner, repo))
    }

    /// Save package metadata.
    pub fn save(&self, owner: &str, repo: &str, meta: &Meta) -> Result<()> {
        let meta_path = self.meta_path(owner, repo);

        // Ensure parent directory exists
        if let Some(parent) = meta_path.parent()
            && !self.runtime.exists(parent)
        {
            self.runtime.create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(meta)?;
        self.runtime
            .write(&meta_path, content.as_bytes())
            .with_context(|| format!("Failed to save metadata to {:?}", meta_path))
    }

    /// Find all installed packages.
    ///
    /// Returns a list of meta.json paths for all installed packages.
    pub fn find_all(&self) -> Result<Vec<PathBuf>> {
        find_all_packages(self.runtime, &self.install_root)
    }

    /// Find all installed packages and load their metadata.
    pub fn find_all_with_meta(&self) -> Result<Vec<(PathBuf, Meta)>> {
        let meta_paths = self.find_all()?;
        let mut results = Vec::with_capacity(meta_paths.len());

        for meta_path in meta_paths {
            match Meta::load(self.runtime, &meta_path) {
                Ok(meta) => results.push((meta_path, meta)),
                Err(e) => {
                    log::warn!("Failed to load metadata from {:?}: {}", meta_path, e);
                }
            }
        }

        Ok(results)
    }

    /// Get list of installed versions for a package.
    ///
    /// Returns version directory names (e.g., ["v1.0.0", "v1.1.0"]).
    pub fn installed_versions(&self, owner: &str, repo: &str) -> Result<Vec<String>> {
        let package_dir = self.package_dir(owner, repo);
        if !self.runtime.exists(&package_dir) {
            return Ok(vec![]);
        }

        let mut versions = Vec::new();
        for entry in self.runtime.read_dir(&package_dir)? {
            // Skip meta.json and current symlink
            if let Some(name) = entry.file_name().and_then(|n| n.to_str())
                && name != "meta.json"
                && name != "current"
                && self.runtime.is_dir(&entry)
            {
                versions.push(name.to_string());
            }
        }

        Ok(versions)
    }

    /// Get the current version from the 'current' symlink.
    ///
    /// Returns `None` if the symlink doesn't exist or is invalid.
    pub fn current_version(&self, owner: &str, repo: &str) -> Option<String> {
        let current_link = self.current_link(owner, repo);
        self.runtime
            .read_link(&current_link)
            .ok()
            .and_then(|target| {
                target
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(String::from)
            })
    }

    /// Remove a specific version directory.
    ///
    /// Does not update the 'current' symlink or metadata.
    pub fn remove_version_dir(&self, owner: &str, repo: &str, version: &str) -> Result<()> {
        let version_dir = self.version_dir(owner, repo, version);
        if self.runtime.exists(&version_dir) {
            self.runtime.remove_dir_all(&version_dir)?;
        }
        Ok(())
    }

    /// Remove the entire package directory.
    pub fn remove_package_dir(&self, owner: &str, repo: &str) -> Result<()> {
        let package_dir = self.package_dir(owner, repo);
        if self.runtime.exists(&package_dir) {
            self.runtime.remove_dir_all(&package_dir)?;
        }

        // Try to remove empty owner directory
        let owner_dir = self.install_root.join(owner);
        if self.runtime.exists(&owner_dir)
            && let Ok(entries) = self.runtime.read_dir(&owner_dir)
            && entries.is_empty()
        {
            let _ = self.runtime.remove_dir_all(&owner_dir);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;

    #[test]
    fn test_package_dir() {
        let runtime = MockRuntime::new();
        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));

        assert_eq!(
            repo.package_dir("owner", "repo"),
            PathBuf::from("/root/owner/repo")
        );
    }

    #[test]
    fn test_version_dir() {
        let runtime = MockRuntime::new();
        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));

        assert_eq!(
            repo.version_dir("owner", "repo", "v1.0.0"),
            PathBuf::from("/root/owner/repo/v1.0.0")
        );
    }

    #[test]
    fn test_meta_path() {
        let runtime = MockRuntime::new();
        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));

        assert_eq!(
            repo.meta_path("owner", "repo"),
            PathBuf::from("/root/owner/repo/meta.json")
        );
    }

    #[test]
    fn test_is_installed_true() {
        let mut runtime = MockRuntime::new();
        let meta_path = PathBuf::from("/root/owner/repo/meta.json");

        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| true);

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        assert!(repo.is_installed("owner", "repo"));
    }

    #[test]
    fn test_is_installed_false() {
        let mut runtime = MockRuntime::new();
        let meta_path = PathBuf::from("/root/owner/repo/meta.json");

        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| false);

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        assert!(!repo.is_installed("owner", "repo"));
    }

    #[test]
    fn test_is_version_installed() {
        let mut runtime = MockRuntime::new();
        let version_dir = PathBuf::from("/root/owner/repo/v1.0.0");

        runtime
            .expect_is_dir()
            .with(eq(version_dir))
            .returning(|_| true);

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        assert!(repo.is_version_installed("owner", "repo", "v1.0.0"));
    }

    #[test]
    fn test_current_version() {
        let mut runtime = MockRuntime::new();
        let current_link = PathBuf::from("/root/owner/repo/current");

        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("/root/owner/repo/v1.2.3")));

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        assert_eq!(
            repo.current_version("owner", "repo"),
            Some("v1.2.3".to_string())
        );
    }

    #[test]
    fn test_current_version_no_link() {
        let mut runtime = MockRuntime::new();
        let current_link = PathBuf::from("/root/owner/repo/current");

        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into())
            });

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        assert_eq!(repo.current_version("owner", "repo"), None);
    }

    #[test]
    fn test_installed_versions() {
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/root/owner/repo");

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|p| {
                Ok(vec![
                    p.join("v1.0.0"),
                    p.join("v1.1.0"),
                    p.join("meta.json"),
                    p.join("current"),
                ])
            });

        // v1.0.0 and v1.1.0 are directories
        runtime
            .expect_is_dir()
            .with(eq(package_dir.join("v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(package_dir.join("v1.1.0")))
            .returning(|_| true);
        // meta.json and current are not directories
        runtime
            .expect_is_dir()
            .with(eq(package_dir.join("meta.json")))
            .returning(|_| false);
        runtime
            .expect_is_dir()
            .with(eq(package_dir.join("current")))
            .returning(|_| false);

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        let versions = repo.installed_versions("owner", "repo").unwrap();

        assert_eq!(versions.len(), 2);
        assert!(versions.contains(&"v1.0.0".to_string()));
        assert!(versions.contains(&"v1.1.0".to_string()));
    }

    #[test]
    fn test_installed_versions_not_installed() {
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/root/owner/repo");

        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| false);

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        let versions = repo.installed_versions("owner", "repo").unwrap();

        assert!(versions.is_empty());
    }
}
