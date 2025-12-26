//! Link management for packages.
//!
//! This module provides a unified interface for managing symlinks
//! associated with installed packages, including creation, validation,
//! and removal of external links.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::package::{LinkRule, VersionedLink};
use crate::runtime::{Runtime, is_path_under, relative_symlink_path};

/// Status of a symlink check.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkStatus {
    /// Link exists and points to the expected location
    Valid,
    /// Link doesn't exist yet (will be created)
    NotExists,
    /// Link exists but points to a different location
    WrongTarget,
    /// Path exists but is not a symlink
    NotSymlink,
    /// Cannot resolve the symlink target
    Unresolvable,
}

/// Result of a safe link removal operation.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoveLinkResult {
    /// Link was successfully removed
    Removed,
    /// Link doesn't exist (nothing to remove)
    NotExists,
    /// Path exists but is not a symlink
    NotSymlink,
    /// Link points to external path (not under expected prefix)
    ExternalTarget,
    /// Cannot resolve the symlink target
    Unresolvable,
}

impl LinkStatus {
    /// Get a human-readable reason for this status.
    pub fn reason(&self) -> &'static str {
        match self {
            LinkStatus::Valid => "valid",
            LinkStatus::NotExists => "does not exist",
            LinkStatus::WrongTarget => "points to different location",
            LinkStatus::NotSymlink => "not a symlink",
            LinkStatus::Unresolvable => "cannot resolve target",
        }
    }

    /// Check if this status indicates a valid link.
    pub fn is_valid(&self) -> bool {
        matches!(self, LinkStatus::Valid)
    }

    /// Check if this status indicates the link will be created.
    pub fn is_creatable(&self) -> bool {
        matches!(self, LinkStatus::NotExists)
    }

    /// Check if this status indicates a problem.
    pub fn is_problematic(&self) -> bool {
        matches!(
            self,
            LinkStatus::WrongTarget | LinkStatus::NotSymlink | LinkStatus::Unresolvable
        )
    }
}

/// A checked link with its status.
#[derive(Debug, Clone)]
pub struct CheckedLink {
    /// The destination path (symlink location)
    pub dest: PathBuf,
    /// The status of this link
    pub status: LinkStatus,
    /// Optional path within version directory
    pub path: Option<String>,
}

/// Result of validating a link for creation/update.
#[derive(Debug)]
pub enum LinkValidation {
    /// Link is valid and ready to be created/updated
    Valid {
        /// The link target (source file in version directory)
        target: PathBuf,
        /// The destination path (symlink location)
        dest: PathBuf,
        /// Whether the destination already exists and needs to be removed first
        needs_removal: bool,
    },
    /// Link should be skipped
    Skip { dest: PathBuf, reason: String },
    /// Link validation failed with an error
    Error { dest: PathBuf, error: String },
}

/// Link manager for handling symlinks associated with packages.
///
/// Provides methods for:
/// - Checking link status
/// - Validating links before creation
/// - Creating and removing symlinks
/// - Categorizing links by status
pub struct LinkManager<'a, R: Runtime> {
    runtime: &'a R,
}

impl<'a, R: Runtime> LinkManager<'a, R> {
    /// Create a new link manager.
    pub fn new(runtime: &'a R) -> Self {
        Self { runtime }
    }

    /// Check if a symlink at `dest` points to somewhere under `expected_prefix`.
    pub fn check_link(&self, dest: &Path, expected_prefix: &Path) -> LinkStatus {
        if self.runtime.is_symlink(dest) {
            if let Ok(resolved_target) = self.runtime.resolve_link(dest) {
                // Ensure both paths are absolute for correct comparison
                // If resolved_target is relative, canonicalize it
                let abs_target = if resolved_target.is_relative() {
                    self.runtime
                        .canonicalize(&resolved_target)
                        .unwrap_or(resolved_target)
                } else {
                    resolved_target
                };

                let abs_prefix = if expected_prefix.is_relative() {
                    self.runtime
                        .canonicalize(expected_prefix)
                        .unwrap_or(expected_prefix.to_path_buf())
                } else {
                    expected_prefix.to_path_buf()
                };

                if is_path_under(&abs_target, &abs_prefix) {
                    LinkStatus::Valid
                } else {
                    LinkStatus::WrongTarget
                }
            } else {
                LinkStatus::Unresolvable
            }
        } else if self.runtime.exists(dest) {
            LinkStatus::NotSymlink
        } else {
            LinkStatus::NotExists
        }
    }

    /// Check all links in a list and categorize them.
    ///
    /// Returns (valid_or_creatable, problematic) links.
    pub fn check_links(
        &self,
        links: &[LinkRule],
        expected_prefix: &Path,
    ) -> (Vec<CheckedLink>, Vec<CheckedLink>) {
        let mut valid = Vec::new();
        let mut invalid = Vec::new();

        for link in links {
            let status = self.check_link(&link.dest, expected_prefix);
            let checked = CheckedLink {
                dest: link.dest.clone(),
                status: status.clone(),
                path: link.path.clone(),
            };

            if status.is_valid() || status.is_creatable() {
                valid.push(checked);
            } else {
                invalid.push(checked);
            }
        }

        (valid, invalid)
    }

    /// Check versioned links and categorize them.
    pub fn check_versioned_links(
        &self,
        links: &[VersionedLink],
        package_dir: &Path,
    ) -> (Vec<CheckedLink>, Vec<CheckedLink>) {
        let mut valid = Vec::new();
        let mut invalid = Vec::new();

        for link in links {
            let version_dir = package_dir.join(&link.version);
            let status = self.check_link(&link.dest, &version_dir);
            let checked = CheckedLink {
                dest: link.dest.clone(),
                status: status.clone(),
                path: link.path.clone(),
            };

            if status.is_valid() || status.is_creatable() {
                valid.push(checked);
            } else {
                invalid.push(checked);
            }
        }

        (valid, invalid)
    }

    /// Check versioned links for a specific version.
    pub fn check_versioned_links_for_version(
        &self,
        links: &[VersionedLink],
        version: &str,
        expected_prefix: &Path,
    ) -> (Vec<CheckedLink>, Vec<CheckedLink>) {
        let filtered: Vec<_> = links.iter().filter(|l| l.version == version).collect();

        let mut valid = Vec::new();
        let mut invalid = Vec::new();

        for link in filtered {
            let status = self.check_link(&link.dest, expected_prefix);
            let checked = CheckedLink {
                dest: link.dest.clone(),
                status: status.clone(),
                path: link.path.clone(),
            };

            if status.is_valid() || status.is_creatable() {
                valid.push(checked);
            } else {
                invalid.push(checked);
            }
        }

        (valid, invalid)
    }

    /// Validate a link rule for creation.
    ///
    /// Checks if the link can be created and whether any cleanup is needed.
    pub fn validate_link(
        &self,
        rule: &LinkRule,
        version_dir: &Path,
        package_dir: &Path,
    ) -> LinkValidation {
        // Determine target path
        let target = if let Some(ref path) = rule.path {
            let t = version_dir.join(path);
            if !self.runtime.exists(&t) {
                return LinkValidation::Error {
                    dest: rule.dest.clone(),
                    error: format!("Path '{}' does not exist in {:?}", path, version_dir),
                };
            }
            t
        } else {
            // Auto-detect: single file or directory
            match self.find_default_target(version_dir) {
                Ok(target) => target,
                Err(e) => {
                    return LinkValidation::Error {
                        dest: rule.dest.clone(),
                        error: format!("Failed to scan version directory: {}", e),
                    };
                }
            }
        };

        // Check destination
        if self.runtime.exists(&rule.dest) {
            if !self.runtime.is_symlink(&rule.dest) {
                return LinkValidation::Skip {
                    dest: rule.dest.clone(),
                    reason: "destination exists but is not a symlink".to_string(),
                };
            }

            // Check if symlink points to within our package
            if let Ok(existing_target) = self.runtime.resolve_link(&rule.dest)
                && !is_path_under(&existing_target, package_dir)
            {
                return LinkValidation::Skip {
                    dest: rule.dest.clone(),
                    reason: format!(
                        "symlink points to external path {:?}, not managed by this package",
                        existing_target
                    ),
                };
            }

            LinkValidation::Valid {
                target,
                dest: rule.dest.clone(),
                needs_removal: true,
            }
        } else {
            LinkValidation::Valid {
                target,
                dest: rule.dest.clone(),
                needs_removal: false,
            }
        }
    }

    /// Create a symlink from `dest` to `target`.
    ///
    /// Uses relative paths when possible for portability.
    /// Both `target` and `dest` are expected to be absolute paths.
    pub fn create_link(&self, target: &Path, dest: &Path) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = dest.parent()
            && !self.runtime.exists(parent)
        {
            self.runtime.create_dir_all(parent)?;
        }

        // Calculate relative path if possible (from symlink location to target)
        if let Some(link_target) = relative_symlink_path(dest, target) {
            self.runtime.symlink(&link_target, dest)
        } else {
            self.runtime.symlink(target, dest)
        }
    }

    /// Remove a symlink if it exists and is a symlink.
    pub fn remove_link(&self, dest: &Path) -> Result<bool> {
        if self.runtime.is_symlink(dest) {
            self.runtime.remove_symlink(dest)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Remove a symlink only if it points to somewhere under `prefix`.
    ///
    /// This is a safety check to avoid removing symlinks managed by other tools.
    pub fn remove_link_if_under(&self, dest: &Path, prefix: &Path) -> Result<bool> {
        if !self.runtime.is_symlink(dest) {
            return Ok(false);
        }

        if let Ok(target) = self.runtime.resolve_link(dest)
            && is_path_under(&target, prefix)
        {
            self.runtime.remove_symlink(dest)?;
            return Ok(true);
        }

        Ok(false)
    }

    /// Safely remove a symlink with detailed status reporting.
    ///
    /// This method checks:
    /// - If the path exists
    /// - If it's a symlink
    /// - If it points to somewhere under `prefix`
    ///
    /// Returns a `RemoveLinkResult` indicating what happened:
    /// - `Removed`: The symlink was successfully removed
    /// - `NotExists`: The path doesn't exist (nothing to remove)
    /// - `NotSymlink`: The path exists but is not a symlink
    /// - `ExternalTarget`: The symlink points outside the expected prefix
    /// - `Unresolvable`: Cannot resolve the symlink target
    pub fn remove_link_safely(&self, dest: &Path, prefix: &Path) -> Result<RemoveLinkResult> {
        // Check if path exists at all
        if !self.runtime.exists(dest) && !self.runtime.is_symlink(dest) {
            return Ok(RemoveLinkResult::NotExists);
        }

        // Check if it's a symlink
        if !self.runtime.is_symlink(dest) {
            return Ok(RemoveLinkResult::NotSymlink);
        }

        // Try to resolve the symlink target
        match self.runtime.resolve_link(dest) {
            Ok(target) => {
                if is_path_under(&target, prefix) {
                    // Safe to remove - points within expected prefix
                    self.runtime.remove_symlink(dest)?;
                    Ok(RemoveLinkResult::Removed)
                } else {
                    // Points outside expected prefix - don't remove
                    Ok(RemoveLinkResult::ExternalTarget)
                }
            }
            Err(_) => {
                // Cannot resolve the symlink
                Ok(RemoveLinkResult::Unresolvable)
            }
        }
    }

    /// Update the 'current' symlink in a package directory to point to a specific version.
    ///
    /// The `current` symlink is located in the package directory and points to a version subdirectory.
    /// For example: `/home/user/.ghri/owner/repo/current` -> `v1.0.0`
    ///
    /// This method:
    /// - Creates a new symlink if it doesn't exist
    /// - Updates an existing symlink if it points to a different version
    /// - Does nothing if the symlink already points to the correct version
    /// - Returns an error if `current` exists but is not a symlink
    pub fn update_current_link(&self, package_dir: &Path, version: &str) -> Result<()> {
        let current_link = package_dir.join("current");
        let link_target = Path::new(version);

        if self.runtime.exists(&current_link) {
            if !self.runtime.is_symlink(&current_link) {
                anyhow::bail!("'current' exists but is not a symlink");
            }

            match self.runtime.read_link(&current_link) {
                Ok(target) => {
                    // Normalize paths for comparison
                    let existing_target = target.components().as_path();
                    let new_target = link_target.components().as_path();

                    if existing_target == new_target {
                        log::debug!("'current' symlink already points to the correct version");
                        return Ok(());
                    }

                    log::debug!(
                        "'current' symlink points to {:?}, updating to {:?}",
                        existing_target,
                        new_target
                    );
                    self.runtime.remove_symlink(&current_link)?;
                }
                Err(_) => {
                    log::debug!("'current' symlink is unreadable, recreating...");
                    self.runtime.remove_symlink(&current_link)?;
                }
            }
        }

        self.runtime.symlink(link_target, &current_link)?;
        Ok(())
    }

    /// Check if destination can be updated (for creating/updating a link).
    ///
    /// Returns Ok(true) if the destination doesn't exist or is a symlink pointing to within package_dir.
    /// Returns Ok(false) if the destination doesn't exist (safe to create).
    /// Returns Err if:
    /// - Destination exists but is not a symlink
    /// - Destination is a symlink pointing outside package_dir
    /// - Destination is a symlink but cannot be read
    pub fn can_update_link(&self, dest: &Path, package_dir: &Path) -> Result<bool> {
        // Check if dest or symlink exists
        if !self.runtime.exists(dest) && !self.runtime.is_symlink(dest) {
            return Ok(false); // Doesn't exist, safe to create
        }

        if !self.runtime.is_symlink(dest) {
            anyhow::bail!("Destination {:?} already exists and is not a symlink", dest);
        }

        // It's a symlink, check where it points
        let existing_target = self.runtime.resolve_link(dest).map_err(|_| {
            anyhow::anyhow!(
                "Destination {:?} is a symlink but cannot read its target",
                dest
            )
        })?;

        if is_path_under(&existing_target, package_dir) {
            Ok(true) // Points within package, can be updated
        } else {
            anyhow::bail!(
                "Destination {:?} exists and points to {:?} which is not managed by this package",
                dest,
                existing_target
            )
        }
    }

    /// Prepare destination for link creation/update.
    ///
    /// If the destination is a symlink pointing within package_dir, removes it.
    /// Returns Ok(()) if ready to create the new link.
    /// Returns Err if the destination cannot be updated.
    pub fn prepare_link_destination(&self, dest: &Path, package_dir: &Path) -> Result<()> {
        if self.can_update_link(dest, package_dir)? {
            // Symlink exists and points within package, remove it
            self.remove_link(dest)?;
        }
        Ok(())
    }

    /// Find the default link target in a version directory.
    ///
    /// If there's a single non-directory entry, returns it.
    /// Otherwise, returns the directory itself.
    pub fn find_default_target(&self, version_dir: &Path) -> Result<PathBuf> {
        let entries = self.runtime.read_dir(version_dir)?;

        if entries.len() == 1 {
            let single_entry = &entries[0];
            if !self.runtime.is_dir(single_entry) {
                return Ok(single_entry.clone());
            }
        }

        // Multiple entries or single directory - use version dir itself
        Ok(version_dir.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use crate::test_utils::{test_bin_dir, test_other_path, test_root};
    use mockall::predicate::eq;

    #[test]
    fn test_check_link_valid() {
        let mut runtime = MockRuntime::new();
        let dest = test_bin_dir().join("tool");
        let prefix = test_root().join("owner").join("repo");
        let target = test_root()
            .join("owner")
            .join("repo")
            .join("v1")
            .join("tool");

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(target.clone()));

        let manager = LinkManager::new(&runtime);
        let status = manager.check_link(&dest, &prefix);
        assert!(status.is_valid());
    }

    #[test]
    fn test_check_link_not_exists() {
        let mut runtime = MockRuntime::new();
        let dest = test_bin_dir().join("tool");
        let prefix = test_root().join("owner").join("repo");

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        let manager = LinkManager::new(&runtime);
        let status = manager.check_link(&dest, &prefix);
        assert!(status.is_creatable());
    }

    #[test]
    fn test_check_link_wrong_target() {
        let mut runtime = MockRuntime::new();
        let dest = test_bin_dir().join("tool");
        let prefix = test_root().join("owner").join("repo");
        let target = test_other_path(); // Not under prefix

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(target.clone()));

        let manager = LinkManager::new(&runtime);
        let status = manager.check_link(&dest, &prefix);
        assert_eq!(status, LinkStatus::WrongTarget);
    }

    #[test]
    fn test_check_link_not_symlink() {
        let mut runtime = MockRuntime::new();
        let dest = test_bin_dir().join("tool");
        let prefix = test_root().join("owner").join("repo");

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);

        let manager = LinkManager::new(&runtime);
        let status = manager.check_link(&dest, &prefix);
        assert_eq!(status, LinkStatus::NotSymlink);
    }

    #[test]
    fn test_check_links_categorizes() {
        let mut runtime = MockRuntime::new();
        let prefix = test_root().join("owner").join("repo");

        let valid_dest = test_bin_dir().join("valid");
        let invalid_dest = test_bin_dir().join("invalid");
        let valid_target = test_root()
            .join("owner")
            .join("repo")
            .join("v1")
            .join("tool");

        // Valid link
        runtime
            .expect_is_symlink()
            .with(eq(valid_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(valid_dest.clone()))
            .returning(move |_| Ok(valid_target.clone()));

        // Invalid link (not a symlink)
        runtime
            .expect_is_symlink()
            .with(eq(invalid_dest.clone()))
            .returning(|_| false);
        runtime
            .expect_exists()
            .with(eq(invalid_dest.clone()))
            .returning(|_| true);

        let links = vec![
            LinkRule {
                dest: valid_dest,
                path: None,
            },
            LinkRule {
                dest: invalid_dest,
                path: None,
            },
        ];

        let manager = LinkManager::new(&runtime);
        let (valid, invalid) = manager.check_links(&links, &prefix);

        assert_eq!(valid.len(), 1);
        assert_eq!(invalid.len(), 1);
    }

    #[test]
    fn test_validate_link_valid() {
        let mut runtime = MockRuntime::new();
        let version_dir = test_root().join("owner").join("repo").join("v1");
        let package_dir = test_root().join("owner").join("repo");
        let dest = test_bin_dir().join("tool");
        let target = version_dir.join("bin").join("tool");

        // Path exists
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| true);

        // Dest doesn't exist
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        let rule = LinkRule {
            dest: dest.clone(),
            path: Some("bin/tool".to_string()),
        };

        let manager = LinkManager::new(&runtime);
        let result = manager.validate_link(&rule, &version_dir, &package_dir);

        match result {
            LinkValidation::Valid {
                target: t,
                dest: d,
                needs_removal,
            } => {
                assert_eq!(t, target);
                assert_eq!(d, dest);
                assert!(!needs_removal);
            }
            _ => panic!("Expected Valid result"),
        }
    }

    #[test]
    fn test_validate_link_path_not_exists() {
        let mut runtime = MockRuntime::new();
        let version_dir = test_root().join("owner").join("repo").join("v1");
        let package_dir = test_root().join("owner").join("repo");
        let dest = test_bin_dir().join("tool");
        let target = version_dir.join("bin").join("notfound");

        // Path doesn't exist
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| false);

        let rule = LinkRule {
            dest: dest.clone(),
            path: Some("bin/notfound".to_string()),
        };

        let manager = LinkManager::new(&runtime);
        let result = manager.validate_link(&rule, &version_dir, &package_dir);

        match result {
            LinkValidation::Error { dest: d, error } => {
                assert_eq!(d, dest);
                assert!(error.contains("does not exist"));
            }
            _ => panic!("Expected Error result"),
        }
    }

    #[test]
    fn test_validate_link_dest_not_symlink() {
        let mut runtime = MockRuntime::new();
        let version_dir = test_root().join("owner").join("repo").join("v1");
        let package_dir = test_root().join("owner").join("repo");
        let dest = test_bin_dir().join("tool");
        let target = version_dir.join("bin").join("tool");

        // Path exists
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| true);

        // Dest exists but is not a symlink
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        let rule = LinkRule {
            dest: dest.clone(),
            path: Some("bin/tool".to_string()),
        };

        let manager = LinkManager::new(&runtime);
        let result = manager.validate_link(&rule, &version_dir, &package_dir);

        match result {
            LinkValidation::Skip { dest: d, reason } => {
                assert_eq!(d, dest);
                assert!(reason.contains("not a symlink"));
            }
            _ => panic!("Expected Skip result"),
        }
    }

    #[test]
    fn test_link_status_methods() {
        assert!(LinkStatus::Valid.is_valid());
        assert!(!LinkStatus::Valid.is_creatable());
        assert!(!LinkStatus::Valid.is_problematic());

        assert!(!LinkStatus::NotExists.is_valid());
        assert!(LinkStatus::NotExists.is_creatable());
        assert!(!LinkStatus::NotExists.is_problematic());

        assert!(!LinkStatus::WrongTarget.is_valid());
        assert!(!LinkStatus::WrongTarget.is_creatable());
        assert!(LinkStatus::WrongTarget.is_problematic());

        assert!(!LinkStatus::NotSymlink.is_valid());
        assert!(!LinkStatus::NotSymlink.is_creatable());
        assert!(LinkStatus::NotSymlink.is_problematic());
    }

    #[test]
    fn test_can_update_link_not_exists() {
        let mut runtime = MockRuntime::new();
        let dest = test_bin_dir().join("tool");
        let package_dir = test_root().join("owner").join("repo");

        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        let manager = LinkManager::new(&runtime);
        let result = manager.can_update_link(&dest, &package_dir);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Returns false (doesn't exist, safe to create)
    }

    #[test]
    fn test_can_update_link_symlink_in_package() {
        let mut runtime = MockRuntime::new();
        let dest = test_bin_dir().join("tool");
        let package_dir = test_root().join("owner").join("repo");
        let target = test_root()
            .join("owner")
            .join("repo")
            .join("v1")
            .join("tool");

        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(target.clone()));

        let manager = LinkManager::new(&runtime);
        let result = manager.can_update_link(&dest, &package_dir);
        assert!(result.is_ok());
        assert!(result.unwrap()); // Returns true (can be updated)
    }

    #[test]
    fn test_can_update_link_symlink_outside_package() {
        let mut runtime = MockRuntime::new();
        let dest = test_bin_dir().join("tool");
        let package_dir = test_root().join("owner").join("repo");
        let target = test_other_path().join("tool");

        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(target.clone()));

        let manager = LinkManager::new(&runtime);
        let result = manager.can_update_link(&dest, &package_dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not managed"));
    }

    #[test]
    fn test_can_update_link_not_a_symlink() {
        let mut runtime = MockRuntime::new();
        let dest = test_bin_dir().join("tool");
        let package_dir = test_root().join("owner").join("repo");

        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        let manager = LinkManager::new(&runtime);
        let result = manager.can_update_link(&dest, &package_dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a symlink"));
    }

    #[test]
    fn test_prepare_link_destination_removes_existing() {
        let mut runtime = MockRuntime::new();
        let dest = test_bin_dir().join("tool");
        let package_dir = test_root().join("owner").join("repo");
        let target = test_root()
            .join("owner")
            .join("repo")
            .join("v1")
            .join("tool");

        // can_update_link checks
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(target.clone()));

        // remove_link checks
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_remove_symlink()
            .with(eq(dest.clone()))
            .returning(|_| Ok(()));

        let manager = LinkManager::new(&runtime);
        let result = manager.prepare_link_destination(&dest, &package_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_current_link_create_new() {
        // Test creating a new 'current' symlink when it doesn't exist
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/root/o/r");
        let current_link = PathBuf::from("/root/o/r/current");

        // File doesn't exist
        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| false);

        // Create symlink: current -> v1
        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("v1")), eq(current_link.clone()))
            .returning(|_, _| Ok(()));

        let manager = LinkManager::new(&runtime);
        manager.update_current_link(&package_dir, "v1").unwrap();
    }

    #[test]
    fn test_update_current_link_update_existing() {
        // Test updating 'current' symlink from v1 to v2
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/root/o/r");
        let current_link = PathBuf::from("/root/o/r/current");

        // File exists
        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Is symlink
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Read link -> points to v1
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1")));

        // Remove old symlink
        runtime
            .expect_remove_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(()));

        // Create new symlink: current -> v2
        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("v2")), eq(current_link.clone()))
            .returning(|_, _| Ok(()));

        let manager = LinkManager::new(&runtime);
        manager.update_current_link(&package_dir, "v2").unwrap();
    }

    #[test]
    fn test_update_current_link_idempotent() {
        // Test that update is idempotent - no changes when already correct
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/root/o/r");
        let current_link = PathBuf::from("/root/o/r/current");

        // File exists
        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Is symlink
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Read link -> already points to v1
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1")));

        // No remove_symlink or symlink calls expected

        let manager = LinkManager::new(&runtime);
        manager.update_current_link(&package_dir, "v1").unwrap();
    }

    #[test]
    fn test_update_current_link_fails_if_not_symlink() {
        // Test that update fails if 'current' exists but is not a symlink
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/root/o/r");
        let current_link = PathBuf::from("/root/o/r/current");

        // File exists
        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Not a symlink (regular file)
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| false);

        let manager = LinkManager::new(&runtime);
        let result = manager.update_current_link(&package_dir, "v1");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a symlink"));
    }

    #[test]
    fn test_update_current_link_recreates_unreadable() {
        // Test that symlink is recreated when read_link fails
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/root/o/r");
        let current_link = PathBuf::from("/root/o/r/current");

        // File exists
        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Is symlink
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Read link fails (corrupted)
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Err(anyhow::anyhow!("corrupted symlink")));

        // Remove corrupted symlink
        runtime
            .expect_remove_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(()));

        // Create new symlink
        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("v1")), eq(current_link.clone()))
            .returning(|_, _| Ok(()));

        let manager = LinkManager::new(&runtime);
        manager.update_current_link(&package_dir, "v1").unwrap();
    }
}
