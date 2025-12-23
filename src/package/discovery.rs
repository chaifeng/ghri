use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::runtime::Runtime;

/// Find all installed packages by scanning for meta.json files
///
/// Directory structure: `<root>/<owner>/<repo>/meta.json`
#[tracing::instrument(skip(runtime, root))]
pub fn find_all_packages<R: Runtime>(runtime: &R, root: &Path) -> Result<Vec<PathBuf>> {
    let mut meta_files = Vec::new();

    if !runtime.exists(root) {
        return Ok(meta_files);
    }

    // Root structure: <root>/<owner>/<repo>/meta.json
    for owner_path in runtime.read_dir(root)? {
        if runtime.is_dir(&owner_path) {
            for repo_path in runtime.read_dir(&owner_path)? {
                if runtime.is_dir(&repo_path) {
                    let meta_path = repo_path.join("meta.json");
                    if runtime.exists(&meta_path) {
                        meta_files.push(meta_path);
                    }
                }
            }
        }
    }

    Ok(meta_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;

    #[test]
    fn test_find_all_packages() {
        // Test finding a single installed package

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let root = PathBuf::from("/root");
        let owner_dir = root.join("owner"); // /root/owner
        let repo_dir = owner_dir.join("repo"); // /root/owner/repo
        let meta_path = repo_dir.join("meta.json"); // /root/owner/repo/meta.json

        // --- 1. Check Root Exists ---

        // Directory exists: /root -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // --- 2. Scan Root Directory ---

        // Read dir /root -> [/root/owner]
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|p| Ok(vec![p.join("owner")]));

        // Is dir checks for owner and repo
        runtime.expect_is_dir().returning(|_| true);

        // --- 3. Scan Owner Directory ---

        // Read dir /root/owner -> [/root/owner/repo]
        runtime
            .expect_read_dir()
            .with(eq(owner_dir))
            .returning(|p| Ok(vec![p.join("repo")]));

        // --- 4. Check for meta.json ---

        // File exists: /root/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // --- Execute & Verify ---

        let packages = find_all_packages(&runtime, &root).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0], meta_path);
    }

    #[test]
    fn test_find_all_packages_no_root() {
        // Test that empty list is returned when root directory doesn't exist

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let root = PathBuf::from("/non-existent");

        // --- Check Root Exists ---

        // Directory exists: /non-existent -> false
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| false);

        // --- Execute & Verify ---

        let packages = find_all_packages(&runtime, &root).unwrap();
        assert!(packages.is_empty());
    }

    #[test]
    fn test_find_all_packages_empty_root() {
        // Test that empty list is returned when root directory is empty

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let root = PathBuf::from("/empty");

        // --- 1. Check Root Exists ---

        // Directory exists: /empty -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // --- 2. Scan Root Directory ---

        // Read dir /empty -> [] (empty)
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|_| Ok(vec![]));

        // --- Execute & Verify ---

        let packages = find_all_packages(&runtime, &root).unwrap();
        assert!(packages.is_empty());
    }

    #[test]
    fn test_find_all_packages_multiple() {
        // Test finding multiple packages from different owners

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let root = PathBuf::from("/root");
        // Package 1: /root/owner1/repo1/meta.json
        // Package 2: /root/owner2/repo2/meta.json

        // --- 1. Check Root Exists ---

        // Directory exists: /root -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // --- 2. Scan Root Directory ---

        // Read dir /root -> [/root/owner1, /root/owner2]
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|p| Ok(vec![p.join("owner1"), p.join("owner2")]));

        // All paths are directories
        runtime.expect_is_dir().returning(|_| true);

        // --- 3. Scan Owner Directories ---

        // Read dir /root/owner1 -> [/root/owner1/repo1]
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner1")))
            .returning(|p| Ok(vec![p.join("repo1")]));

        // Read dir /root/owner2 -> [/root/owner2/repo2]
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner2")))
            .returning(|p| Ok(vec![p.join("repo2")]));

        // --- 4. Check for meta.json Files ---

        // File exists: /root/owner1/repo1/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(root.join("owner1/repo1/meta.json")))
            .returning(|_| true);

        // File exists: /root/owner2/repo2/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(root.join("owner2/repo2/meta.json")))
            .returning(|_| true);

        // --- Execute & Verify ---

        let packages = find_all_packages(&runtime, &root).unwrap();
        assert_eq!(packages.len(), 2);
    }
}
