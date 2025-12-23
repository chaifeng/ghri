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
                if is_path_under(&resolved_target, expected_prefix) {
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
    pub fn create_link(&self, target: &Path, dest: &Path) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = dest.parent()
            && !self.runtime.exists(parent)
        {
            self.runtime.create_dir_all(parent)?;
        }

        // Calculate relative path if possible
        if let Some(link_target) = relative_symlink_path(target, dest) {
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
    use mockall::predicate::eq;

    #[test]
    fn test_check_link_valid() {
        let mut runtime = MockRuntime::new();
        let dest = PathBuf::from("/usr/local/bin/tool");
        let prefix = PathBuf::from("/home/user/.ghri/owner/repo");
        let target = PathBuf::from("/home/user/.ghri/owner/repo/v1/tool");

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
        let dest = PathBuf::from("/usr/local/bin/tool");
        let prefix = PathBuf::from("/home/user/.ghri/owner/repo");

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
        let dest = PathBuf::from("/usr/local/bin/tool");
        let prefix = PathBuf::from("/home/user/.ghri/owner/repo");
        let target = PathBuf::from("/some/other/path"); // Not under prefix

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
        let dest = PathBuf::from("/usr/local/bin/tool");
        let prefix = PathBuf::from("/home/user/.ghri/owner/repo");

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
        let prefix = PathBuf::from("/home/user/.ghri/owner/repo");

        let valid_dest = PathBuf::from("/bin/valid");
        let invalid_dest = PathBuf::from("/bin/invalid");
        let valid_target = PathBuf::from("/home/user/.ghri/owner/repo/v1/tool");

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
        let version_dir = PathBuf::from("/home/user/.ghri/owner/repo/v1");
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");
        let dest = PathBuf::from("/usr/local/bin/tool");
        let target = version_dir.join("bin/tool");

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
        let version_dir = PathBuf::from("/home/user/.ghri/owner/repo/v1");
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");
        let dest = PathBuf::from("/usr/local/bin/tool");
        let target = version_dir.join("bin/notfound");

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
        let version_dir = PathBuf::from("/home/user/.ghri/owner/repo/v1");
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");
        let dest = PathBuf::from("/usr/local/bin/tool");
        let target = version_dir.join("bin/tool");

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
}
