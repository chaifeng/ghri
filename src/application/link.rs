//! Link action - manages package symlinks.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::domain::model::{LinkStatus, Meta};
use crate::domain::service::{LinkManager, PackageRepository};
use crate::runtime::Runtime;

/// Link action - manages symlinks for installed packages
pub struct LinkAction<'a, R: Runtime> {
    runtime: &'a R,
    package_repo: PackageRepository<'a, R>,
    link_manager: LinkManager<'a, R>,
}

impl<'a, R: Runtime> LinkAction<'a, R> {
    /// Create a new link action
    pub fn new(runtime: &'a R, install_root: PathBuf) -> Self {
        Self {
            runtime,
            package_repo: PackageRepository::new(runtime, install_root),
            link_manager: LinkManager::new(runtime),
        }
    }

    /// Get reference to runtime
    pub fn runtime(&self) -> &R {
        self.runtime
    }

    /// Get reference to package repository
    pub fn package_repo(&self) -> &PackageRepository<'a, R> {
        &self.package_repo
    }

    /// Get reference to link manager
    pub fn link_manager(&self) -> &LinkManager<'a, R> {
        &self.link_manager
    }

    /// Check if a package is installed
    pub fn is_installed(&self, owner: &str, repo: &str) -> bool {
        self.package_repo.is_installed(owner, repo)
    }

    /// Check if a specific version is installed
    pub fn is_version_installed(&self, owner: &str, repo: &str, version: &str) -> bool {
        self.package_repo.is_version_installed(owner, repo, version)
    }

    /// Load package metadata (required - fails if not found)
    pub fn load_meta(&self, owner: &str, repo: &str) -> Result<Meta> {
        self.package_repo.load_required(owner, repo)
    }

    /// Save package metadata
    pub fn save_meta(&self, owner: &str, repo: &str, meta: &Meta) -> Result<()> {
        self.package_repo.save(owner, repo, meta)
    }

    /// Get the package directory path
    pub fn package_dir(&self, owner: &str, repo: &str) -> PathBuf {
        self.package_repo.package_dir(owner, repo)
    }

    /// Get the version directory path
    pub fn version_dir(&self, owner: &str, repo: &str, version: &str) -> PathBuf {
        self.package_repo.version_dir(owner, repo, version)
    }

    /// Find the default link target in a version directory
    pub fn find_default_target(&self, version_dir: &Path) -> Result<PathBuf> {
        self.link_manager.find_default_target(version_dir)
    }

    /// Prepare a link destination (check conflicts, remove existing if safe)
    pub fn prepare_link_destination(&self, dest: &Path, package_dir: &Path) -> Result<()> {
        self.link_manager
            .prepare_link_destination(dest, package_dir)
    }

    /// Create a symlink
    pub fn create_link(&self, target: &Path, link: &Path) -> Result<()> {
        self.link_manager.create_link(target, link)
    }

    /// Remove a symlink
    pub fn remove_link(&self, link: &Path) -> Result<bool> {
        self.link_manager.remove_link(link)
    }

    /// Check link status
    pub fn check_link(&self, dest: &Path, expected_prefix: &Path) -> LinkStatus {
        self.link_manager.check_link(dest, expected_prefix)
    }

    /// Check if a path exists
    pub fn exists(&self, path: &Path) -> bool {
        self.runtime.exists(path)
    }

    /// Check if a path is a directory
    pub fn is_dir(&self, path: &Path) -> bool {
        self.runtime.is_dir(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;

    #[test]
    fn test_is_installed() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/test/root");
        let meta_path = root.join("owner/repo/meta.json");

        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| true);

        let action = LinkAction::new(&runtime, root);
        assert!(action.is_installed("owner", "repo"));
    }

    #[test]
    fn test_is_not_installed() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/test/root");
        let meta_path = root.join("owner/repo/meta.json");

        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| false);

        let action = LinkAction::new(&runtime, root);
        assert!(!action.is_installed("owner", "repo"));
    }
}
