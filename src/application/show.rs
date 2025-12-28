//! Show action - retrieves detailed package information.

use std::path::PathBuf;

use anyhow::Result;

use crate::domain::model::LinkRule;
use crate::domain::service::PackageRepository;
use crate::provider::{PackageSpec, Release};
use crate::runtime::Runtime;

/// Detailed information about an installed package
#[derive(Debug)]
pub struct PackageDetails {
    /// Package name (owner/repo)
    pub name: String,
    /// Package directory path
    pub package_dir: PathBuf,
    /// Current version (from symlink or meta)
    pub current_version: Option<String>,
    /// List of installed versions
    pub installed_versions: Vec<String>,
    /// Package description
    pub description: Option<String>,
    /// Homepage URL
    pub homepage: Option<String>,
    /// License
    pub license: Option<String>,
    /// Last updated timestamp
    pub updated_at: Option<String>,
    /// Available releases (from cache)
    pub releases: Vec<Release>,
    /// Link rules
    pub links: Vec<LinkRule>,
    /// Versioned links
    pub versioned_links: Vec<crate::domain::model::VersionedLink>,
    /// Path to current symlink
    pub current_version_path: Option<PathBuf>,
}

/// Show action - retrieves package details
pub struct ShowAction<'a, R: Runtime> {
    runtime: &'a R,
    package_repo: PackageRepository<'a, R>,
}

impl<'a, R: Runtime> ShowAction<'a, R> {
    /// Create a new show action
    pub fn new(runtime: &'a R, install_root: PathBuf) -> Self {
        Self {
            runtime,
            package_repo: PackageRepository::new(runtime, install_root),
        }
    }

    /// Get detailed information about a package
    pub fn get_package_details(&self, spec: &PackageSpec) -> Result<PackageDetails> {
        let owner = &spec.repo.owner;
        let repo = &spec.repo.repo;

        if !self.package_repo.package_exists(owner, repo) {
            anyhow::bail!("Package {} is not installed.", spec.repo);
        }

        let package_dir = self.package_repo.package_dir(owner, repo);
        let current_version_path = self.package_repo.current_version_dir(owner, repo);
        let current_version = self.package_repo.current_version(owner, repo);
        let mut installed_versions = self.package_repo.installed_versions(owner, repo)?;
        installed_versions.sort();

        // Load meta (may be None if meta.json doesn't exist)
        let meta = self.package_repo.load(owner, repo)?;

        // Determine current version (prefer symlink, fallback to meta)
        let effective_current = current_version.clone().or_else(|| {
            meta.as_ref()
                .filter(|m| !m.current_version.is_empty())
                .map(|m| m.current_version.clone())
        });

        Ok(PackageDetails {
            name: spec.repo.to_string(),
            package_dir,
            current_version: effective_current,
            installed_versions,
            description: meta.as_ref().and_then(|m| m.description.clone()),
            homepage: meta.as_ref().and_then(|m| m.homepage.clone()),
            license: meta.as_ref().and_then(|m| m.license.clone()),
            updated_at: meta
                .as_ref()
                .filter(|m| !m.updated_at.is_empty())
                .map(|m| m.updated_at.clone()),
            releases: meta
                .as_ref()
                .map(|m| m.releases.clone())
                .unwrap_or_default(),
            links: meta.as_ref().map(|m| m.links.clone()).unwrap_or_default(),
            versioned_links: meta
                .as_ref()
                .map(|m| m.versioned_links.clone())
                .unwrap_or_default(),
            current_version_path,
        })
    }

    /// Get reference to runtime (for link status checking in command layer)
    pub fn runtime(&self) -> &R {
        self.runtime
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::Meta;
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;

    #[test]
    fn test_get_package_details_not_installed() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/test/root");

        // Package doesn't exist
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/test/root/owner/repo")))
            .returning(|_| false);

        let action = ShowAction::new(&runtime, root);
        let spec = "owner/repo".parse::<PackageSpec>().unwrap();
        let result = action.get_package_details(&spec);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("is not installed"));
    }

    #[test]
    fn test_get_package_details_success() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/test/root");
        let package_dir = root.join("owner/repo");
        let meta_path = package_dir.join("meta.json");
        let current_link = package_dir.join("current");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // Current symlink exists and points to v1.0.0
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        // Read installed versions
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("/test/root/owner/repo/v1.0.0")]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/test/root/owner/repo/v1.0.0")))
            .returning(|_| true);

        // Meta exists
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1.0.0".into(),
            description: Some("Test package".into()),
            ..Default::default()
        };
        let meta_json = serde_json::to_string(&meta).unwrap();
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(move |_| Ok(meta_json.clone()));

        let action = ShowAction::new(&runtime, root);
        let spec = "owner/repo".parse::<PackageSpec>().unwrap();
        let details = action.get_package_details(&spec).unwrap();

        assert_eq!(details.name, "owner/repo");
        assert_eq!(details.current_version, Some("v1.0.0".into()));
        assert_eq!(details.installed_versions, vec!["v1.0.0"]);
        assert_eq!(details.description, Some("Test package".into()));
    }
}
