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
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");

        // Structure: /root/owner/repo/meta.json
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|p| Ok(vec![p.join("owner")]));
        runtime.expect_is_dir().returning(|_| true); // owner and repo are dirs
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner")))
            .returning(|p| Ok(vec![p.join("repo")]));
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        let packages = find_all_packages(&runtime, &root).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0], root.join("owner/repo/meta.json"));
    }

    #[test]
    fn test_find_all_packages_no_root() {
        let mut runtime = MockRuntime::new();
        let root = std::path::Path::new("/non-existent");
        runtime
            .expect_exists()
            .with(eq(root.to_path_buf()))
            .returning(|_| false);
        let packages = find_all_packages(&runtime, root).unwrap();
        assert!(packages.is_empty());
    }

    #[test]
    fn test_find_all_packages_empty_root() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/empty");

        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|_| Ok(vec![]));

        let packages = find_all_packages(&runtime, &root).unwrap();
        assert!(packages.is_empty());
    }

    #[test]
    fn test_find_all_packages_multiple() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");

        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Two owners
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|p| Ok(vec![p.join("owner1"), p.join("owner2")]));

        runtime.expect_is_dir().returning(|_| true);

        // owner1 has repo1
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner1")))
            .returning(|p| Ok(vec![p.join("repo1")]));

        // owner2 has repo2
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner2")))
            .returning(|p| Ok(vec![p.join("repo2")]));

        runtime
            .expect_exists()
            .with(eq(root.join("owner1/repo1/meta.json")))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(root.join("owner2/repo2/meta.json")))
            .returning(|_| true);

        let packages = find_all_packages(&runtime, &root).unwrap();
        assert_eq!(packages.len(), 2);
    }
}
