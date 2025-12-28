//! Prune action - identifies and removes unused package versions.

use std::path::PathBuf;

use anyhow::Result;

use crate::domain::service::PackageRepository;
use crate::runtime::Runtime;

/// Information about versions to prune for a package
#[derive(Debug)]
pub struct PruneInfo {
    /// Package name (owner/repo)
    pub name: String,
    /// Owner part of the package
    pub owner: String,
    /// Repo part of the package  
    pub repo: String,
    /// Current version (kept)
    pub current_version: Option<String>,
    /// Versions to be pruned
    pub versions_to_prune: Vec<String>,
}

/// Prune action - identifies versions to remove
pub struct PruneAction<'a, R: Runtime> {
    runtime: &'a R,
    package_repo: PackageRepository<'a, R>,
    install_root: PathBuf,
}

impl<'a, R: Runtime> PruneAction<'a, R> {
    /// Create a new prune action
    pub fn new(runtime: &'a R, install_root: PathBuf) -> Self {
        Self {
            runtime,
            package_repo: PackageRepository::new(runtime, install_root.clone()),
            install_root,
        }
    }

    /// Get the install root path
    pub fn install_root(&self) -> &PathBuf {
        &self.install_root
    }

    /// Get reference to runtime
    pub fn runtime(&self) -> &R {
        self.runtime
    }

    /// Get reference to package repository
    pub fn package_repo(&self) -> &PackageRepository<'a, R> {
        &self.package_repo
    }

    /// Find all packages and their prunable versions
    pub fn find_all_prunable(&self) -> Result<Vec<PruneInfo>> {
        let packages = self.package_repo.find_all_with_meta()?;

        let mut result = Vec::new();
        for (meta_path, meta) in packages {
            if let Some(package_dir) = meta_path.parent() {
                let repo = package_dir.file_name().and_then(|s| s.to_str());
                let owner = package_dir
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str());

                if let (Some(owner), Some(repo)) = (owner, repo)
                    && let Ok(info) = self.find_prunable(owner, repo, &meta.name)
                {
                    result.push(info);
                }
            }
        }

        Ok(result)
    }

    /// Find prunable versions for a specific package
    pub fn find_prunable(&self, owner: &str, repo: &str, name: &str) -> Result<PruneInfo> {
        if !self.package_repo.package_exists(owner, repo) {
            anyhow::bail!("Package {} is not installed.", name);
        }

        let current_version = self.package_repo.current_version(owner, repo);

        let versions_to_prune = if let Some(ref current) = current_version {
            self.package_repo
                .installed_versions(owner, repo)?
                .into_iter()
                .filter(|v| v != current)
                .collect()
        } else {
            vec![]
        };

        Ok(PruneInfo {
            name: name.to_string(),
            owner: owner.to_string(),
            repo: repo.to_string(),
            current_version,
            versions_to_prune,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;

    #[test]
    fn test_find_prunable_not_installed() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/test/root");

        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/test/root/owner/repo")))
            .returning(|_| false);

        let action = PruneAction::new(&runtime, root);
        let result = action.find_prunable("owner", "repo", "owner/repo");

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_find_prunable_no_current() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/test/root");
        let package_dir = root.join("owner/repo");
        let current_link = package_dir.join("current");

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "").into()));

        let action = PruneAction::new(&runtime, root);
        let info = action.find_prunable("owner", "repo", "owner/repo").unwrap();

        assert_eq!(info.current_version, None);
        assert!(info.versions_to_prune.is_empty());
    }

    #[test]
    fn test_find_prunable_with_old_versions() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/test/root");
        let package_dir = root.join("owner/repo");
        let current_link = package_dir.join("current");

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v2.0.0")));
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| {
                Ok(vec![
                    PathBuf::from("/test/root/owner/repo/v1.0.0"),
                    PathBuf::from("/test/root/owner/repo/v2.0.0"),
                ])
            });
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/test/root/owner/repo/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/test/root/owner/repo/v2.0.0")))
            .returning(|_| true);

        let action = PruneAction::new(&runtime, root);
        let info = action.find_prunable("owner", "repo", "owner/repo").unwrap();

        assert_eq!(info.current_version, Some("v2.0.0".into()));
        assert_eq!(info.versions_to_prune, vec!["v1.0.0"]);
    }

    #[test]
    fn test_find_all_prunable_empty() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/test/root");

        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|_| Ok(vec![]));

        let action = PruneAction::new(&runtime, root);
        let result = action.find_all_prunable().unwrap();

        assert!(result.is_empty());
    }
}
