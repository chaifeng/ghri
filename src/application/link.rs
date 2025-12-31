//! Link action - manages package symlinks.
//!
//! This module provides high-level link operations for packages:
//! - Creating links (with trailing slash handling, directory detection, etc.)
//! - Removing links (with safety checks and rule matching)
//! - Managing link rules in package metadata

use std::path::{Path, PathBuf};

use anyhow::Result;
use log::{debug, info, warn};

use crate::domain::model::{LinkRule, LinkStatus, PackageContext, RemoveLinkResult, VersionedLink};
use crate::domain::service::{LinkManager, PackageRepository};
use crate::runtime::{Runtime, resolve_relative_path};

/// Result of a link operation
#[derive(Debug)]
pub struct LinkResult {
    /// The final destination path where link was created
    pub dest: PathBuf,
    /// The target the link points to
    pub target: PathBuf,
    /// Whether this is a versioned link (specific version) or default link (follows current)
    pub is_versioned: bool,
}

/// Result of an unlink operation
#[derive(Debug)]
pub struct UnlinkResult {
    /// Number of links successfully removed
    pub removed_count: usize,
    /// Number of errors encountered
    pub error_count: usize,
    /// Links that were skipped (external targets)
    pub skipped_external: Vec<PathBuf>,
}

/// Link action - manages symlinks for installed packages
pub struct LinkAction<'a, R: Runtime> {
    runtime: &'a R,
    package_repo: PackageRepository<'a, R>,
    link_manager: LinkManager<'a, R>,
}

impl<'a, R: Runtime> LinkAction<'a, R> {
    /// Create a new link action
    pub fn new(runtime: &'a R, install_root: PathBuf) -> Self {
        Self {
            runtime,
            package_repo: PackageRepository::new(runtime, install_root.clone()),
            link_manager: LinkManager::new(runtime),
        }
    }

    /// Get reference to runtime
    pub fn runtime(&self) -> &R {
        self.runtime
    }

    /// Get reference to package repository
    pub fn package_repo(&self) -> &PackageRepository<'a, R> {
        &self.package_repo
    }

    /// Get reference to link manager
    pub fn link_manager(&self) -> &LinkManager<'a, R> {
        &self.link_manager
    }

    /// Find the default link target in a version directory
    pub fn find_default_target(&self, version_dir: &Path) -> Result<PathBuf> {
        self.link_manager.find_default_target(version_dir)
    }

    /// Prepare a link destination (check conflicts, remove existing if safe)
    pub fn prepare_link_destination(&self, dest: &Path, package_dir: &Path) -> Result<()> {
        self.link_manager
            .prepare_link_destination(dest, package_dir)
    }

    /// Create a symlink
    pub fn create_link(&self, target: &Path, link: &Path) -> Result<()> {
        self.link_manager.create_link(target, link)
    }

    /// Remove a symlink
    pub fn remove_link(&self, link: &Path) -> Result<bool> {
        self.link_manager.remove_link(link)
    }

    /// Check link status
    pub fn check_link(&self, dest: &Path, expected_prefix: &Path) -> LinkStatus {
        self.link_manager.check_link(dest, expected_prefix)
    }

    /// Check if a path exists
    pub fn exists(&self, path: &Path) -> bool {
        self.runtime.exists(path)
    }

    /// Check if a path is a directory
    pub fn is_dir(&self, path: &Path) -> bool {
        self.runtime.is_dir(path)
    }

    // ============== High-level link operations ==============

    /// Create a link for a package.
    ///
    /// This method handles all the complexity of link creation:
    /// - Resolving the destination path (relative to cwd if needed)
    /// - Handling trailing slash (forces directory behavior)
    /// - Detecting if destination is a directory or managed symlink
    /// - Creating the actual symlink
    /// - Updating package metadata
    ///
    /// # Arguments
    /// * `ctx` - Package context (must have version resolved)
    /// * `dest` - Destination path (may be relative)
    /// * `source_path` - Optional path inside version directory (e.g., "bin/tool")
    ///
    /// # Returns
    /// `LinkResult` with details about the created link
    pub fn create_package_link(
        &self,
        ctx: &mut PackageContext,
        dest: PathBuf,
        source_path: Option<String>,
    ) -> Result<LinkResult> {
        let version_dir = ctx.version_dir();
        if !self.runtime.exists(version_dir) {
            anyhow::bail!(
                "Version directory {:?} does not exist. The package may be corrupted.",
                version_dir
            );
        }

        // Check trailing slash before any path manipulation
        let has_trailing_slash = {
            let path_str = dest.to_string_lossy();
            path_str.ends_with('/') || path_str.ends_with('\\')
        };

        // Convert relative dest path to absolute using current working directory
        let dest = if dest.is_relative() {
            let cwd = self.runtime.current_dir()?;
            resolve_relative_path(&cwd, &dest)
        } else {
            dest
        };

        // Determine link target based on source_path or default behavior
        let link_target = if let Some(ref path) = source_path {
            let target = version_dir.join(path);
            if !self.runtime.exists(&target) {
                anyhow::bail!(
                    "Path '{}' does not exist in version {} of {}",
                    path,
                    ctx.version(),
                    ctx.display_name
                );
            }
            target
        } else {
            self.find_default_target(version_dir)?
        };

        // Determine final destination path
        let final_dest =
            self.resolve_link_destination(&dest, &link_target, ctx, has_trailing_slash)?;

        // Prepare destination (check conflicts, remove existing if safe)
        self.prepare_link_destination(&final_dest, &ctx.package_dir)?;

        // Create the symlink
        self.create_link(&link_target, &final_dest)?;

        // Update metadata
        let is_versioned = ctx.version_specified;
        self.update_meta_after_link(ctx, &final_dest, source_path.clone(), is_versioned);

        // Save metadata
        self.package_repo.save(&ctx.owner, &ctx.repo, &ctx.meta)?;

        info!(
            "Linked {} {} -> {:?}",
            ctx.display_name,
            ctx.version(),
            final_dest
        );

        Ok(LinkResult {
            dest: final_dest,
            target: link_target,
            is_versioned,
        })
    }

    /// Resolve the final destination path for a link.
    ///
    /// Handles:
    /// - Trailing slash (forces directory behavior)
    /// - Existing directory (create link inside)
    /// - Managed symlink (overwrite it)
    fn resolve_link_destination(
        &self,
        dest: &Path,
        link_target: &Path,
        ctx: &PackageContext,
        has_trailing_slash: bool,
    ) -> Result<PathBuf> {
        let is_symlink = self.runtime.is_symlink(dest);
        let is_dir = self.runtime.exists(dest) && self.runtime.is_dir(dest);

        let should_treat_as_dir = if has_trailing_slash {
            // If trailing slash is present, it MUST be treated as a directory
            if self.runtime.exists(dest) {
                if !is_dir {
                    anyhow::bail!("Path '{}' is not a directory", dest.display());
                }
                true
            } else {
                // Doesn't exist, but trailing slash implies directory
                true
            }
        } else if is_symlink {
            // If it's a symlink, check if we should overwrite it
            if self
                .link_manager
                .can_update_link(dest, &ctx.package_dir)
                .unwrap_or(false)
            {
                // It's a managed symlink, we want to overwrite it, so DO NOT treat as dir
                false
            } else {
                // It's an unmanaged symlink (or points outside), treat as dir if it points to one
                is_dir
            }
        } else {
            // Not a symlink, treat as dir if it is one
            is_dir
        };

        if should_treat_as_dir {
            let filename = self.determine_link_filename(link_target, ctx)?;
            Ok(dest.join(filename))
        } else {
            Ok(dest.to_path_buf())
        }
    }

    /// Determine the filename to use when linking into a directory.
    fn determine_link_filename(&self, link_target: &Path, ctx: &PackageContext) -> Result<String> {
        let version_dir = ctx.version_dir();

        if link_target == version_dir {
            // When linking to version directory (multiple files), use repo name
            Ok(ctx.repo.clone())
        } else {
            // When linking to single file, use that filename
            Ok(link_target
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| ctx.repo.clone()))
        }
    }

    /// Update package metadata after creating a link.
    fn update_meta_after_link(
        &self,
        ctx: &mut PackageContext,
        dest: &Path,
        source_path: Option<String>,
        is_versioned: bool,
    ) {
        if is_versioned {
            let new_link = VersionedLink {
                version: ctx.version().to_string(),
                dest: dest.to_path_buf(),
                path: source_path,
            };

            // Remove any existing entry with same dest from default links
            ctx.meta.links.retain(|l| l.dest != *dest);

            // Update or add versioned link
            if let Some(existing) = ctx
                .meta
                .versioned_links
                .iter_mut()
                .find(|l| l.dest == *dest)
            {
                existing.version = new_link.version;
                existing.path = new_link.path;
            } else {
                ctx.meta.versioned_links.push(new_link);
            }
        } else {
            let new_rule = LinkRule {
                dest: dest.to_path_buf(),
                path: source_path,
            };

            // Remove any existing entry with same dest from versioned links
            ctx.meta.versioned_links.retain(|l| l.dest != *dest);

            // Update or add default link
            if let Some(existing) = ctx.meta.links.iter_mut().find(|l| l.dest == *dest) {
                existing.path = new_rule.path;
            } else {
                ctx.meta.links.push(new_rule);
            }
        }

        // Clear legacy fields
        ctx.meta.linked_to = None;
        ctx.meta.linked_path = None;
    }

    /// Remove links for a package.
    ///
    /// # Arguments
    /// * `ctx` - Package context
    /// * `dest` - Specific destination to unlink (None means use `all` or `path`)
    /// * `path` - Specific source path to match (from link spec like "owner/repo:bin/tool")
    /// * `all` - Remove all links (for the version if version_specified, else default links)
    ///
    /// # Returns
    /// `UnlinkResult` with details about what was removed
    pub fn remove_package_links(
        &self,
        ctx: &mut PackageContext,
        dest: Option<PathBuf>,
        path: Option<String>,
        all: bool,
    ) -> Result<UnlinkResult> {
        // Convert relative dest path to absolute
        let dest = if let Some(d) = dest {
            if d.is_relative() {
                let cwd = self.runtime.current_dir()?;
                Some(resolve_relative_path(&cwd, &d))
            } else {
                Some(d)
            }
        } else {
            None
        };

        if ctx.meta.links.is_empty() && ctx.meta.versioned_links.is_empty() {
            anyhow::bail!("No link rules for {}.", ctx.display_name);
        }

        // Determine which rules to remove
        let rules_to_remove = self.find_rules_to_remove(ctx, dest.as_ref(), path.as_ref(), all)?;

        if rules_to_remove.is_empty() {
            return self.handle_no_matching_rules(ctx, dest.as_ref(), path.as_ref());
        }

        // Remove symlinks and rules
        let result = self.execute_unlink(ctx, &rules_to_remove, all)?;

        // Save updated meta
        self.package_repo.save(&ctx.owner, &ctx.repo, &ctx.meta)?;
        info!("Saved updated meta.json");

        Ok(result)
    }

    /// Find rules to remove based on criteria.
    fn find_rules_to_remove(
        &self,
        ctx: &PackageContext,
        dest: Option<&PathBuf>,
        path: Option<&String>,
        all: bool,
    ) -> Result<Vec<LinkRule>> {
        // Select candidates based on whether user specified a version
        let candidates: Vec<LinkRule> = if ctx.version_specified {
            debug!(
                "Version {} specified, looking in versioned_links",
                ctx.version()
            );
            ctx.meta
                .versioned_links
                .iter()
                .filter(|v| v.version == ctx.version().as_str())
                .map(|v| LinkRule {
                    dest: v.dest.clone(),
                    path: v.path.clone(),
                })
                .collect()
        } else {
            debug!("No version specified, looking in standard links");
            ctx.meta.links.clone()
        };

        let mut rules_to_remove = Vec::new();

        if all {
            debug!("Removing all matching link rules");
            rules_to_remove.extend(candidates);
        } else if let Some(dest_path) = dest {
            rules_to_remove.extend(self.find_rules_by_dest(
                &candidates,
                dest_path,
                &ctx.package_dir,
            ));
        } else if let Some(p) = path {
            debug!("Looking for rule with path {:?}", p);
            rules_to_remove.extend(
                candidates
                    .iter()
                    .filter(|r| r.path.as_ref() == Some(p))
                    .cloned(),
            );
        } else {
            // No destination specified and --all not set
            return Err(self.build_no_dest_error(ctx));
        }

        Ok(rules_to_remove)
    }

    /// Find rules matching a destination path.
    fn find_rules_by_dest(
        &self,
        candidates: &[LinkRule],
        dest_path: &Path,
        package_dir: &Path,
    ) -> Vec<LinkRule> {
        debug!("Looking for rule with dest {:?}", dest_path);

        let is_match = |dest: &PathBuf| {
            let rule_dest = if dest.is_relative() {
                resolve_relative_path(package_dir, dest)
            } else {
                dest.clone()
            };
            rule_dest == dest_path
        };

        // Try exact match first
        let mut matched: Vec<LinkRule> = candidates
            .iter()
            .filter(|r| is_match(&r.dest))
            .cloned()
            .collect();

        if matched.is_empty() {
            // Try filename match
            let dest_filename = dest_path.file_name().and_then(|s| s.to_str());
            debug!("No exact match, trying filename match: {:?}", dest_filename);

            let is_filename_match = |dest: &PathBuf| {
                let rule_dest = if dest.is_relative() {
                    resolve_relative_path(package_dir, dest)
                } else {
                    dest.clone()
                };
                rule_dest.file_name().and_then(|s| s.to_str()) == dest_filename
            };

            matched.extend(
                candidates
                    .iter()
                    .filter(|r| is_filename_match(&r.dest))
                    .cloned(),
            );
        }

        matched
    }

    /// Build error message when no destination specified.
    fn build_no_dest_error(&self, ctx: &PackageContext) -> anyhow::Error {
        let mut all_links = Vec::new();

        if ctx.version_specified {
            for r in &ctx.meta.versioned_links {
                if r.version == ctx.version().as_str() {
                    all_links.push(format!("  {:?} (version {})", r.dest, r.version));
                }
            }
        } else {
            for r in &ctx.meta.links {
                all_links.push(format!("  {:?}", r.dest));
            }
        }

        anyhow::anyhow!(
            "Please specify a destination path or use --all to remove all links.\n\
             Current link rules:\n{}",
            all_links.join("\n")
        )
    }

    /// Handle case when no matching rules found.
    fn handle_no_matching_rules(
        &self,
        ctx: &PackageContext,
        dest: Option<&PathBuf>,
        path: Option<&String>,
    ) -> Result<UnlinkResult> {
        debug!("No matching rules found");
        let search_target = dest
            .map(|d| format!("{:?}", d))
            .or_else(|| path.map(|p| format!("path '{}'", p)))
            .unwrap_or_else(|| "unknown".to_string());

        let mut all_links = Vec::new();

        if ctx.version_specified {
            for r in &ctx.meta.versioned_links {
                if r.version == ctx.version().as_str() {
                    if let Some(ref p) = r.path {
                        all_links.push(format!("  {} -> {:?} (version {})", p, r.dest, r.version));
                    } else {
                        all_links.push(format!(
                            "  (default) -> {:?} (version {})",
                            r.dest, r.version
                        ));
                    }
                }
            }
        } else {
            for r in &ctx.meta.links {
                if let Some(ref p) = r.path {
                    all_links.push(format!("  {} -> {:?}", p, r.dest));
                } else {
                    all_links.push(format!("  (default) -> {:?}", r.dest));
                }
            }
        }

        anyhow::bail!(
            "No link rule found matching {}.\n\
             Current link rules:\n{}",
            search_target,
            all_links.join("\n")
        )
    }

    /// Execute the actual unlink operation.
    fn execute_unlink(
        &self,
        ctx: &mut PackageContext,
        rules_to_remove: &[LinkRule],
        all: bool,
    ) -> Result<UnlinkResult> {
        let mut removed_count = 0;
        let mut error_count = 0;
        let mut skipped_external = Vec::new();

        for rule in rules_to_remove {
            debug!("Processing rule: {:?}", rule);

            match self
                .link_manager
                .remove_link_safely(&rule.dest, &ctx.package_dir)?
            {
                RemoveLinkResult::Removed => {
                    info!("Removed symlink {:?}", rule.dest);
                    removed_count += 1;
                }
                RemoveLinkResult::NotExists => {
                    debug!("Symlink {:?} does not exist, removing rule only", rule.dest);
                    removed_count += 1;
                }
                RemoveLinkResult::NotSymlink => {
                    warn!(
                        "Path {:?} exists but is not a symlink, skipping removal",
                        rule.dest
                    );
                    error_count += 1;
                    continue; // Don't remove this rule from meta
                }
                RemoveLinkResult::ExternalTarget => {
                    if all {
                        warn!(
                            "Skipping symlink {:?}: points outside package directory {:?}",
                            rule.dest, ctx.package_dir
                        );
                        skipped_external.push(rule.dest.clone());
                        error_count += 1;
                        continue; // Don't remove this rule from meta
                    } else {
                        anyhow::bail!(
                            "Cannot remove symlink {:?}: it points to external path which is outside the package directory {:?}",
                            rule.dest,
                            ctx.package_dir
                        );
                    }
                }
                RemoveLinkResult::Unresolvable => {
                    if all {
                        warn!(
                            "Cannot resolve symlink target for {:?}, skipping",
                            rule.dest
                        );
                        error_count += 1;
                        continue; // Don't remove this rule from meta
                    } else {
                        anyhow::bail!(
                            "Cannot remove symlink {:?}: unable to resolve target",
                            rule.dest
                        );
                    }
                }
            }

            // Remove the rule from meta
            ctx.meta.links.retain(|r| r.dest != rule.dest);
            ctx.meta.versioned_links.retain(|r| r.dest != rule.dest);
            debug!(
                "Removed rule from meta, {} rules remaining",
                ctx.meta.links.len() + ctx.meta.versioned_links.len()
            );
        }

        Ok(UnlinkResult {
            removed_count,
            error_count,
            skipped_external,
        })
    }
}
