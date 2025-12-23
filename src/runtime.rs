use anyhow::{Context, Result};
use async_trait::async_trait;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use tracing::{debug, warn};

/// Normalize a path by processing `.` and `..` components lexically.
/// This does not access the filesystem and does not follow symlinks.
fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {
                // Skip `.` components
            }
            Component::ParentDir => {
                // Pop the last component if possible
                if !result.pop() {
                    // If we can't pop (e.g., at root), keep the `..`
                    result.push(component);
                }
            }
            _ => {
                result.push(component);
            }
        }
    }
    result
}

/// Check if a path is under a given directory by comparing normalized path components.
/// This function normalizes both paths to handle `..` components safely.
/// Returns true if `path` is under `dir` (i.e., `dir` is a prefix of `path`).
///
/// # Security
/// This function normalizes paths to prevent directory traversal attacks.
/// For example, `/usr/local/bin/../../../etc/passwd` is NOT under `/usr/local`.
pub fn is_path_under(path: &Path, dir: &Path) -> bool {
    let normalized_path = normalize_path(path);
    let normalized_dir = normalize_path(dir);

    let path_components: Vec<_> = normalized_path.components().collect();
    let dir_components: Vec<_> = normalized_dir.components().collect();

    // Path must have at least as many components as dir
    if path_components.len() < dir_components.len() {
        return false;
    }

    // All dir components must match the beginning of path components
    dir_components
        .iter()
        .zip(path_components.iter())
        .all(|(d, p)| d == p)
}

/// Calculate the relative path from a symlink location to a target.
/// This is used to create shorter symlinks using relative paths when possible.
///
/// For example, if creating a symlink at `/usr/local/bin/tool` pointing to
/// `/opt/ghri/owner/repo/v1/tool`, this returns `../../opt/ghri/owner/repo/v1/tool`.
///
/// Returns `None` if a relative path cannot be computed (e.g., different drive letters on Windows).
pub fn relative_symlink_path(from_link: &Path, to_target: &Path) -> Option<PathBuf> {
    // Get the directory containing the symlink
    let from_dir = from_link.parent()?;
    let result = pathdiff::diff_paths(to_target, from_dir)?;

    // If the result is an absolute path, it means we couldn't compute a relative path
    // (e.g., different drives on Windows). Return None in this case.
    if result.is_absolute() {
        return None;
    }

    Some(result)
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait Runtime: Send + Sync {
    // Environment
    fn env_var(&self, key: &str) -> Result<String, env::VarError>;

    // File System
    fn write(&self, path: &Path, contents: &[u8]) -> Result<()>;
    fn read_to_string(&self, path: &Path) -> Result<String>;
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;
    fn copy(&self, from: &Path, to: &Path) -> Result<u64>;
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    fn remove_file(&self, path: &Path) -> Result<()>;
    fn remove_dir(&self, path: &Path) -> Result<()>;
    fn remove_symlink(&self, path: &Path) -> Result<()>;
    fn exists(&self, path: &Path) -> bool;
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>>;
    fn symlink(&self, original: &Path, link: &Path) -> Result<()>;
    fn read_link(&self, path: &Path) -> Result<PathBuf>;

    /// Resolve a symlink to an absolute path (without recursively resolving symlinks).
    /// If the link target is relative, it is resolved relative to the link's parent directory.
    /// Unlike canonicalize, this does not follow nested symlinks.
    fn resolve_link(&self, path: &Path) -> Result<PathBuf>;

    fn is_symlink(&self, path: &Path) -> bool;
    fn create_file(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>>;
    fn open(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>>;
    fn remove_dir_all(&self, path: &Path) -> Result<()>;
    fn is_dir(&self, path: &Path) -> bool;

    /// Set file permissions (mode) on Unix systems. No-op on Windows.
    fn set_permissions(&self, path: &Path, mode: u32) -> Result<()>;

    /// Remove a symlink if its target is under the given prefix directory.
    /// The prefix is checked by directory components, not string prefix.
    /// Returns Ok(true) if removed, Ok(false) if skipped, Err if operation failed.
    fn remove_symlink_if_target_under(
        &self,
        link_path: &Path,
        target_prefix: &Path,
        description: &str,
    ) -> Result<bool>;

    // Directories
    fn home_dir(&self) -> Option<PathBuf>;
    fn config_dir(&self) -> Option<PathBuf>;

    // Privilege
    fn is_privileged(&self) -> bool;

    // User interaction
    /// Prompt user for confirmation. Returns true if user confirms (y/yes), false otherwise.
    fn confirm(&self, prompt: &str) -> Result<bool>;
}

pub struct RealRuntime;

#[async_trait]
impl Runtime for RealRuntime {
    #[tracing::instrument(skip(self))]
    fn env_var(&self, key: &str) -> Result<String, env::VarError> {
        env::var(key)
    }

    #[tracing::instrument(skip(self, contents))]
    fn write(&self, path: &Path, contents: &[u8]) -> Result<()> {
        fs::write(path, contents).context("Failed to write to file")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn read_to_string(&self, path: &Path) -> Result<String> {
        fs::read_to_string(path).context("Failed to read file to string")
    }

    #[tracing::instrument(skip(self))]
    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        fs::rename(from, to).context("Failed to rename file")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn copy(&self, from: &Path, to: &Path) -> Result<u64> {
        fs::copy(from, to).context("Failed to copy file")
    }

    #[tracing::instrument(skip(self))]
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).context("Failed to create directory")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn remove_file(&self, path: &Path) -> Result<()> {
        fs::remove_file(path).context("Failed to remove file")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn remove_dir(&self, path: &Path) -> Result<()> {
        fs::remove_dir(path).context("Failed to remove directory")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn remove_symlink(&self, path: &Path) -> Result<()> {
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
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    #[tracing::instrument(skip(self))]
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        fs::read_dir(path)?.map(|entry| Ok(entry?.path())).collect()
    }

    #[tracing::instrument(skip(self))]
    fn symlink(&self, original: &Path, link: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink as unix_symlink;
            unix_symlink(original, link).context("Failed to create symlink")?;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::{symlink_dir, symlink_file};

            // If `original` is a relative path, `is_dir()` would check it against the
            // current working directory. We need to check it relative to the directory
            // where the symlink will be created.
            let target_path = if original.is_absolute() {
                original.to_path_buf()
            } else {
                link.parent()
                    .context("Failed to get parent directory for symlink")?
                    .join(original)
            };

            if target_path.is_dir() {
                symlink_dir(original, link).context("Failed to create directory symlink")?;
            } else {
                symlink_file(original, link).context("Failed to create file symlink")?;
            }
        }
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn read_link(&self, path: &Path) -> Result<PathBuf> {
        fs::read_link(path).context("Failed to read symlink")
    }

    #[tracing::instrument(skip(self))]
    fn resolve_link(&self, path: &Path) -> Result<PathBuf> {
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
    fn is_symlink(&self, path: &Path) -> bool {
        fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }

    #[tracing::instrument(skip(self))]
    fn create_file(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>> {
        let file = std::fs::File::create(path).context("Failed to create file")?;
        Ok(Box::new(file))
    }

    #[tracing::instrument(skip(self))]
    fn open(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>> {
        let file = std::fs::File::open(path).context("Failed to open file")?;
        Ok(Box::new(file))
    }

    #[tracing::instrument(skip(self))]
    fn remove_dir_all(&self, path: &Path) -> Result<()> {
        fs::remove_dir_all(path).context("Failed to remove directory and its contents")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    #[tracing::instrument(skip(self))]
    fn set_permissions(&self, path: &Path, mode: u32) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::Permissions::from_mode(mode);
            fs::set_permissions(path, permissions).context("Failed to set permissions")?;
        }
        #[cfg(not(unix))]
        {
            let _ = (path, mode); // Suppress unused warnings on non-Unix
        }
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn remove_symlink_if_target_under(
        &self,
        link_path: &Path,
        target_prefix: &Path,
        description: &str,
    ) -> Result<bool> {
        debug!(
            "Validating link {:?} (expected prefix: {:?})",
            link_path, target_prefix
        );

        if !self.is_symlink(link_path) {
            if self.exists(link_path) {
                eprintln!(
                    "Warning: {} {:?} exists but is not a symlink, skipping",
                    description, link_path
                );
            } else {
                debug!("{} {:?} does not exist, skipping", description, link_path);
            }
            return Ok(false);
        }

        match self.read_link(link_path) {
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
                match self.remove_symlink(link_path) {
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

    #[tracing::instrument(skip(self))]
    fn home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }

    #[tracing::instrument(skip(self))]
    fn config_dir(&self) -> Option<PathBuf> {
        dirs::config_dir()
    }

    #[tracing::instrument(skip(self))]
    fn is_privileged(&self) -> bool {
        #[cfg(unix)]
        return nix::unistd::geteuid().as_raw() == 0;

        #[cfg(windows)]
        return is_elevated::is_elevated();
    }

    fn confirm(&self, prompt: &str) -> Result<bool> {
        use std::io::{self, Write};
        print!("{} [y/N] ", prompt);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let response = input.trim().to_lowercase();
        Ok(response == "y" || response == "yes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use tempfile::tempdir;

    #[test]
    fn test_real_runtime_file_ops() {
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        // Write
        rt.write(&file_path, b"hello").unwrap();
        assert!(rt.exists(&file_path));

        // Read
        let content = rt.read_to_string(&file_path).unwrap();
        assert_eq!(content, "hello");

        // Open
        let mut reader = rt.open(&file_path).unwrap();
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "hello");

        // Rename
        let new_path = dir.path().join("test_new.txt");
        rt.rename(&file_path, &new_path).unwrap();
        assert!(!rt.exists(&file_path));
        assert!(rt.exists(&new_path));

        // Create file using write stream
        let file_path2 = dir.path().join("test2.txt");
        {
            let mut writer = rt.create_file(&file_path2).unwrap();
            writer.write_all(b"world").unwrap();
        }
        assert_eq!(rt.read_to_string(&file_path2).unwrap(), "world");

        // Remove
        rt.remove_file(&new_path).unwrap();
        assert!(!rt.exists(&new_path));
    }

    #[test]
    fn test_real_runtime_dir_ops() {
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let sub_dir = dir.path().join("a/b/c");

        // Create
        rt.create_dir_all(&sub_dir).unwrap();
        assert!(rt.exists(&sub_dir));
        assert!(rt.is_dir(&sub_dir));

        // Read dir
        let parent = sub_dir.parent().unwrap();
        let entries = rt.read_dir(parent).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], sub_dir);

        // Remove dir
        rt.remove_dir(&sub_dir).unwrap();
        assert!(!rt.exists(&sub_dir));

        // Remove dir all
        let sub_dir_full = dir.path().join("x/y/z");
        rt.create_dir_all(&sub_dir_full).unwrap();
        rt.write(&sub_dir_full.join("file.txt"), b"data").unwrap();
        rt.remove_dir_all(&dir.path().join("x")).unwrap();
        assert!(!rt.exists(&dir.path().join("x")));
    }

    #[test]
    fn test_real_runtime_symlink_ops() {
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");

        // Create target (must be dir for Windows compatibility in some cases)
        rt.create_dir_all(&target).unwrap();

        // Symlink
        rt.symlink(&target, &link).unwrap();
        assert!(rt.exists(&link));
        assert!(rt.is_symlink(&link));

        // Read link
        let read_target = rt.read_link(&link).unwrap();
        // Note: read_link might return relative or absolute depends on how it was created
        // In our case we passed absolute path
        assert_eq!(read_target, target);

        // Remove symlink
        rt.remove_symlink(&link).unwrap();
        assert!(!rt.exists(&link));
        assert!(rt.exists(&target));
    }

    #[test]
    fn test_real_runtime_file_symlink() {
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let target_file = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");

        // Create target file
        rt.write(&target_file, b"hello").unwrap();

        // Symlink
        rt.symlink(&target_file, &link).unwrap();
        assert!(rt.exists(&link));
        assert!(rt.is_symlink(&link));

        // Read link
        let read_target = rt.read_link(&link).unwrap();
        assert_eq!(read_target, target_file);

        // Verify that we can read the file through the symlink
        let content = rt.read_to_string(&link).unwrap();
        assert_eq!(content, "hello");

        // Remove symlink
        rt.remove_symlink(&link).unwrap();
        assert!(!rt.exists(&link));
        assert!(rt.exists(&target_file));
    }

    #[test]
    fn test_resolve_link_absolute_target() {
        // --- Test: Resolve symlink with absolute target ---
        // Symlink /tmp/xxx/link -> /tmp/xxx/target.txt (absolute)
        // Expected: /tmp/xxx/target.txt
        let rt = RealRuntime;
        let dir = tempdir().unwrap();

        // --- Setup Paths ---
        let target_file = dir.path().join("target.txt");
        let link = dir.path().join("link");

        // --- Create Files ---
        // Create file: /tmp/xxx/target.txt
        rt.write(&target_file, b"hello").unwrap();

        // Create symlink: /tmp/xxx/link -> /tmp/xxx/target.txt (absolute path)
        rt.symlink(&target_file, &link).unwrap();

        // --- Verify ---
        // resolve_link should return the absolute path
        let resolved = rt.resolve_link(&link).unwrap();
        assert_eq!(resolved, target_file);
    }

    #[test]
    fn test_resolve_link_relative_target_same_dir() {
        // --- Test: Resolve symlink with relative target in same directory ---
        // Symlink /tmp/xxx/link -> target.txt (relative)
        // Expected: /tmp/xxx/target.txt
        let rt = RealRuntime;
        let dir = tempdir().unwrap();

        // --- Setup Paths ---
        let target_file = dir.path().join("target.txt");
        let link = dir.path().join("link");

        // --- Create Files ---
        // Create file: /tmp/xxx/target.txt
        rt.write(&target_file, b"hello").unwrap();

        // Create symlink: /tmp/xxx/link -> target.txt (relative path)
        rt.symlink(Path::new("target.txt"), &link).unwrap();

        // --- Verify ---
        // read_link returns raw target: "target.txt"
        let raw_target = rt.read_link(&link).unwrap();
        assert_eq!(raw_target, PathBuf::from("target.txt"));

        // resolve_link should return absolute path: /tmp/xxx/target.txt
        let resolved = rt.resolve_link(&link).unwrap();
        assert_eq!(resolved, target_file);
    }

    #[test]
    fn test_resolve_link_relative_target_parent_dir() {
        // --- Test: Resolve symlink with relative target using .. ---
        // Directory structure:
        //   /tmp/xxx/package/bin/tool
        //   /tmp/xxx/links/tool -> ../package/bin/tool
        // Expected: /tmp/xxx/package/bin/tool
        let rt = RealRuntime;
        let dir = tempdir().unwrap();

        // --- Setup Paths ---
        let package_dir = dir.path().join("package/bin");
        let target_file = package_dir.join("tool");
        let links_dir = dir.path().join("links");
        let link = links_dir.join("tool");

        // --- Create Files ---
        // Create directory: /tmp/xxx/package/bin
        rt.create_dir_all(&package_dir).unwrap();

        // Create file: /tmp/xxx/package/bin/tool
        rt.write(&target_file, b"binary").unwrap();

        // Create directory: /tmp/xxx/links
        rt.create_dir_all(&links_dir).unwrap();

        // Create symlink: /tmp/xxx/links/tool -> ../package/bin/tool
        rt.symlink(Path::new("../package/bin/tool"), &link).unwrap();

        // --- Verify ---
        // read_link returns raw target: "../package/bin/tool"
        let raw_target = rt.read_link(&link).unwrap();
        assert_eq!(raw_target, PathBuf::from("../package/bin/tool"));

        // resolve_link should return absolute path: /tmp/xxx/package/bin/tool
        let resolved = rt.resolve_link(&link).unwrap();
        assert_eq!(resolved, target_file);
    }

    #[test]
    fn test_resolve_link_multiple_parent_dirs() {
        // --- Test: Resolve symlink with multiple .. components ---
        // Directory structure:
        //   /tmp/xxx/a/b/c/target.txt
        //   /tmp/xxx/x/y/z/link -> ../../../a/b/c/target.txt
        // Expected: /tmp/xxx/a/b/c/target.txt
        let rt = RealRuntime;
        let dir = tempdir().unwrap();

        // --- Setup Paths ---
        let target_dir = dir.path().join("a/b/c");
        let target_file = target_dir.join("target.txt");
        let link_dir = dir.path().join("x/y/z");
        let link = link_dir.join("link");

        // --- Create Files ---
        // Create directory: /tmp/xxx/a/b/c
        rt.create_dir_all(&target_dir).unwrap();

        // Create file: /tmp/xxx/a/b/c/target.txt
        rt.write(&target_file, b"content").unwrap();

        // Create directory: /tmp/xxx/x/y/z
        rt.create_dir_all(&link_dir).unwrap();

        // Create symlink: /tmp/xxx/x/y/z/link -> ../../../a/b/c/target.txt
        rt.symlink(Path::new("../../../a/b/c/target.txt"), &link)
            .unwrap();

        // --- Verify ---
        // resolve_link should return absolute path: /tmp/xxx/a/b/c/target.txt
        let resolved = rt.resolve_link(&link).unwrap();
        assert_eq!(resolved, target_file);
    }

    #[test]
    fn test_resolve_link_with_dot_components() {
        // --- Test: Resolve symlink with . components ---
        // Symlink /tmp/xxx/link -> ./subdir/./file.txt
        // Expected: /tmp/xxx/subdir/file.txt
        let rt = RealRuntime;
        let dir = tempdir().unwrap();

        // --- Setup Paths ---
        let subdir = dir.path().join("subdir");
        let target_file = subdir.join("file.txt");
        let link = dir.path().join("link");

        // --- Create Files ---
        // Create directory: /tmp/xxx/subdir
        rt.create_dir_all(&subdir).unwrap();

        // Create file: /tmp/xxx/subdir/file.txt
        rt.write(&target_file, b"content").unwrap();

        // Create symlink: /tmp/xxx/link -> ./subdir/./file.txt
        rt.symlink(Path::new("./subdir/./file.txt"), &link).unwrap();

        // --- Verify ---
        // resolve_link should normalize and return: /tmp/xxx/subdir/file.txt
        let resolved = rt.resolve_link(&link).unwrap();
        assert_eq!(resolved, target_file);
    }

    #[test]
    fn test_real_runtime_env_and_dirs() {
        let rt = RealRuntime;
        // Test env_var with a likely existing var
        if let Ok(path) = std::env::var("PATH") {
            assert_eq!(rt.env_var("PATH").unwrap(), path);
        }

        assert!(rt.home_dir().is_some());
    }

    #[test]
    fn test_real_runtime_errors() {
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let non_existent = dir.path().join("non_existent");

        assert!(rt.read_to_string(&non_existent).is_err());
        assert!(rt.open(&non_existent).is_err());
        assert!(rt.rename(&non_existent, &dir.path().join("new")).is_err());
        assert!(rt.remove_file(&non_existent).is_err());
        assert!(rt.remove_dir(&non_existent).is_err());
    }

    #[test]
    fn test_remove_symlink_if_target_under_success() {
        // Symlink target is under the prefix - should be removed
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let prefix = dir.path().join("foo/bar");
        let target = prefix.join("file.txt");
        let link = dir.path().join("link");

        rt.create_dir_all(&prefix).unwrap();
        rt.write(&target, b"content").unwrap();
        rt.symlink(&target, &link).unwrap();

        let result = rt.remove_symlink_if_target_under(&link, &prefix, "test link");
        assert!(result.is_ok());
        assert!(result.unwrap()); // true = removed
        assert!(!rt.exists(&link));
        assert!(rt.exists(&target)); // target should still exist
    }

    #[test]
    fn test_remove_symlink_if_target_under_exact_match() {
        // Symlink target is exactly the prefix - should be removed
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let prefix = dir.path().join("foo/bar/file.txt");
        let link = dir.path().join("link");

        rt.create_dir_all(prefix.parent().unwrap()).unwrap();
        rt.write(&prefix, b"content").unwrap();
        rt.symlink(&prefix, &link).unwrap();

        let result = rt.remove_symlink_if_target_under(&link, &prefix, "test link");
        assert!(result.is_ok());
        assert!(result.unwrap()); // true = removed
        assert!(!rt.exists(&link));
    }

    #[test]
    fn test_remove_symlink_if_target_under_parent_prefix() {
        // Target /foo/bar/file.txt, prefix /foo - should be removed
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let prefix = dir.path().join("foo");
        let target = prefix.join("bar/file.txt");
        let link = dir.path().join("link");

        rt.create_dir_all(target.parent().unwrap()).unwrap();
        rt.write(&target, b"content").unwrap();
        rt.symlink(&target, &link).unwrap();

        let result = rt.remove_symlink_if_target_under(&link, &prefix, "test link");
        assert!(result.is_ok());
        assert!(result.unwrap()); // true = removed
    }

    #[test]
    fn test_remove_symlink_if_target_under_wrong_prefix() {
        // Target /foo/bar/file.txt, prefix /bar - should NOT be removed
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let target = dir.path().join("foo/bar/file.txt");
        let wrong_prefix = dir.path().join("bar");
        let link = dir.path().join("link");

        rt.create_dir_all(target.parent().unwrap()).unwrap();
        rt.write(&target, b"content").unwrap();
        rt.symlink(&target, &link).unwrap();

        let result = rt.remove_symlink_if_target_under(&link, &wrong_prefix, "test link");
        assert!(result.is_ok());
        assert!(!result.unwrap()); // false = skipped
        assert!(rt.is_symlink(&link)); // link should still exist
    }

    #[test]
    fn test_remove_symlink_if_target_under_partial_component() {
        // Target /foo/bar/file.txt, prefix /foo/b (incomplete component) - should NOT be removed
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let target = dir.path().join("foo/bar/file.txt");
        let partial_prefix = dir.path().join("foo/b");
        let link = dir.path().join("link");

        rt.create_dir_all(target.parent().unwrap()).unwrap();
        rt.write(&target, b"content").unwrap();
        rt.symlink(&target, &link).unwrap();

        let result = rt.remove_symlink_if_target_under(&link, &partial_prefix, "test link");
        assert!(result.is_ok());
        assert!(!result.unwrap()); // false = skipped (partial component doesn't match)
        assert!(rt.is_symlink(&link)); // link should still exist
    }

    #[test]
    fn test_remove_symlink_if_target_under_different_file() {
        // Target /foo/bar/file.txt, prefix /foo/bar/aaa.txt - should NOT be removed
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let target = dir.path().join("foo/bar/file.txt");
        let wrong_prefix = dir.path().join("foo/bar/aaa.txt");
        let link = dir.path().join("link");

        rt.create_dir_all(target.parent().unwrap()).unwrap();
        rt.write(&target, b"content").unwrap();
        rt.symlink(&target, &link).unwrap();

        let result = rt.remove_symlink_if_target_under(&link, &wrong_prefix, "test link");
        assert!(result.is_ok());
        assert!(!result.unwrap()); // false = skipped
        assert!(rt.is_symlink(&link));
    }

    #[test]
    fn test_remove_symlink_if_target_under_not_symlink() {
        // Path exists but is not a symlink - should skip
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let regular_file = dir.path().join("regular.txt");
        let prefix = dir.path();

        rt.write(&regular_file, b"content").unwrap();

        let result = rt.remove_symlink_if_target_under(&regular_file, prefix, "test file");
        assert!(result.is_ok());
        assert!(!result.unwrap()); // false = skipped
        assert!(rt.exists(&regular_file)); // file should still exist
    }

    #[test]
    fn test_remove_symlink_if_target_under_nonexistent() {
        // Path does not exist - should skip
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let nonexistent = dir.path().join("nonexistent");
        let prefix = dir.path();

        let result = rt.remove_symlink_if_target_under(&nonexistent, prefix, "test link");
        assert!(result.is_ok());
        assert!(!result.unwrap()); // false = skipped
    }

    #[test]
    fn test_remove_symlink_if_target_under_relative_link() {
        // Symlink uses relative path - should resolve and check correctly
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let prefix = dir.path().join("package");
        let target = prefix.join("bin/tool");
        let links_dir = dir.path().join("links");
        let link = links_dir.join("tool");

        rt.create_dir_all(target.parent().unwrap()).unwrap();
        rt.write(&target, b"content").unwrap();
        rt.create_dir_all(&links_dir).unwrap();

        // Create symlink with relative path
        let relative_target = PathBuf::from("../package/bin/tool");
        rt.symlink(&relative_target, &link).unwrap();

        let result = rt.remove_symlink_if_target_under(&link, &prefix, "test link");
        assert!(result.is_ok());
        assert!(result.unwrap()); // true = removed
        assert!(!rt.exists(&link));
    }

    #[test]
    fn test_remove_symlink_if_target_under_relative_link_wrong_prefix() {
        // Symlink uses relative path pointing outside prefix
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let other_package = dir.path().join("other_package");
        let target = other_package.join("bin/tool");
        let prefix = dir.path().join("my_package");
        let links_dir = dir.path().join("links");
        let link = links_dir.join("tool");

        rt.create_dir_all(target.parent().unwrap()).unwrap();
        rt.write(&target, b"content").unwrap();
        rt.create_dir_all(&links_dir).unwrap();
        rt.create_dir_all(&prefix).unwrap();

        // Create symlink with relative path to other_package
        let relative_target = PathBuf::from("../other_package/bin/tool");
        rt.symlink(&relative_target, &link).unwrap();

        let result = rt.remove_symlink_if_target_under(&link, &prefix, "test link");
        assert!(result.is_ok());
        assert!(!result.unwrap()); // false = skipped (target not under prefix)
        assert!(rt.is_symlink(&link));
    }

    #[test]
    fn test_remove_symlink_if_target_under_nested_prefix() {
        // Target is deeply nested under prefix
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let prefix = dir.path().join("a");
        let target = prefix.join("b/c/d/e/file.txt");
        let link = dir.path().join("link");

        rt.create_dir_all(target.parent().unwrap()).unwrap();
        rt.write(&target, b"content").unwrap();
        rt.symlink(&target, &link).unwrap();

        let result = rt.remove_symlink_if_target_under(&link, &prefix, "test link");
        assert!(result.is_ok());
        assert!(result.unwrap()); // true = removed
    }

    // --- normalize_path tests ---

    #[test]
    fn test_normalize_path_simple() {
        // --- Test: Normalize path without special components ---
        // Input: /a/b/c
        // Expected: /a/b/c
        let path = PathBuf::from("/a/b/c");
        let result = normalize_path(&path);
        assert_eq!(result, PathBuf::from("/a/b/c"));
    }

    #[test]
    fn test_normalize_path_with_dot() {
        // --- Test: Normalize path with . (current directory) components ---
        // Input: /a/./b/./c
        // Expected: /a/b/c
        let path = PathBuf::from("/a/./b/./c");
        let result = normalize_path(&path);
        assert_eq!(result, PathBuf::from("/a/b/c"));
    }

    #[test]
    fn test_normalize_path_with_parent_dir() {
        // --- Test: Normalize path with .. (parent directory) components ---
        // Input: /a/b/../c
        // Expected: /a/c
        let path = PathBuf::from("/a/b/../c");
        let result = normalize_path(&path);
        assert_eq!(result, PathBuf::from("/a/c"));
    }

    #[test]
    fn test_normalize_path_multiple_parent_dirs() {
        // --- Test: Normalize path with multiple .. components ---
        // Input: /a/b/c/../../d
        // Expected: /a/d
        let path = PathBuf::from("/a/b/c/../../d");
        let result = normalize_path(&path);
        assert_eq!(result, PathBuf::from("/a/d"));
    }

    #[test]
    fn test_normalize_path_mixed_components() {
        // --- Test: Normalize path with mixed . and .. components ---
        // Input: /a/./b/../c/./d/../e
        // Expected: /a/c/e
        let path = PathBuf::from("/a/./b/../c/./d/../e");
        let result = normalize_path(&path);
        assert_eq!(result, PathBuf::from("/a/c/e"));
    }

    #[test]
    fn test_normalize_path_parent_at_root() {
        // --- Test: Normalize path with .. at root level ---
        // Input: /../a/b
        // Expected: /../a/b (can't go above root, so .. is preserved)
        // Note: This is a lexical operation, not filesystem-aware
        let path = PathBuf::from("/../a/b");
        let result = normalize_path(&path);
        // On Unix, this becomes /../a/b since we can't pop past root
        // The behavior might differ but the path should be normalized
        assert!(
            result.to_string_lossy().contains("a/b") || result.to_string_lossy().contains("a\\b")
        );
    }

    #[test]
    fn test_normalize_path_relative() {
        // --- Test: Normalize relative path ---
        // Input: a/b/../c/./d
        // Expected: a/c/d
        let path = PathBuf::from("a/b/../c/./d");
        let result = normalize_path(&path);
        assert_eq!(result, PathBuf::from("a/c/d"));
    }

    #[test]
    fn test_normalize_path_trailing_parent() {
        // --- Test: Normalize path ending with .. ---
        // Input: /a/b/c/..
        // Expected: /a/b
        let path = PathBuf::from("/a/b/c/..");
        let result = normalize_path(&path);
        assert_eq!(result, PathBuf::from("/a/b"));
    }

    #[test]
    fn test_normalize_path_only_dots() {
        // --- Test: Normalize path with only . components ---
        // Input: /./././
        // Expected: /
        let path = PathBuf::from("/./././");
        let result = normalize_path(&path);
        assert_eq!(result, PathBuf::from("/"));
    }

    #[cfg(windows)]
    #[test]
    fn test_normalize_path_windows_style() {
        // --- Test: Normalize Windows-style path ---
        // Input: C:\a\b\..\c
        // Expected: C:\a\c
        let path = PathBuf::from(r"C:\a\b\..\c");
        let result = normalize_path(&path);
        assert_eq!(result, PathBuf::from(r"C:\a\c"));
    }

    #[cfg(windows)]
    #[test]
    fn test_normalize_path_windows_with_dots() {
        // --- Test: Normalize Windows path with . and .. ---
        // Input: C:\Users\.\foo\..\bar
        // Expected: C:\Users\bar
        let path = PathBuf::from(r"C:\Users\.\foo\..\bar");
        let result = normalize_path(&path);
        assert_eq!(result, PathBuf::from(r"C:\Users\bar"));
    }

    // --- is_path_under tests ---

    #[test]
    fn test_is_path_under_simple() {
        // --- Test: Simple path under directory ---
        // Path: /usr/local/bin/tool
        // Dir:  /usr/local
        // Expected: true
        let path = Path::new("/usr/local/bin/tool");
        let dir = Path::new("/usr/local");
        assert!(is_path_under(path, dir));
    }

    #[test]
    fn test_is_path_under_same_path() {
        // --- Test: Path equals directory ---
        // Path: /usr/local
        // Dir:  /usr/local
        // Expected: true (path is under itself)
        let path = Path::new("/usr/local");
        let dir = Path::new("/usr/local");
        assert!(is_path_under(path, dir));
    }

    #[test]
    fn test_is_path_under_not_under() {
        // --- Test: Path not under directory ---
        // Path: /opt/bin/tool
        // Dir:  /usr/local
        // Expected: false
        let path = Path::new("/opt/bin/tool");
        let dir = Path::new("/usr/local");
        assert!(!is_path_under(path, dir));
    }

    #[test]
    fn test_is_path_under_partial_component_match() {
        // --- Test: Partial component match (security check) ---
        // Path: /usr/local-other/bin/tool
        // Dir:  /usr/local
        // Expected: false (different component, not just prefix string)
        let path = Path::new("/usr/local-other/bin/tool");
        let dir = Path::new("/usr/local");
        assert!(!is_path_under(path, dir));
    }

    #[test]
    fn test_is_path_under_directory_traversal_attack() {
        // --- Test: Directory traversal attack prevention ---
        // Path: /usr/local/bin/../../../etc/passwd
        // Dir:  /usr/local
        // Expected: false (after normalization, path is /etc/passwd)
        let path = Path::new("/usr/local/bin/../../../etc/passwd");
        let dir = Path::new("/usr/local");
        assert!(!is_path_under(path, dir));
    }

    #[test]
    fn test_is_path_under_directory_traversal_subtle() {
        // --- Test: Subtle directory traversal ---
        // Path: /home/user/.ghri/owner/repo/v1/../../other/malicious
        // Dir:  /home/user/.ghri/owner/repo
        // Expected: false (after normalization, path is /home/user/.ghri/owner/other/malicious)
        let path = Path::new("/home/user/.ghri/owner/repo/v1/../../other/malicious");
        let dir = Path::new("/home/user/.ghri/owner/repo");
        assert!(!is_path_under(path, dir));
    }

    #[test]
    fn test_is_path_under_with_dot_components() {
        // --- Test: Path with . components ---
        // Path: /usr/local/./bin/./tool
        // Dir:  /usr/local
        // Expected: true (after normalization, path is /usr/local/bin/tool)
        let path = Path::new("/usr/local/./bin/./tool");
        let dir = Path::new("/usr/local");
        assert!(is_path_under(path, dir));
    }

    #[test]
    fn test_is_path_under_normalized_still_under() {
        // --- Test: Path with .. that still remains under directory ---
        // Path: /usr/local/bin/../lib/tool
        // Dir:  /usr/local
        // Expected: true (after normalization, path is /usr/local/lib/tool)
        let path = Path::new("/usr/local/bin/../lib/tool");
        let dir = Path::new("/usr/local");
        assert!(is_path_under(path, dir));
    }

    #[test]
    fn test_is_path_under_relative_paths() {
        // --- Test: Relative paths ---
        // Path: owner/repo/v1/bin/tool
        // Dir:  owner/repo
        // Expected: true
        let path = Path::new("owner/repo/v1/bin/tool");
        let dir = Path::new("owner/repo");
        assert!(is_path_under(path, dir));
    }

    #[test]
    fn test_is_path_under_path_shorter_than_dir() {
        // --- Test: Path shorter than directory ---
        // Path: /usr
        // Dir:  /usr/local/bin
        // Expected: false
        let path = Path::new("/usr");
        let dir = Path::new("/usr/local/bin");
        assert!(!is_path_under(path, dir));
    }

    #[cfg(windows)]
    #[test]
    fn test_is_path_under_windows() {
        // --- Test: Windows paths ---
        // Path: C:\Users\foo\AppData\Local\ghri\owner\repo\v1\tool.exe
        // Dir:  C:\Users\foo\AppData\Local\ghri
        // Expected: true
        let path = Path::new(r"C:\Users\foo\AppData\Local\ghri\owner\repo\v1\tool.exe");
        let dir = Path::new(r"C:\Users\foo\AppData\Local\ghri");
        assert!(is_path_under(path, dir));
    }

    #[cfg(windows)]
    #[test]
    fn test_is_path_under_windows_traversal() {
        // --- Test: Windows directory traversal attack ---
        // Path: C:\Users\foo\ghri\owner\repo\..\..\..\Windows\System32\cmd.exe
        // Dir:  C:\Users\foo\ghri
        // Expected: false
        let path = Path::new(r"C:\Users\foo\ghri\owner\repo\..\..\..\Windows\System32\cmd.exe");
        let dir = Path::new(r"C:\Users\foo\ghri");
        assert!(!is_path_under(path, dir));
    }

    #[cfg(windows)]
    #[test]
    fn test_is_path_under_windows_partial_match() {
        // --- Test: Windows partial component match ---
        // Path: C:\Users\foobar\data
        // Dir:  C:\Users\foo
        // Expected: false (foobar != foo)
        let path = Path::new(r"C:\Users\foobar\data");
        let dir = Path::new(r"C:\Users\foo");
        assert!(!is_path_under(path, dir));
    }

    // ==================== relative_symlink_path tests ====================

    #[test]
    fn test_relative_symlink_path_same_parent() {
        // --- Test: Symlink and target in same directory ---
        // Link:   /opt/bin/tool
        // Target: /opt/bin/actual-tool
        // Expected: actual-tool (just the filename)
        let link = Path::new("/opt/bin/tool");
        let target = Path::new("/opt/bin/actual-tool");
        let result = relative_symlink_path(link, target);
        assert_eq!(result, Some(PathBuf::from("actual-tool")));
    }

    #[test]
    fn test_relative_symlink_path_sibling_directory() {
        // --- Test: Target in sibling directory ---
        // Link:   /usr/local/bin/tool
        // Target: /usr/local/ghri/owner/repo/v1/tool
        // Expected: ../ghri/owner/repo/v1/tool
        let link = Path::new("/usr/local/bin/tool");
        let target = Path::new("/usr/local/ghri/owner/repo/v1/tool");
        let result = relative_symlink_path(link, target);
        assert_eq!(result, Some(PathBuf::from("../ghri/owner/repo/v1/tool")));
    }

    #[test]
    fn test_relative_symlink_path_deeper_nesting() {
        // --- Test: Target in deeply nested directory ---
        // Link:   /usr/local/bin/tool
        // Target: /opt/ghri/owner/repo/v1/tool
        // From /usr/local/bin to /opt: go up 3 levels (bin -> local -> usr -> /)
        // Expected: ../../../opt/ghri/owner/repo/v1/tool
        let link = Path::new("/usr/local/bin/tool");
        let target = Path::new("/opt/ghri/owner/repo/v1/tool");
        let result = relative_symlink_path(link, target);
        assert_eq!(
            result,
            Some(PathBuf::from("../../../opt/ghri/owner/repo/v1/tool"))
        );
    }

    #[test]
    fn test_relative_symlink_path_example_from_spec() {
        // --- Test: Example from user spec ---
        // ghri root: /opt/ghri
        // Link:   /opt/lib/bach
        // Target: /opt/ghri/bach-sh/bach/0.7.2
        // Expected: ../ghri/bach-sh/bach/0.7.2
        let link = Path::new("/opt/lib/bach");
        let target = Path::new("/opt/ghri/bach-sh/bach/0.7.2");
        let result = relative_symlink_path(link, target);
        assert_eq!(result, Some(PathBuf::from("../ghri/bach-sh/bach/0.7.2")));
    }

    #[test]
    fn test_relative_symlink_path_sibling_directories() {
        // --- Test: E2E scenario - sibling directories with similar names ---
        // install_root: /tmp/xxx/external_link_relative
        // bin_dir:      /tmp/xxx/external_link_relative_bin
        // Link:   /tmp/xxx/external_link_relative_bin/bach
        // Target: /tmp/xxx/external_link_relative/bach-sh/bach/0.7.2
        // Expected: ../external_link_relative/bach-sh/bach/0.7.2
        let link = Path::new("/tmp/xxx/external_link_relative_bin/bach");
        let target = Path::new("/tmp/xxx/external_link_relative/bach-sh/bach/0.7.2");
        let result = relative_symlink_path(link, target);
        assert_eq!(
            result,
            Some(PathBuf::from(
                "../external_link_relative/bach-sh/bach/0.7.2"
            ))
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_relative_symlink_path_windows() {
        // --- Test: Windows paths in same drive ---
        // Link:   C:\Users\foo\bin\tool.exe
        // Target: C:\Users\foo\ghri\owner\repo\v1\tool.exe
        // Expected: ..\ghri\owner\repo\v1\tool.exe
        let link = Path::new(r"C:\Users\foo\bin\tool.exe");
        let target = Path::new(r"C:\Users\foo\ghri\owner\repo\v1\tool.exe");
        let result = relative_symlink_path(link, target);
        assert_eq!(
            result,
            Some(PathBuf::from(r"..\ghri\owner\repo\v1\tool.exe"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_relative_symlink_path_windows_different_drives() {
        // --- Test: Windows paths on different drives ---
        // Link:   C:\Users\foo\bin\tool.exe
        // Target: D:\ghri\owner\repo\v1\tool.exe
        // Expected: None (cannot compute relative path across drives)
        let link = Path::new(r"C:\Users\foo\bin\tool.exe");
        let target = Path::new(r"D:\ghri\owner\repo\v1\tool.exe");
        let result = relative_symlink_path(link, target);
        assert_eq!(result, None);
    }
}
