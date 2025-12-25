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

    /// Get the version directory for the current version of a package.
    ///
    /// Returns `None` if the current symlink doesn't exist or is invalid.
    /// The returned path is relative to install_root, constructed from
    /// the package directory and the symlink target (version name).
    pub fn current_version_dir(&self, owner: &str, repo: &str) -> Option<PathBuf> {
        let current_link = self.current_link(owner, repo);
        let link_target = self.runtime.read_link(&current_link).ok()?;
        // The link target is typically a relative path like "v1.2.0"
        // Join it with the package directory to get the full path
        Some(self.package_dir(owner, repo).join(link_target))
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

    /// Check if a package directory exists.
    ///
    /// This is a weaker check than `is_installed()` - the directory might exist
    /// without a meta.json file (e.g., partially installed or corrupted state).
    pub fn package_exists(&self, owner: &str, repo: &str) -> bool {
        self.runtime.exists(&self.package_dir(owner, repo))
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
    ///
    /// Link destinations (`links` and `versioned_links`) are converted to relative paths
    /// before saving. The relative paths are calculated from the package directory
    /// (parent of version directories), making them portable if the install root is moved.
    ///
    /// For example, if version_dir is `~/.ghri/owner/repo/v1.0.0` and link dest is
    /// `~/.local/bin/tool`, the stored relative path is `../../.local/bin/tool`
    /// (relative to `~/.ghri/owner/repo`, not `~/.ghri/owner/repo/v1.0.0`).
    pub fn save(&self, owner: &str, repo: &str, meta: &Meta) -> Result<()> {
        use crate::runtime::relative_path_from_dir;

        let meta_path = self.meta_path(owner, repo);
        let package_dir = self.package_dir(owner, repo);

        // Ensure parent directory exists
        if let Some(parent) = meta_path.parent()
            && !self.runtime.exists(parent)
        {
            self.runtime.create_dir_all(parent)?;
        }

        // Clone meta and convert link destinations to relative paths
        let mut meta_to_save = meta.clone();

        // Helper to get canonical package_dir only when needed
        // (when link.dest is absolute but package_dir is relative)
        let get_canonical_package_dir = || -> PathBuf {
            if package_dir.is_relative() {
                self.runtime
                    .canonicalize(&package_dir)
                    .unwrap_or_else(|_| package_dir.clone())
            } else {
                package_dir.clone()
            }
        };

        // Convert links destinations to relative paths (relative to package dir)
        for link in &mut meta_to_save.links {
            if link.dest.is_absolute() {
                let base_dir = if package_dir.is_relative() {
                    get_canonical_package_dir()
                } else {
                    package_dir.clone()
                };
                if let Some(relative) = relative_path_from_dir(&base_dir, &link.dest) {
                    link.dest = relative;
                }
            }
        }

        // Convert versioned_links destinations to relative paths (relative to package dir)
        for link in &mut meta_to_save.versioned_links {
            if link.dest.is_absolute() {
                let base_dir = if package_dir.is_relative() {
                    get_canonical_package_dir()
                } else {
                    package_dir.clone()
                };
                if let Some(relative) = relative_path_from_dir(&base_dir, &link.dest) {
                    link.dest = relative;
                }
            }
        }

        let content = serde_json::to_string_pretty(&meta_to_save)?;
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

    /// Check if the given version is the current version.
    ///
    /// Returns `true` if the 'current' symlink points to the given version.
    pub fn is_current_version(&self, owner: &str, repo: &str, version: &str) -> bool {
        self.current_version(owner, repo)
            .is_some_and(|current| current == version)
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

    #[test]
    fn test_is_current_version_true() {
        let mut runtime = MockRuntime::new();
        let current_link = PathBuf::from("/root/owner/repo/current");

        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("/root/owner/repo/v1.2.3")));

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        assert!(repo.is_current_version("owner", "repo", "v1.2.3"));
    }

    #[test]
    fn test_is_current_version_false_different_version() {
        let mut runtime = MockRuntime::new();
        let current_link = PathBuf::from("/root/owner/repo/current");

        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("/root/owner/repo/v2.0.0")));

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        assert!(!repo.is_current_version("owner", "repo", "v1.2.3"));
    }

    #[test]
    fn test_is_current_version_false_no_link() {
        let mut runtime = MockRuntime::new();
        let current_link = PathBuf::from("/root/owner/repo/current");

        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into())
            });

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        assert!(!repo.is_current_version("owner", "repo", "v1.2.3"));
    }

    #[test]
    fn test_current_version_dir_returns_full_path() {
        // When current symlink points to a relative path like "v1.2.3",
        // current_version_dir should return the full path: /root/owner/repo/v1.2.3
        let mut runtime = MockRuntime::new();
        let current_link = PathBuf::from("/root/owner/repo/current");

        // Symlink target is a relative path (as created by the installer)
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v1.2.3")));

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        let version_dir = repo.current_version_dir("owner", "repo");

        assert_eq!(version_dir, Some(PathBuf::from("/root/owner/repo/v1.2.3")));
    }

    #[test]
    fn test_current_version_dir_with_absolute_symlink_target() {
        // Even if symlink points to an absolute path, we should still construct
        // the path from package_dir + link_target for consistency
        let mut runtime = MockRuntime::new();
        let current_link = PathBuf::from("/root/owner/repo/current");

        // Symlink target is already an absolute path
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("/root/owner/repo/v1.2.3")));

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        let version_dir = repo.current_version_dir("owner", "repo");

        // Note: join with absolute path returns the absolute path itself
        assert_eq!(version_dir, Some(PathBuf::from("/root/owner/repo/v1.2.3")));
    }

    #[test]
    fn test_current_version_dir_no_symlink() {
        // When current symlink doesn't exist, should return None
        let mut runtime = MockRuntime::new();
        let current_link = PathBuf::from("/root/owner/repo/current");

        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into())
            });

        let repo = PackageRepository::new(&runtime, PathBuf::from("/root"));
        let version_dir = repo.current_version_dir("owner", "repo");

        assert_eq!(version_dir, None);
    }
}
