//! Path utility functions for normalization and comparison.

use std::path::{Component, Path, PathBuf};

/// Normalize a path by processing `.` and `..` components lexically.
/// This does not access the filesystem and does not follow symlinks.
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
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

/// Calculate the relative path from a directory to a target path.
/// This is used to store link destinations relative to the version directory.
///
/// For example, if version_dir is `/home/user/.ghri/owner/repo/v1.0.0` and
/// dest is `/home/user/.local/bin/tool`, this returns `../../../.local/bin/tool`.
///
/// Returns `None` if a relative path cannot be computed (e.g., different drive letters on Windows).
pub fn relative_path_from_dir(from_dir: &Path, to_path: &Path) -> Option<PathBuf> {
    let result = pathdiff::diff_paths(to_path, from_dir)?;

    // If the result is an absolute path, it means we couldn't compute a relative path
    // (e.g., different drives on Windows). Return None in this case.
    if result.is_absolute() {
        return None;
    }

    Some(result)
}

/// Resolve a relative path against a base directory to get an absolute path.
/// This is used to convert stored relative link destinations back to absolute paths.
///
/// For example, if base_dir is `/home/user/.ghri/owner/repo/v1.0.0` and
/// relative_path is `../../../.local/bin/tool`, this returns `/home/user/.local/bin/tool`.
pub fn resolve_relative_path(base_dir: &Path, relative_path: &Path) -> PathBuf {
    if relative_path.is_absolute() {
        // Already absolute, return as-is
        relative_path.to_path_buf()
    } else {
        // Join and normalize to resolve .. components
        normalize_path(&base_dir.join(relative_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path_simple() {
        assert_eq!(
            normalize_path(Path::new("/usr/local/bin")),
            PathBuf::from("/usr/local/bin")
        );
    }

    #[test]
    fn test_normalize_path_with_dot() {
        assert_eq!(
            normalize_path(Path::new("/usr/./local/./bin")),
            PathBuf::from("/usr/local/bin")
        );
    }

    #[test]
    fn test_normalize_path_with_parent_dir() {
        assert_eq!(
            normalize_path(Path::new("/usr/local/../bin")),
            PathBuf::from("/usr/bin")
        );
    }

    #[test]
    fn test_normalize_path_multiple_parent_dirs() {
        assert_eq!(
            normalize_path(Path::new("/usr/local/bin/../../lib")),
            PathBuf::from("/usr/lib")
        );
    }

    #[test]
    fn test_normalize_path_mixed_components() {
        assert_eq!(
            normalize_path(Path::new("/usr/./local/../bin/./tool")),
            PathBuf::from("/usr/bin/tool")
        );
    }

    #[test]
    fn test_normalize_path_parent_at_root() {
        // Going above root should keep the ..
        #[cfg(unix)]
        assert_eq!(
            normalize_path(Path::new("/usr/../../../etc")),
            PathBuf::from("/etc")
        );
    }

    #[test]
    fn test_normalize_path_relative() {
        assert_eq!(
            normalize_path(Path::new("foo/bar/../baz")),
            PathBuf::from("foo/baz")
        );
    }

    #[test]
    fn test_normalize_path_trailing_parent() {
        assert_eq!(
            normalize_path(Path::new("/usr/local/bin/..")),
            PathBuf::from("/usr/local")
        );
    }

    #[test]
    fn test_normalize_path_only_dots() {
        assert_eq!(normalize_path(Path::new("./././.")), PathBuf::from(""));
    }

    #[cfg(windows)]
    #[test]
    fn test_normalize_path_windows_style() {
        assert_eq!(
            normalize_path(Path::new("C:\\Users\\test\\file")),
            PathBuf::from("C:\\Users\\test\\file")
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_normalize_path_windows_with_dots() {
        assert_eq!(
            normalize_path(Path::new("C:\\Users\\test\\..\\other")),
            PathBuf::from("C:\\Users\\other")
        );
    }

    #[test]
    fn test_is_path_under_simple() {
        assert!(is_path_under(
            Path::new("/usr/local/bin/tool"),
            Path::new("/usr/local")
        ));
    }

    #[test]
    fn test_is_path_under_same_path() {
        assert!(is_path_under(
            Path::new("/usr/local"),
            Path::new("/usr/local")
        ));
    }

    #[test]
    fn test_is_path_under_not_under() {
        assert!(!is_path_under(
            Path::new("/etc/passwd"),
            Path::new("/usr/local")
        ));
    }

    #[test]
    fn test_is_path_under_partial_component_match() {
        // "/usr/local-extra" should NOT be under "/usr/local"
        assert!(!is_path_under(
            Path::new("/usr/local-extra/bin"),
            Path::new("/usr/local")
        ));
    }

    #[test]
    fn test_is_path_under_directory_traversal_attack() {
        // Directory traversal attack: path appears to be under /usr/local but escapes
        assert!(!is_path_under(
            Path::new("/usr/local/bin/../../../etc/passwd"),
            Path::new("/usr/local")
        ));
    }

    #[test]
    fn test_is_path_under_directory_traversal_subtle() {
        // Subtle traversal: stays within /usr but not under /usr/local
        assert!(!is_path_under(
            Path::new("/usr/local/../share/file"),
            Path::new("/usr/local")
        ));
    }

    #[test]
    fn test_is_path_under_with_dot_components() {
        assert!(is_path_under(
            Path::new("/usr/./local/./bin"),
            Path::new("/usr/local")
        ));
    }

    #[test]
    fn test_is_path_under_normalized_still_under() {
        // After normalization, path is still under dir
        assert!(is_path_under(
            Path::new("/usr/local/bin/../lib/file"),
            Path::new("/usr/local")
        ));
    }

    #[test]
    fn test_is_path_under_relative_paths() {
        assert!(is_path_under(
            Path::new("foo/bar/baz"),
            Path::new("foo/bar")
        ));
    }

    #[test]
    fn test_is_path_under_path_shorter_than_dir() {
        assert!(!is_path_under(
            Path::new("/usr"),
            Path::new("/usr/local/bin")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn test_is_path_under_windows() {
        assert!(is_path_under(
            Path::new("C:\\Users\\test\\Documents\\file.txt"),
            Path::new("C:\\Users\\test")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn test_is_path_under_windows_traversal() {
        assert!(!is_path_under(
            Path::new("C:\\Users\\test\\..\\other\\file.txt"),
            Path::new("C:\\Users\\test")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn test_is_path_under_windows_partial_match() {
        assert!(!is_path_under(
            Path::new("C:\\Users\\testing\\file.txt"),
            Path::new("C:\\Users\\test")
        ));
    }

    #[test]
    fn test_relative_symlink_path_same_parent() {
        // Symlink and target in same directory
        let result = relative_symlink_path(
            Path::new("/usr/local/bin/tool"),
            Path::new("/usr/local/bin/real-tool"),
        );
        assert_eq!(result, Some(PathBuf::from("real-tool")));
    }

    #[test]
    fn test_relative_symlink_path_sibling_directory() {
        // Target in sibling directory
        let result = relative_symlink_path(
            Path::new("/usr/local/bin/tool"),
            Path::new("/usr/local/lib/tool.so"),
        );
        assert_eq!(result, Some(PathBuf::from("../lib/tool.so")));
    }

    #[test]
    fn test_relative_symlink_path_deeper_nesting() {
        // Target deeply nested - from /home/user/bin to /opt means going up 3 levels
        // /home/user/bin -> parent is /home/user
        // From /home/user to /opt: ../../opt
        // So from /home/user/bin: ../../../opt
        let result = relative_symlink_path(
            Path::new("/home/user/bin/tool"),
            Path::new("/opt/ghri/owner/repo/v1/bin/tool"),
        );
        assert_eq!(
            result,
            Some(PathBuf::from("../../../opt/ghri/owner/repo/v1/bin/tool"))
        );
    }

    #[test]
    fn test_relative_symlink_path_example_from_spec() {
        // From /usr/local/bin -> parent is /usr/local
        // From /usr/local to /opt: ../../opt
        // So from /usr/local/bin: ../../../opt
        let result = relative_symlink_path(
            Path::new("/usr/local/bin/tool"),
            Path::new("/opt/ghri/owner/repo/v1/tool"),
        );
        assert_eq!(
            result,
            Some(PathBuf::from("../../../opt/ghri/owner/repo/v1/tool"))
        );
    }

    #[test]
    fn test_relative_symlink_path_sibling_directories() {
        // ~/.ghri/owner/repo/current -> v1
        let result = relative_symlink_path(
            Path::new("/home/user/.ghri/owner/repo/current"),
            Path::new("/home/user/.ghri/owner/repo/v1"),
        );
        assert_eq!(result, Some(PathBuf::from("v1")));
    }

    #[cfg(windows)]
    #[test]
    fn test_relative_symlink_path_windows() {
        let result = relative_symlink_path(
            Path::new("C:\\Users\\test\\bin\\tool"),
            Path::new("C:\\Users\\test\\lib\\tool.dll"),
        );
        assert_eq!(result, Some(PathBuf::from("..\\lib\\tool.dll")));
    }

    #[cfg(windows)]
    #[test]
    fn test_relative_symlink_path_windows_different_drives() {
        // Different drives on Windows - should return None
        let result = relative_symlink_path(
            Path::new("C:\\Users\\test\\bin\\tool"),
            Path::new("D:\\Programs\\tool.exe"),
        );
        assert_eq!(result, None);
    }

    #[test]
    fn test_relative_path_from_dir_basic() {
        // From package dir to link destination
        // Package dir: /home/user/.ghri/owner/repo
        // Link dest: /home/user/.local/bin/tool
        // Need to go up 3 levels (.ghri/owner/repo) then down to .local/bin/tool
        let result = relative_path_from_dir(
            Path::new("/home/user/.ghri/owner/repo"),
            Path::new("/home/user/.local/bin/tool"),
        );
        assert_eq!(result, Some(PathBuf::from("../../../.local/bin/tool")));
    }

    #[test]
    fn test_relative_path_from_dir_same_directory() {
        // Link in same directory as package
        let result = relative_path_from_dir(
            Path::new("/home/user/.ghri/owner/repo"),
            Path::new("/home/user/.ghri/owner/repo/latest"),
        );
        assert_eq!(result, Some(PathBuf::from("latest")));
    }

    #[test]
    fn test_relative_path_from_dir_sibling() {
        // Link in sibling directory
        let result = relative_path_from_dir(
            Path::new("/home/user/.ghri/owner/repo"),
            Path::new("/home/user/.ghri/owner/repo-latest"),
        );
        assert_eq!(result, Some(PathBuf::from("../repo-latest")));
    }

    #[test]
    fn test_resolve_relative_path_basic() {
        // Resolve ../../../.local/bin/tool from /home/user/.ghri/owner/repo
        let result = resolve_relative_path(
            Path::new("/home/user/.ghri/owner/repo"),
            Path::new("../../../.local/bin/tool"),
        );
        assert_eq!(result, PathBuf::from("/home/user/.local/bin/tool"));
    }

    #[test]
    fn test_resolve_relative_path_absolute_passthrough() {
        // If the path is already absolute, return it unchanged
        let result = resolve_relative_path(
            Path::new("/home/user/.ghri/owner/repo/v1.0.0"),
            Path::new("/usr/local/bin/tool"),
        );
        assert_eq!(result, PathBuf::from("/usr/local/bin/tool"));
    }

    #[test]
    fn test_resolve_relative_path_no_parent_dir() {
        // Relative path without .. components (link in same dir)
        let result = resolve_relative_path(
            Path::new("/home/user/.ghri/owner/repo"),
            Path::new("latest"),
        );
        assert_eq!(result, PathBuf::from("/home/user/.ghri/owner/repo/latest"));
    }

    #[test]
    fn test_roundtrip_relative_path() {
        // Test that converting to relative and back gives the original path
        let package_dir = Path::new("/home/user/.ghri/owner/repo");
        let original_dest = Path::new("/home/user/.local/bin/tool");

        let relative = relative_path_from_dir(package_dir, original_dest).unwrap();
        // Should be ../../../.local/bin/tool
        assert_eq!(relative, PathBuf::from("../../../.local/bin/tool"));

        let resolved = resolve_relative_path(package_dir, &relative);
        assert_eq!(resolved, original_dest);
    }
}
