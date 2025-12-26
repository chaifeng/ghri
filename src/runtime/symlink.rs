//! Symlink operations (create, read, resolve, remove).

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

use super::RealRuntime;
use super::path::{is_path_under, normalize_path};

impl RealRuntime {
    #[tracing::instrument(skip(self))]
    pub(crate) fn symlink_impl(&self, original: &Path, link: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink as unix_symlink;
            unix_symlink(original, link).context("Failed to create symlink")?;
        }
        #[cfg(windows)]
        {
            use anyhow::bail;
            use std::os::windows::fs::{symlink_dir, symlink_file};
            use tracing::trace;

            debug!("Creating symlink from {:?} to {:?}", link, original);

            // `is_dir()` on a relative path is relative to CWD; we want it relative to the link's parent.
            let target_path = if original.is_absolute() {
                original.to_path_buf()
            } else {
                link.parent()
                    .context("Failed to get parent directory for symlink")?
                    .join(original)
            };

            if target_path.is_dir() {
                trace!(
                    "Target path {} is a directory, creating directory symlink",
                    target_path.display()
                );
                symlink_dir(original, link).context("Failed to create directory symlink")?;
            } else {
                trace!(
                    "Target path {} is a file, creating file",
                    target_path.display()
                );
                symlink_file(original, link).context("Failed to create file symlink")?;
            }

            if fs::symlink_metadata(link).is_err() {
                bail!(
                    "Symlink creation reported success but link does not exist: link={:?} target={:?}",
                    link,
                    original
                );
            }
        }
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn read_link_impl(&self, path: &Path) -> Result<PathBuf> {
        fs::read_link(path).context("Failed to read symlink")
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn resolve_link_impl(&self, path: &Path) -> Result<PathBuf> {
        let target = fs::read_link(path).context("Failed to read symlink")?;
        if target.is_absolute() {
            Ok(target)
        } else {
            // Resolve relative path against the link's parent directory
            let parent = path
                .parent()
                .context("Failed to get parent directory of symlink")?;
            // Use lexical path joining and normalize the result
            let resolved = parent.join(&target);
            // Normalize the path by processing . and .. components
            Ok(normalize_path(&resolved))
        }
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn canonicalize_impl(&self, path: &Path) -> Result<PathBuf> {
        fs::canonicalize(path).context("Failed to canonicalize path")
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn is_symlink_impl(&self, path: &Path) -> bool {
        fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn remove_symlink_impl(&self, path: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            fs::remove_file(path).context("Failed to remove symlink")?;
        }
        #[cfg(windows)]
        {
            // On Windows, removing a symlink requires remove_dir for a directory symlink
            // and remove_file for a file symlink. We try to remove it as a directory
            // first, and if that fails, we try to remove it as a file.
            fs::remove_dir(path)
                .or_else(|_| fs::remove_file(path))
                .context("Failed to remove symlink")?;
        }
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn remove_symlink_if_target_under_impl(
        &self,
        link_path: &Path,
        target_prefix: &Path,
        description: &str,
    ) -> Result<bool> {
        debug!(
            "Validating link {:?} (expected prefix: {:?})",
            link_path, target_prefix
        );

        if !self.is_symlink_impl(link_path) {
            if self.exists_impl(link_path) {
                eprintln!(
                    "Warning: {} {:?} exists but is not a symlink, skipping",
                    description, link_path
                );
            } else {
                debug!("{} {:?} does not exist, skipping", description, link_path);
            }
            return Ok(false);
        }

        match self.read_link_impl(link_path) {
            Ok(target) => {
                // Resolve relative paths to absolute and canonicalize
                let resolved_target = if target.is_relative() {
                    link_path.parent().unwrap_or(Path::new(".")).join(&target)
                } else {
                    target.clone()
                };

                // Canonicalize if target exists to resolve .. and symlinks
                let canonicalized_target =
                    fs::canonicalize(&resolved_target).unwrap_or_else(|_| resolved_target.clone());

                debug!(
                    "Link {:?} points to {:?} (resolved: {:?}, canonicalized: {:?})",
                    link_path, target, resolved_target, canonicalized_target
                );

                // Canonicalize prefix as well for accurate comparison
                let canonicalized_prefix =
                    fs::canonicalize(target_prefix).unwrap_or_else(|_| target_prefix.to_path_buf());

                // Check if the target is under the prefix using safe path comparison
                if !is_path_under(&canonicalized_target, &canonicalized_prefix) {
                    eprintln!(
                        "Warning: {} {:?} points to {:?} which is not within {:?}, skipping removal",
                        description, link_path, canonicalized_target, canonicalized_prefix
                    );
                    return Ok(false);
                }

                // Link is valid, remove it
                debug!("Removing {} {:?}", description, link_path);
                match self.remove_symlink_impl(link_path) {
                    Ok(()) => {
                        println!("Removed {} {:?}", description, link_path);
                        Ok(true)
                    }
                    Err(e) => {
                        warn!("Failed to remove {} {:?}: {}", description, link_path, e);
                        eprintln!(
                            "Warning: Failed to remove {} {:?}: {}",
                            description, link_path, e
                        );
                        Ok(false)
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "Warning: {} {:?} is a symlink but cannot read its target: {}, skipping",
                    description, link_path, e
                );
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::{RealRuntime, Runtime};
    use tempfile::tempdir;

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_real_runtime_symlink_ops() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        // Create a target directory
        let target = dir.path().join("target");
        runtime.create_dir_all(&target).unwrap();

        // Test symlink and is_symlink
        let link = dir.path().join("link");
        runtime.symlink(&target, &link).unwrap();
        assert!(runtime.is_symlink(&link));
        assert!(!runtime.is_symlink(&target));

        // Test read_link
        let read_target = runtime.read_link(&link).unwrap();
        let resolved_read_target = if read_target.is_absolute() {
            read_target.clone()
        } else {
            link.parent().unwrap_or(dir.path()).join(&read_target)
        };
        assert_eq!(resolved_read_target, target);

        // Test canonicalize
        let canonical = runtime.canonicalize(&link).unwrap();
        assert!(canonical.ends_with("target"));

        // Test remove_symlink
        runtime.remove_symlink(&link).unwrap();
        assert!(!runtime.exists(&link));
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_real_runtime_file_symlink() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        use std::fs;

        // Create a target file
        let target = dir.path().join("target.txt");
        runtime.write(&target, b"content").unwrap();

        // Test symlink to file
        let link = dir.path().join("link.txt");
        runtime.symlink(&target, &link).unwrap();
        assert!(
            runtime.is_symlink(&link),
            "Expected symlink: link={:?} target={:?} exists={} metadata={:?} read_link={:?}",
            link,
            target,
            runtime.exists(&link),
            fs::symlink_metadata(&link),
            runtime.read_link(&link),
        );

        // Read through symlink
        let content = runtime.read_to_string(&link).unwrap_or_else(|e| {
            panic!(
                "Failed to read via symlink: link={:?} target={:?} exists={} metadata={:?} read_link={:?} err={}",
                link,
                target,
                runtime.exists(&link),
                fs::symlink_metadata(&link),
                runtime.read_link(&link),
                e
            )
        });
        assert_eq!(
            content,
            "content",
            "Unexpected content via symlink: link={:?} target={:?} exists={} metadata={:?} read_link={:?}",
            link,
            target,
            runtime.exists(&link),
            fs::symlink_metadata(&link),
            runtime.read_link(&link),
        );

        // Clean up
        runtime.remove_symlink(&link).unwrap();
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_resolve_link_absolute_target() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        // Create a target file
        let target = dir.path().join("target.txt");
        runtime.write(&target, b"content").unwrap();

        // Create symlink with absolute target
        let link = dir.path().join("link.txt");
        runtime.symlink(&target, &link).unwrap();

        // resolve_link should return the absolute path
        let resolved = runtime.resolve_link(&link).unwrap();
        assert!(resolved.is_absolute());
        assert!(resolved.ends_with("target.txt"));
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_resolve_link_relative_target_same_dir() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        // Create a target file
        let target = dir.path().join("target.txt");
        runtime.write(&target, b"content").unwrap();

        // Create symlink with relative target (same directory)
        let link = dir.path().join("link.txt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(std::path::Path::new("target.txt"), &link).unwrap();
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_file;
            symlink_file(std::path::Path::new("target.txt"), &link).unwrap();
        }

        // resolve_link should resolve relative to link's parent
        let resolved = runtime.resolve_link(&link).unwrap();
        assert!(resolved.ends_with("target.txt"));
        // The resolved path should be the same directory as the original target
        assert_eq!(resolved.parent(), target.parent());
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_resolve_link_relative_target_parent_dir() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        // Create structure: dir/target.txt, dir/sub/link.txt -> ../target.txt
        let target = dir.path().join("target.txt");
        runtime.write(&target, b"content").unwrap();

        let sub_dir = dir.path().join("sub");
        runtime.create_dir_all(&sub_dir).unwrap();

        let link = sub_dir.join("link.txt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(std::path::Path::new("../target.txt"), &link).unwrap();
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_file;
            symlink_file(std::path::Path::new("..\\target.txt"), &link).unwrap();
        }

        // resolve_link should resolve ../target.txt relative to sub/
        let resolved = runtime.resolve_link(&link).unwrap();
        // After normalization, should be dir/target.txt
        assert!(resolved.ends_with("target.txt"));
        // Compare canonicalized paths to handle macOS /var -> /private/var symlinks
        let resolved_canonical = std::fs::canonicalize(&resolved).unwrap_or(resolved);
        let target_canonical = std::fs::canonicalize(&target).unwrap();
        assert_eq!(resolved_canonical, target_canonical);
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_resolve_link_multiple_parent_dirs() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        // Create structure: dir/target.txt, dir/a/b/link.txt -> ../../target.txt
        let target = dir.path().join("target.txt");
        runtime.write(&target, b"content").unwrap();

        let sub_dir = dir.path().join("a").join("b");
        runtime.create_dir_all(&sub_dir).unwrap();

        let link = sub_dir.join("link.txt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(std::path::Path::new("../../target.txt"), &link).unwrap();
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_file;
            symlink_file(std::path::Path::new("..\\..\\target.txt"), &link).unwrap();
        }

        // resolve_link should resolve ../../target.txt relative to a/b/
        let resolved = runtime.resolve_link(&link).unwrap();
        assert!(resolved.ends_with("target.txt"));
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_resolve_link_with_dot_components() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        // Create target
        let target = dir.path().join("target.txt");
        runtime.write(&target, b"content").unwrap();

        // Create symlink with ./ in path
        let link = dir.path().join("link.txt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(std::path::Path::new("./target.txt"), &link).unwrap();
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_file;
            symlink_file(std::path::Path::new(".\\target.txt"), &link).unwrap();
        }

        let resolved = runtime.resolve_link(&link).unwrap();
        assert!(resolved.ends_with("target.txt"));
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_remove_symlink_if_target_under_success() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        let prefix = dir.path().join("ghri");
        runtime.create_dir_all(&prefix).unwrap();

        let target = prefix.join("target.txt");
        runtime.write(&target, b"content").unwrap();

        let link = dir.path().join("link.txt");
        runtime.symlink(&target, &link).unwrap();

        let removed = runtime
            .remove_symlink_if_target_under(&link, &prefix, "test link")
            .unwrap();
        assert!(removed);
        assert!(!runtime.exists(&link));
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_remove_symlink_if_target_under_exact_match() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        let prefix = dir.path().join("ghri");
        runtime.create_dir_all(&prefix).unwrap();

        // Link points exactly to the prefix directory
        let link = dir.path().join("link");
        runtime.symlink(&prefix, &link).unwrap();

        let removed = runtime
            .remove_symlink_if_target_under(&link, &prefix, "test link")
            .unwrap();
        assert!(removed);
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_remove_symlink_if_target_under_parent_prefix() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        let prefix = dir.path().join("ghri").join("owner").join("repo");
        runtime.create_dir_all(&prefix).unwrap();

        let target = prefix.join("v1").join("bin").join("tool");
        runtime.create_dir_all(target.parent().unwrap()).unwrap();
        runtime.write(&target, b"content").unwrap();

        let link = dir.path().join("tool");
        runtime.symlink(&target, &link).unwrap();

        // Should be removed because target is under ghri/owner/repo
        let removed = runtime
            .remove_symlink_if_target_under(&link, &prefix, "test link")
            .unwrap();
        assert!(removed);
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_remove_symlink_if_target_under_wrong_prefix() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        let prefix = dir.path().join("ghri");
        runtime.create_dir_all(&prefix).unwrap();

        let other_dir = dir.path().join("other");
        runtime.create_dir_all(&other_dir).unwrap();

        let target = other_dir.join("target.txt");
        runtime.write(&target, b"content").unwrap();

        let link = dir.path().join("link.txt");
        runtime.symlink(&target, &link).unwrap();

        // Should NOT be removed because target is not under prefix
        let removed = runtime
            .remove_symlink_if_target_under(&link, &prefix, "test link")
            .unwrap();
        assert!(!removed);
        assert!(runtime.exists(&link));

        // Clean up
        runtime.remove_symlink(&link).unwrap();
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_remove_symlink_if_target_under_partial_component() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        let prefix = dir.path().join("ghri");
        runtime.create_dir_all(&prefix).unwrap();

        // Create "ghri-extra" which is NOT under "ghri"
        let other_dir = dir.path().join("ghri-extra");
        runtime.create_dir_all(&other_dir).unwrap();

        let target = other_dir.join("target.txt");
        runtime.write(&target, b"content").unwrap();

        let link = dir.path().join("link.txt");
        runtime.symlink(&target, &link).unwrap();

        // Should NOT be removed - "ghri-extra" is not under "ghri"
        let removed = runtime
            .remove_symlink_if_target_under(&link, &prefix, "test link")
            .unwrap();
        assert!(!removed);

        // Clean up
        runtime.remove_symlink(&link).unwrap();
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_remove_symlink_if_target_under_different_file() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();

        let prefix = dir.path().join("ghri");
        runtime.create_dir_all(&prefix).unwrap();

        let target = prefix.join("file.txt");
        runtime.write(&target, b"content").unwrap();

        // Not a symlink, just a regular file
        let regular_file = dir.path().join("regular.txt");
        runtime.write(&regular_file, b"content").unwrap();

        // Should return false for non-symlink
        let removed = runtime
            .remove_symlink_if_target_under(&regular_file, &prefix, "test link")
            .unwrap();
        assert!(!removed);
        assert!(runtime.exists(&regular_file));
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_remove_symlink_if_target_under_not_symlink() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();

        let prefix = dir.path().join("ghri");
        runtime.create_dir_all(&prefix).unwrap();

        let regular_file = dir.path().join("regular.txt");
        runtime.write(&regular_file, b"content").unwrap();

        let removed = runtime
            .remove_symlink_if_target_under(&regular_file, &prefix, "test")
            .unwrap();
        assert!(!removed);
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_remove_symlink_if_target_under_nonexistent() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();

        let prefix = dir.path().join("ghri");
        let nonexistent = dir.path().join("nonexistent");

        let removed = runtime
            .remove_symlink_if_target_under(&nonexistent, &prefix, "test")
            .unwrap();
        assert!(!removed);
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_remove_symlink_if_target_under_relative_link() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        // Create prefix/target.txt
        let prefix = dir.path().join("ghri");
        runtime.create_dir_all(&prefix).unwrap();
        let target = prefix.join("target.txt");
        runtime.write(&target, b"content").unwrap();

        // Create relative symlink: link.txt -> ghri/target.txt
        let link = dir.path().join("link.txt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(std::path::Path::new("ghri/target.txt"), &link).unwrap();
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_file;
            symlink_file(std::path::Path::new("ghri\\target.txt"), &link).unwrap();
        }

        let removed = runtime
            .remove_symlink_if_target_under(&link, &prefix, "test link")
            .unwrap();
        assert!(removed);
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_remove_symlink_if_target_under_relative_link_wrong_prefix() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        // Create other/target.txt (NOT under ghri)
        let prefix = dir.path().join("ghri");
        runtime.create_dir_all(&prefix).unwrap();

        let other = dir.path().join("other");
        runtime.create_dir_all(&other).unwrap();
        let target = other.join("target.txt");
        runtime.write(&target, b"content").unwrap();

        // Create relative symlink: link.txt -> other/target.txt
        let link = dir.path().join("link.txt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(std::path::Path::new("other/target.txt"), &link).unwrap();
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_file;
            symlink_file(std::path::Path::new("other\\target.txt"), &link).unwrap();
        }

        // Should NOT be removed
        let removed = runtime
            .remove_symlink_if_target_under(&link, &prefix, "test link")
            .unwrap();
        assert!(!removed);

        // Clean up
        runtime.remove_symlink(&link).unwrap();
    }

    #[cfg_attr(
        ghri_skip_cross_windows_tests,
        ignore = "cross windows tests disabled; set GHRI_RUN_CROSS_WINDOWS_TESTS=1 to enable"
    )]
    #[test]
    fn test_remove_symlink_if_target_under_nested_prefix() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        let prefix = dir.path().join("ghri").join("owner").join("repo");
        runtime.create_dir_all(&prefix).unwrap();

        let target = prefix.join("v1").join("bin").join("tool");
        runtime.create_dir_all(target.parent().unwrap()).unwrap();
        runtime.write(&target, b"").unwrap();

        let link = dir.path().join("tool");
        runtime.symlink(&target, &link).unwrap();

        let removed = runtime
            .remove_symlink_if_target_under(&link, &prefix, "test")
            .unwrap();
        assert!(removed);
    }
}
