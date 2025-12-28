//! List action - retrieves installed packages information.

use std::path::PathBuf;

use anyhow::Result;

use crate::domain::service::PackageRepository;
use crate::runtime::Runtime;

/// Information about an installed package
#[derive(Debug, Clone)]
pub struct PackageInfo {
    /// Package name (owner/repo)
    pub name: String,
    /// Current installed version
    pub version: String,
}

/// List action - queries installed packages
pub struct ListAction<'a, R: Runtime> {
    package_repo: PackageRepository<'a, R>,
}

impl<'a, R: Runtime> ListAction<'a, R> {
    /// Create a new list action
    pub fn new(runtime: &'a R, install_root: PathBuf) -> Self {
        Self {
            package_repo: PackageRepository::new(runtime, install_root),
        }
    }

    /// List all installed packages
    pub fn list_packages(&self) -> Result<Vec<PackageInfo>> {
        let packages = self.package_repo.find_all_with_meta()?;

        Ok(packages
            .into_iter()
            .map(|(_path, meta)| PackageInfo {
                name: meta.name,
                version: if meta.current_version.is_empty() {
                    "(unknown)".to_string()
                } else {
                    meta.current_version
                },
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::Meta;
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;

    #[test]
    fn test_list_packages_empty() {
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

        let action = ListAction::new(&runtime, root);
        let result = action.list_packages().unwrap();

        assert!(result.is_empty());
    }

    #[test]
    fn test_list_packages_with_packages() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/test/root");
        let owner_dir = root.join("owner");
        let repo_dir = owner_dir.join("repo");
        let meta_path = repo_dir.join("meta.json");

        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(move |_| Ok(vec![PathBuf::from("/test/root/owner")]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/test/root/owner")))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(PathBuf::from("/test/root/owner")))
            .returning(|_| Ok(vec![PathBuf::from("/test/root/owner/repo")]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/test/root/owner/repo")))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1.0.0".into(),
            ..Default::default()
        };
        let meta_json = serde_json::to_string(&meta).unwrap();
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(move |_| Ok(meta_json.clone()));

        let action = ListAction::new(&runtime, root);
        let result = action.list_packages().unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "owner/repo");
        assert_eq!(result[0].version, "v1.0.0");
    }
}
