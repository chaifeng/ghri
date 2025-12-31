//! Remove action - handles package and version removal.

use std::path::Path;

use anyhow::Result;
use log::debug;

use crate::domain::model::{Meta, PackageContext};
use crate::domain::service::{LinkManager, PackageRepository};
use crate::runtime::Runtime;

/// Remove action - handles removal of packages and versions
pub struct RemoveAction<'a, R: Runtime> {
    package_repo: PackageRepository<'a, R>,
    link_manager: LinkManager<'a, R>,
}

impl<'a, R: Runtime> RemoveAction<'a, R> {
    /// Create a new remove action
    pub fn new(runtime: &'a R, install_root: impl Into<std::path::PathBuf>) -> Self {
        let install_root = install_root.into();
        Self {
            package_repo: PackageRepository::new(runtime, install_root),
            link_manager: LinkManager::new(runtime),
        }
    }

    /// Get reference to package repository
    pub fn package_repo(&self) -> &PackageRepository<'a, R> {
        &self.package_repo
    }

    /// Remove links pointing to a directory.
    ///
    /// This removes symlinks for both regular links and versioned links
    /// that point inside the given directory.
    fn remove_links_under(&self, meta: &Meta, dir: &Path) {
        for rule in &meta.links {
            let _ = self.link_manager.remove_link_if_under(&rule.dest, dir);
        }
        for link in &meta.versioned_links {
            let _ = self.link_manager.remove_link_if_under(&link.dest, dir);
        }
    }

    /// Remove links for a specific version only.
    ///
    /// For versioned_links, only removes links that match the version.
    /// For regular links, removes if pointing to the version directory.
    fn remove_version_links(&self, meta: &Meta, version: &str, version_dir: &Path) {
        for rule in &meta.links {
            let _ = self
                .link_manager
                .remove_link_if_under(&rule.dest, version_dir);
        }
        for link in &meta.versioned_links {
            if link.version == version {
                let _ = self
                    .link_manager
                    .remove_link_if_under(&link.dest, version_dir);
            }
        }
    }

    /// Remove a specific version of a package.
    ///
    /// # Arguments
    /// * `ctx` - Package context with version to remove
    /// * `force` - If true, allows removing the current version
    ///
    /// # Returns
    /// Ok(()) on success, Err if version is current and force is false
    pub fn remove_version(&self, ctx: &PackageContext, force: bool) -> Result<()> {
        let version = ctx.version();
        let version_dir = ctx.version_dir();
        debug!("Removing version {} from {}", version, ctx.display_name);

        if !self
            .package_repo
            .is_version_installed(&ctx.owner, &ctx.repo, version.as_str())
        {
            anyhow::bail!("Version {} is not installed.", version);
        }

        // Check if this is the current version
        let is_current =
            self.package_repo
                .is_current_version(&ctx.owner, &ctx.repo, version.as_str());

        if is_current && !force {
            anyhow::bail!(
                "Version {} is the current version. Use --force to remove it anyway.",
                version
            );
        }

        // Remove links pointing to this version
        self.remove_version_links(&ctx.meta, version.as_str(), version_dir);

        // Update meta.json to remove versioned_links for this version
        if let Ok(Some(mut updated_meta)) = self.package_repo.load(&ctx.owner, &ctx.repo) {
            let original_len = updated_meta.versioned_links.len();
            updated_meta
                .versioned_links
                .retain(|l| l.version != version.as_str());
            if updated_meta.versioned_links.len() != original_len {
                debug!(
                    "Removed {} versioned link(s) from meta.json",
                    original_len - updated_meta.versioned_links.len()
                );
                self.package_repo
                    .save(&ctx.owner, &ctx.repo, &updated_meta)?;
            }
        }

        // Remove the version directory
        debug!("Removing version directory {:?}", version_dir);
        self.package_repo
            .remove_version_dir(&ctx.owner, &ctx.repo, version.as_str())?;

        // If this was the current version, remove the current symlink
        if is_current {
            debug!("Removing current symlink");
            let current_link = self.package_repo.current_link(&ctx.owner, &ctx.repo);
            let _ = self.link_manager.remove_link(&current_link);
        }

        Ok(())
    }

    /// Remove an entire package and all its versions.
    pub fn remove_package(&self, ctx: &PackageContext) -> Result<()> {
        debug!("Removing package {}", ctx.display_name);

        // Remove all external links
        self.remove_links_under(&ctx.meta, &ctx.package_dir);

        // Remove the package directory (also cleans up empty owner directory)
        self.package_repo
            .remove_package_dir(&ctx.owner, &ctx.repo)?;

        Ok(())
    }

    /// Prune a specific version (used by prune command).
    ///
    /// Unlike `remove_version`, this doesn't check if it's the current version
    /// and assumes the caller has already validated that.
    pub fn prune_version(&self, owner: &str, repo: &str, version: &str, meta: &Meta) -> Result<()> {
        let version_dir = self.package_repo.version_dir(owner, repo, version);
        debug!("Pruning version {} from {}/{}", version, owner, repo);

        // Remove links pointing to this version
        self.remove_version_links(meta, version, &version_dir);

        // Remove the version directory
        self.package_repo.remove_version_dir(owner, repo, version)?;

        Ok(())
    }

    /// Update meta.json to remove versioned_links for pruned versions.
    pub fn update_meta_after_prune(
        &self,
        owner: &str,
        repo: &str,
        pruned_versions: &[String],
    ) -> Result<()> {
        if let Some(mut meta) = self.package_repo.load(owner, repo)? {
            let original_len = meta.versioned_links.len();
            meta.versioned_links
                .retain(|l| !pruned_versions.contains(&l.version));
            if meta.versioned_links.len() != original_len {
                self.package_repo.save(owner, repo, &meta)?;
            }
        }
        Ok(())
    }
}
