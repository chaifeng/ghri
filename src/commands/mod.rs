use anyhow::{Context, Result};
use log::{debug, info, warn};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{
    archive::Extractor,
    cleanup::CleanupContext,
    download::download_file,
    github::{GetReleases, GitHubRepo, LinkSpec, Release, RepoSpec},
    package::{LinkRule, Meta, VersionedLink, find_all_packages},
    runtime::Runtime,
};

pub mod config;
mod links;
mod paths;
mod remove;
mod show;
mod symlink;

pub use links::links;
pub(crate) use links::{print_links, print_versioned_links};
pub use remove::remove;
pub use show::show;

use config::Config;
use paths::{default_install_root, get_target_dir};
use symlink::update_current_symlink;

#[tracing::instrument(skip(runtime, install_root, api_url))]
pub async fn install<R: Runtime + 'static>(
    runtime: R,
    repo_str: &str,
    install_root: Option<PathBuf>,
    api_url: Option<String>,
) -> Result<()> {
    let config = Config::new(runtime, install_root, api_url)?;
    run(repo_str, config).await
}

#[tracing::instrument(skip(config))]
pub async fn run<R: Runtime + 'static, G: GetReleases, E: Extractor>(
    repo_str: &str,
    config: Config<R, G, E>,
) -> Result<()> {
    let spec = repo_str.parse::<RepoSpec>()?;
    let installer = Installer::new(
        config.runtime,
        config.github,
        config.client,
        config.extractor,
    );
    installer.install(&spec.repo, spec.version.as_deref(), config.install_root).await
}

#[tracing::instrument(skip(runtime, install_root, api_url))]
pub async fn update<R: Runtime + 'static>(
    runtime: R,
    install_root: Option<PathBuf>,
    api_url: Option<String>,
) -> Result<()> {
    let config = Config::new(runtime, install_root, api_url)?;
    let installer = Installer::new(
        config.runtime,
        config.github,
        config.client,
        config.extractor,
    );
    installer.update_all(config.install_root).await
}

/// List all installed packages
#[tracing::instrument(skip(runtime, install_root))]
pub fn list<R: Runtime>(runtime: R, install_root: Option<PathBuf>) -> Result<()> {
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(&runtime)?,
    };

    debug!("Listing packages from {:?}", root);

    let meta_files = find_all_packages(&runtime, &root)?;
    if meta_files.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    debug!("Found {} package(s)", meta_files.len());

    for meta_path in meta_files {
        match Meta::load(&runtime, &meta_path) {
            Ok(meta) => {
                let version = if meta.current_version.is_empty() {
                    "(unknown)".to_string()
                } else {
                    meta.current_version.clone()
                };
                println!("{} {}", meta.name, version);
            }
            Err(e) => {
                debug!("Failed to load meta from {:?}: {}", meta_path, e);
            }
        }
    }

    Ok(())
}

/// Remove a link rule and its symlink
#[tracing::instrument(skip(runtime, install_root))]
pub fn unlink<R: Runtime>(
    runtime: R,
    repo_str: &str,
    dest: Option<PathBuf>,
    all: bool,
    install_root: Option<PathBuf>,
) -> Result<()> {
    debug!("Unlinking {} dest={:?} all={}", repo_str, dest, all);
    // Use LinkSpec to handle "owner/repo:path" format
    let spec = repo_str.parse::<LinkSpec>()?;
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(&runtime)?,
    };
    debug!("Using install root: {:?}", root);

    let package_dir = root.join(&spec.repo.owner).join(&spec.repo.repo);
    let meta_path = package_dir.join("meta.json");
    debug!("Loading meta from {:?}", meta_path);

    if !runtime.exists(&meta_path) {
        debug!("Meta file not found");
        anyhow::bail!(
            "Package {} is not installed.",
            spec.repo
        );
    }

    let mut meta = Meta::load(&runtime, &meta_path)?;
    debug!("Found {} link rules before unlink", meta.links.len());

    if meta.links.is_empty() {
        println!("No link rules for {}.", spec.repo);
        return Ok(());
    }

    // Determine which rules to remove
    let rules_to_remove: Vec<LinkRule> = if all {
        debug!("Removing all link rules");
        meta.links.clone()
    } else if let Some(ref dest_path) = dest {
        debug!("Looking for rule with dest {:?}", dest_path);
        // Find rules matching the destination
        let matching: Vec<_> = meta.links.iter()
            .filter(|r| r.dest == *dest_path)
            .cloned()
            .collect();
        if matching.is_empty() {
            // Try to find by partial match (filename)
            let dest_filename = dest_path.file_name().and_then(|s| s.to_str());
            debug!("No exact match, trying filename match: {:?}", dest_filename);
            meta.links.iter()
                .filter(|r| {
                    r.dest.file_name().and_then(|s| s.to_str()) == dest_filename
                })
                .cloned()
                .collect()
        } else {
            matching
        }
    } else if let Some(ref path) = spec.path {
        // Filter by path in the link rule (e.g., "bach-sh/bach:bach.sh")
        debug!("Looking for rule with path {:?}", path);
        meta.links.iter()
            .filter(|r| r.path.as_ref() == Some(path))
            .cloned()
            .collect()
    } else {
        debug!("No destination specified and --all not set");
        anyhow::bail!(
            "Please specify a destination path or use --all to remove all links.\n\
             Current link rules:\n{}",
            meta.links.iter()
                .map(|r| format!("  {:?}", r.dest))
                .collect::<Vec<_>>()
                .join("\n")
        );
    };

    if rules_to_remove.is_empty() {
        debug!("No matching rules found");
        let search_target = dest.as_ref()
            .map(|d| format!("{:?}", d))
            .or_else(|| spec.path.as_ref().map(|p| format!("path '{}'", p)))
            .unwrap_or_else(|| "unknown".to_string());
        anyhow::bail!(
            "No link rule found matching {}.\n\
             Current link rules:\n{}",
            search_target,
            meta.links.iter()
                .map(|r| {
                    if let Some(ref p) = r.path {
                        format!("  {} -> {:?}", p, r.dest)
                    } else {
                        format!("  (default) -> {:?}", r.dest)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    // Remove symlinks and rules
    let mut removed_count = 0;
    let mut error_count = 0;

    for rule in &rules_to_remove {
        debug!("Processing rule: {:?}", rule);

        // Try to remove the symlink
        if runtime.exists(&rule.dest) || runtime.is_symlink(&rule.dest) {
            if runtime.is_symlink(&rule.dest) {
                debug!("Removing symlink {:?}", rule.dest);
                match runtime.remove_symlink(&rule.dest) {
                    Ok(()) => {
                        info!("Removed symlink {:?}", rule.dest);
                        println!("Removed symlink {:?}", rule.dest);
                        removed_count += 1;
                    }
                    Err(e) => {
                        warn!("Failed to remove symlink {:?}: {}", rule.dest, e);
                        eprintln!("Warning: Failed to remove symlink {:?}: {}", rule.dest, e);
                        error_count += 1;
                    }
                }
            } else {
                warn!("Path {:?} exists but is not a symlink, skipping removal", rule.dest);
                eprintln!("Warning: {:?} is not a symlink, skipping removal", rule.dest);
                error_count += 1;
            }
        } else {
            debug!("Symlink {:?} does not exist, removing rule only", rule.dest);
            println!("Symlink {:?} does not exist, removing rule only", rule.dest);
            removed_count += 1;
        }

        // Remove the rule from meta
        meta.links.retain(|r| r.dest != rule.dest);
        debug!("Removed rule from meta, {} rules remaining", meta.links.len());
    }

    // Save updated meta
    debug!("Saving updated meta with {} rules", meta.links.len());
    let json = serde_json::to_string_pretty(&meta)?;
    let tmp_path = meta_path.with_extension("json.tmp");
    runtime.write(&tmp_path, json.as_bytes())?;
    runtime.rename(&tmp_path, &meta_path)?;
    info!("Saved updated meta.json");

    println!(
        "Unlinked {} rule(s) from {}{}",
        removed_count,
        spec.repo,
        if error_count > 0 { format!(" ({} error(s))", error_count) } else { String::new() }
    );

    Ok(())
}

/// Link a package's current version to a destination directory
#[tracing::instrument(skip(runtime, install_root))]
pub fn link<R: Runtime>(
    runtime: R,
    repo_str: &str,
    dest: PathBuf,
    install_root: Option<PathBuf>,
) -> Result<()> {
    let spec = repo_str.parse::<LinkSpec>()?;
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(&runtime)?,
    };

    // Find the package directory
    let package_dir = root.join(&spec.repo.owner).join(&spec.repo.repo);
    let meta_path = package_dir.join("meta.json");

    if !runtime.exists(&meta_path) {
        anyhow::bail!(
            "Package {} is not installed. Install it first with: ghri install {}",
            spec.repo,
            spec.repo
        );
    }

    let mut meta = Meta::load(&runtime, &meta_path)?;

    // Determine which version to link
    let version = if let Some(ref v) = spec.version {
        // Check if specified version exists
        if !runtime.exists(&package_dir.join(v)) {
            anyhow::bail!(
                "Version {} is not installed for {}. Install it first with: ghri install {}@{}",
                v,
                spec.repo,
                spec.repo,
                v
            );
        }
        v.clone()
    } else {
        if meta.current_version.is_empty() {
            anyhow::bail!(
                "No current version set for {}. Install a version first.",
                spec.repo
            );
        }
        meta.current_version.clone()
    };

    let version_dir = package_dir.join(&version);
    if !runtime.exists(&version_dir) {
        anyhow::bail!(
            "Version directory {:?} does not exist. The package may be corrupted.",
            version_dir
        );
    }

    // Determine link target based on spec.path or default behavior
    let link_target = if let Some(ref path) = spec.path {
        let target = version_dir.join(path);
        if !runtime.exists(&target) {
            anyhow::bail!(
                "Path '{}' does not exist in version {} of {}",
                path,
                version,
                spec.repo
            );
        }
        target
    } else {
        determine_link_target(&runtime, &version_dir)?
    };

    // If dest is an existing directory, create link inside it
    // Use the filename from the link target (either specified path or detected file)
    let final_dest = if runtime.exists(&dest) && runtime.is_dir(&dest) {
        let filename = if let Some(ref path) = spec.path {
            // Use the filename from the specified path
            Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| spec.repo.repo.clone())
        } else {
            // Use repo name for default behavior, or filename if linking to single file
            link_target
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| spec.repo.repo.clone())
        };
        dest.join(filename)
    } else {
        dest
    };

    // Create parent directory of destination if needed
    if let Some(parent) = final_dest.parent() {
        if !runtime.exists(parent) {
            runtime.create_dir_all(parent)?;
        }
    }

    // Handle existing destination
    if runtime.exists(&final_dest) || runtime.is_symlink(&final_dest) {
        if runtime.is_symlink(&final_dest) {
            // Check if the existing symlink points to a version in this package
            if let Ok(existing_target) = runtime.read_link(&final_dest) {
                let existing_target = if existing_target.is_relative() {
                    final_dest.parent().unwrap_or(Path::new(".")).join(&existing_target)
                } else {
                    existing_target
                };

                // Check if existing target is within the package directory
                if existing_target.starts_with(&package_dir) {
                    debug!(
                        "Updating existing link from {:?} to {:?}",
                        existing_target, link_target
                    );
                    runtime.remove_symlink(&final_dest)?;
                } else {
                    anyhow::bail!(
                        "Destination {:?} exists and points to {:?} which is not part of package {}",
                        final_dest,
                        existing_target,
                        spec.repo
                    );
                }
            } else {
                anyhow::bail!(
                    "Destination {:?} is a symlink but cannot read its target",
                    final_dest
                );
            }
        } else {
            anyhow::bail!(
                "Destination {:?} already exists and is not a symlink",
                final_dest
            );
        }
    }

    // Create the symlink
    runtime.symlink(&link_target, &final_dest)?;

    // Add or update link rule in meta.json
    // If a version was explicitly specified, save to versioned_links (not updated on install/update)
    // Otherwise save to links (updated on install/update)
    if spec.version.is_some() {
        let new_link = VersionedLink {
            version: version.clone(),
            dest: final_dest.clone(),
            path: spec.path.clone(),
        };

        // Check if a versioned link with the same dest already exists
        if let Some(existing) = meta.versioned_links.iter_mut().find(|l| l.dest == final_dest) {
            // Update existing versioned link
            existing.version = new_link.version;
            existing.path = new_link.path;
        } else {
            // Add new versioned link
            meta.versioned_links.push(new_link);
        }
    } else {
        let new_rule = LinkRule {
            dest: final_dest.clone(),
            path: spec.path.clone(),
        };

        // Check if a rule with the same dest already exists
        if let Some(existing) = meta.links.iter_mut().find(|l| l.dest == final_dest) {
            // Update existing rule
            existing.path = new_rule.path;
        } else {
            // Add new rule
            meta.links.push(new_rule);
        }
    }

    // Clear legacy fields (migration is done by apply_defaults on load)
    meta.linked_to = None;
    meta.linked_path = None;

    let json = serde_json::to_string_pretty(&meta)?;
    let tmp_path = meta_path.with_extension("json.tmp");
    runtime.write(&tmp_path, json.as_bytes())?;
    runtime.rename(&tmp_path, &meta_path)?;

    println!(
        "Linked {} {} -> {:?}",
        spec.repo, version, final_dest
    );

    Ok(())
}

/// Determine what to link to: if version dir has only one file, link to that file
fn determine_link_target<R: Runtime>(runtime: &R, version_dir: &Path) -> Result<PathBuf> {
    let entries = runtime.read_dir(version_dir)?;

    if entries.len() == 1 {
        let single_entry = &entries[0];
        if !runtime.is_dir(single_entry) {
            debug!(
                "Version directory has single file, linking to {:?}",
                single_entry
            );
            return Ok(single_entry.clone());
        }
    }

    // Multiple entries or single directory - link to version dir itself
    Ok(version_dir.to_path_buf())
}

/// Update external links for a package after installation
/// Iterates through all link rules and updates each symlink
#[tracing::instrument(skip(runtime, _package_dir, version_dir))]
pub(crate) fn update_external_links<R: Runtime>(
    runtime: &R,
    _package_dir: &Path,
    version_dir: &Path,
    meta: &Meta,
) -> Result<()> {
    let mut errors = Vec::new();

    for rule in &meta.links {
        if let Err(e) = update_single_link(runtime, version_dir, rule) {
            // Log error but continue with other links
            eprintln!(
                "Error updating link {:?}: {}",
                rule.dest, e
            );
            errors.push((rule.dest.clone(), e));
        }
    }

    // Also handle legacy linked_to field for backward compatibility
    if let Some(ref linked_to) = meta.linked_to {
        let legacy_rule = LinkRule {
            dest: linked_to.clone(),
            path: meta.linked_path.clone(),
        };
        if let Err(e) = update_single_link(runtime, version_dir, &legacy_rule) {
            eprintln!(
                "Error updating legacy link {:?}: {}",
                linked_to, e
            );
            errors.push((linked_to.clone(), e));
        }
    }

    if !errors.is_empty() {
        warn!(
            "{} link(s) failed to update, but continuing",
            errors.len()
        );
    }

    Ok(())
}

/// Update a single link according to a link rule
fn update_single_link<R: Runtime>(
    runtime: &R,
    version_dir: &Path,
    rule: &LinkRule,
) -> Result<()> {
    // Determine link target based on rule.path or default behavior
    let link_target = if let Some(ref path) = rule.path {
        let target = version_dir.join(path);
        if !runtime.exists(&target) {
            anyhow::bail!(
                "Path '{}' does not exist in {:?}",
                path, version_dir
            );
        }
        target
    } else {
        determine_link_target(runtime, version_dir)?
    };

    let linked_to = &rule.dest;

    if runtime.exists(linked_to) || runtime.is_symlink(linked_to) {
        if runtime.is_symlink(linked_to) {
            // Remove the old symlink
            runtime.remove_symlink(linked_to)?;

            // Create new symlink to the new version
            runtime.symlink(&link_target, linked_to)?;

            info!("Updated external link {:?} -> {:?}", linked_to, link_target);
        } else {
            warn!(
                "External link target {:?} exists but is not a symlink, skipping update",
                linked_to
            );
        }
    } else {
        // linked_to path doesn't exist anymore, create it
        if let Some(parent) = linked_to.parent() {
            if !runtime.exists(parent) {
                runtime.create_dir_all(parent)?;
            }
        }

        runtime.symlink(&link_target, linked_to)?;
        info!("Recreated external link {:?} -> {:?}", linked_to, link_target);
    }

    Ok(())
}

pub struct Installer<R: Runtime, G: GetReleases, E: Extractor> {
    pub runtime: R,
    pub github: G,
    pub client: Client,
    pub extractor: E,
}

impl<R: Runtime + 'static, G: GetReleases, E: Extractor> Installer<R, G, E> {
    #[tracing::instrument(skip(runtime, github, client, extractor))]
    pub fn new(runtime: R, github: G, client: Client, extractor: E) -> Self {
        Self {
            runtime,
            github,
            client,
            extractor,
        }
    }

    #[tracing::instrument(skip(self, repo, version, install_root))]
    pub async fn install(&self, repo: &GitHubRepo, version: Option<&str>, install_root: Option<PathBuf>) -> Result<()> {
        println!("   resolving {}", repo);
        let (mut meta, meta_path) = self
            .get_or_fetch_meta(repo, install_root.as_deref())
            .await?;

        let meta_release = if let Some(ver) = version {
            // Find the specific version
            meta.releases
                .iter()
                .find(|r| r.version == ver || r.version == format!("v{}", ver) || r.version.trim_start_matches('v') == ver.trim_start_matches('v'))
                .ok_or_else(|| anyhow::anyhow!("Version '{}' not found for {}. Available versions: {}", 
                    ver, repo, 
                    meta.releases.iter().take(5).map(|r| r.version.as_str()).collect::<Vec<_>>().join(", ")))?
        } else {
            // Get latest stable release
            meta.get_latest_stable_release()
                .ok_or_else(|| anyhow::anyhow!("No stable release found for {}. If you want to install a pre-release, specify the version with @version.", repo))?
        };

        info!("Found version: {}", meta_release.version);
        let release: Release = meta_release.clone().into();

        let target_dir = get_target_dir(&self.runtime, repo, &release, install_root)?;

        // Set up cleanup context for Ctrl-C handling
        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let cleanup_ctx_clone = Arc::clone(&cleanup_ctx);

        // Register Ctrl-C handler
        let ctrl_c_handler = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\nInterrupted, cleaning up...");
                cleanup_ctx_clone.lock().unwrap().cleanup();
                std::process::exit(130); // Standard exit code for Ctrl-C
            }
        });

        let result = ensure_installed(
            &self.runtime,
            &target_dir,
            repo,
            &release,
            &self.client,
            &self.extractor,
            Arc::clone(&cleanup_ctx),
        )
        .await;

        // Abort the Ctrl-C handler since installation completed (successfully or with error)
        ctrl_c_handler.abort();

        result?;

        update_current_symlink(&self.runtime, &target_dir, &release.tag_name)?;

        // Update external links if configured
        if let Some(parent) = target_dir.parent() {
            if let Err(e) = update_external_links(&self.runtime, parent, &target_dir, &meta) {
                warn!("Failed to update external links: {}. Continuing.", e);
            }
        }

        // Metadata handling
        meta.current_version = release.tag_name.clone();
        if let Err(e) = self.save_meta(&meta_path, &meta) {
            warn!("Failed to save package metadata: {}. Continuing.", e);
        }

        self.print_install_success(repo, &release.tag_name, &target_dir);

        Ok(())
    }

    #[tracing::instrument(skip(self, install_root))]
    pub async fn update_all(&self, install_root: Option<PathBuf>) -> Result<()> {
        let root = match install_root {
            Some(path) => path,
            None => default_install_root(&self.runtime)?,
        };

        let meta_files = find_all_packages(&self.runtime, &root)?;
        if meta_files.is_empty() {
            println!("No packages installed.");
            return Ok(());
        }

        for meta_path in meta_files {
            let meta = Meta::load(&self.runtime, &meta_path)?;
            let repo = meta.name.parse::<GitHubRepo>()?;

            println!("   updating {}", repo);
            if let Err(e) = self
                .save_metadata(&repo, &meta.current_version, &meta_path)
                .await
            {
                warn!("Failed to update metadata for {}: {}", repo, e);
            } else {
                // Check if update is available
                let updated_meta = Meta::load(&self.runtime, &meta_path)?;
                if let Some(latest) = updated_meta.get_latest_stable_release()
                    && meta.current_version != latest.version
                {
                    self.print_update_available(&repo, &meta.current_version, &latest.version);
                }
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip(self, repo, tag, target_dir))]
    fn print_install_success(&self, repo: &GitHubRepo, tag: &str, target_dir: &Path) {
        println!("   installed {} {} {}", repo, tag, target_dir.display());
    }

    #[tracing::instrument(skip(self, repo, current, latest))]
    fn print_update_available(&self, repo: &GitHubRepo, current: &str, latest: &str) {
        let current_display = if current.is_empty() {
            "(none)"
        } else {
            current
        };
        println!("  updatable {} {} -> {}", repo, current_display, latest);
    }

    #[tracing::instrument(skip(self, repo, install_root))]
    async fn get_or_fetch_meta(
        &self,
        repo: &GitHubRepo,
        install_root: Option<&Path>,
    ) -> Result<(Meta, PathBuf)> {
        let root = match install_root {
            Some(path) => path.to_path_buf(),
            None => default_install_root(&self.runtime)?,
        };
        let meta_path = root.join(&repo.owner).join(&repo.repo).join("meta.json");

        if self.runtime.exists(&meta_path) {
            match Meta::load(&self.runtime, &meta_path) {
                Ok(meta) => return Ok((meta, meta_path)),
                Err(e) => {
                    warn!(
                        "Failed to load existing meta.json at {:?}: {}. Re-fetching.",
                        meta_path, e
                    );
                }
            }
        }

        let meta = self.fetch_meta(repo, "", None).await?;

        if let Some(parent) = meta_path.parent() {
            self.runtime.create_dir_all(parent)?;
        }
        self.save_meta(&meta_path, &meta)?;

        Ok((meta, meta_path))
    }

    #[tracing::instrument(skip(self, repo, current_version, api_url))]
    async fn fetch_meta(
        &self,
        repo: &GitHubRepo,
        current_version: &str,
        api_url: Option<&str>,
    ) -> Result<Meta> {
        let api_url = api_url.unwrap_or(self.github.api_url());
        let repo_info = self.github.get_repo_info_at(repo, api_url).await?;
        let releases = self.github.get_releases_at(repo, api_url).await?;
        Ok(Meta::from(
            repo.clone(),
            repo_info,
            releases,
            current_version,
            api_url,
        ))
    }

    #[tracing::instrument(skip(self, meta_path, meta))]
    fn save_meta(&self, meta_path: &Path, meta: &Meta) -> Result<()> {
        let json = serde_json::to_string_pretty(meta)?;
        let tmp_path = meta_path.with_extension("json.tmp");

        self.runtime.write(&tmp_path, json.as_bytes())?;
        self.runtime.rename(&tmp_path, meta_path)?;
        Ok(())
    }

    #[tracing::instrument(skip(self, repo, current_version, target_dir))]
    async fn save_metadata(
        &self,
        repo: &GitHubRepo,
        current_version: &str,
        target_dir: &Path,
    ) -> Result<()> {
        let meta_path = if !self.runtime.is_dir(target_dir) {
            target_dir.to_path_buf()
        } else {
            let package_root = target_dir.parent().context("Failed to get package root")?;
            package_root.join("meta.json")
        };

        let existing_meta = if self.runtime.exists(&meta_path) {
            Meta::load(&self.runtime, &meta_path).ok()
        } else {
            None
        };

        let new_meta = self
            .fetch_meta(
                repo,
                current_version,
                existing_meta.as_ref().map(|m| m.api_url.as_str()),
            )
            .await?;

        let mut final_meta = if self.runtime.exists(&meta_path) {
            let mut existing = Meta::load(&self.runtime, &meta_path)?;
            if existing.merge(new_meta.clone()) {
                existing.updated_at = new_meta.updated_at;
            }
            existing
        } else {
            new_meta
        };

        // Ensure current_version is always correct (e.g. if we just installed a new version)
        final_meta.current_version = current_version.to_string();

        self.save_meta(&meta_path, &final_meta)?;

        Ok(())
    }
}

#[tracing::instrument(skip(runtime, target_dir, repo, release, client, extractor, cleanup_ctx))]
async fn ensure_installed<R: Runtime + 'static, E: Extractor>(
    runtime: &R,
    target_dir: &Path,
    repo: &GitHubRepo,
    release: &Release,
    client: &Client,
    extractor: &E,
    cleanup_ctx: Arc<Mutex<CleanupContext>>,
) -> Result<()> {
    if runtime.exists(target_dir) {
        info!(
            "Directory {:?} already exists. Skipping download and extraction.",
            target_dir
        );
        return Ok(());
    }

    debug!("Creating target directory: {:?}", target_dir);
    runtime
        .create_dir_all(target_dir)
        .with_context(|| format!("Failed to create target directory at {:?}", target_dir))?;

    // Register target_dir for cleanup on Ctrl-C
    {
        let mut ctx = cleanup_ctx.lock().unwrap();
        ctx.add(target_dir.to_path_buf());
    }

    let temp_dir = std::env::temp_dir();
    let temp_file_path = temp_dir.join(format!("{}-{}.tar.gz", repo.repo, release.tag_name));

    println!(" downloading {} {}", &repo, release.tag_name);
    if let Err(e) = download_file(runtime, &release.tarball_url, &temp_file_path, client).await {
        // Clean up target directory on download failure
        debug!("Download failed, cleaning up target directory: {:?}", target_dir);
        let _ = runtime.remove_dir_all(target_dir);
        return Err(e);
    }

    // Register temp file for cleanup (after download succeeds)
    {
        let mut ctx = cleanup_ctx.lock().unwrap();
        ctx.add(temp_file_path.clone());
    }

    println!("  installing {} {}", &repo, release.tag_name);
    if let Err(e) = extractor.extract_with_cleanup(runtime, &temp_file_path, target_dir, Arc::clone(&cleanup_ctx)) {
        // Clean up target directory and temp file on extraction failure
        debug!("Extraction failed, cleaning up target directory: {:?}", target_dir);
        let _ = runtime.remove_dir_all(target_dir);
        let _ = runtime.remove_file(&temp_file_path);
        return Err(e);
    }

    // Remove temp file from cleanup list and delete it
    {
        let mut ctx = cleanup_ctx.lock().unwrap();
        ctx.remove(&temp_file_path);
    }
    runtime
        .remove_file(&temp_file_path)
        .with_context(|| format!("Failed to clean up temporary file: {:?}", temp_file_path))?;

    // Installation succeeded, remove target_dir from cleanup list
    {
        let mut ctx = cleanup_ctx.lock().unwrap();
        ctx.remove(target_dir);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::MockExtractor;
    use crate::github::{MockGetReleases, RepoInfo};
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;
    use std::path::PathBuf;

    // Helper to configure simple home dir and user
    fn configure_runtime_basics(runtime: &mut MockRuntime) {
        #[cfg(not(windows))]
        runtime
            .expect_home_dir()
            .returning(|| Some(PathBuf::from("/home/user")));

        #[cfg(windows)]
        runtime
            .expect_home_dir()
            .returning(|| Some(PathBuf::from("C:\\Users\\user")));

        runtime
            .expect_env_var()
            .with(eq("USER"))
            .returning(|_| Ok("user".to_string()));

        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        runtime.expect_is_privileged().returning(|| false);
    }
    // Tests for get_target_dir and update_current_symlink are now in paths.rs and symlink.rs

    #[cfg(not(windows))]
    #[tokio::test]
    async fn test_install_happy_path() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // Meta check
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/home/user/.ghri/o/r/meta.json")))
            .returning(|_| false);

        // Save meta interactions
        runtime
            .expect_create_dir_all()
            .with(eq(PathBuf::from("/home/user/.ghri/o/r")))
            .returning(|_| Ok(()));
        runtime
            .expect_write()
            .with(
                eq(PathBuf::from("/home/user/.ghri/o/r/meta.json.tmp")),
                always(),
            )
            .returning(|_, _| Ok(()));
        runtime
            .expect_rename()
            .with(
                eq(PathBuf::from("/home/user/.ghri/o/r/meta.json.tmp")),
                eq(PathBuf::from("/home/user/.ghri/o/r/meta.json")),
            )
            .returning(|_, _| Ok(()));

        // Ensure installed interactions
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/home/user/.ghri/o/r/v1")))
            .returning(|_| false);
        runtime
            .expect_create_dir_all()
            .with(eq(PathBuf::from("/home/user/.ghri/o/r/v1")))
            .returning(|_| Ok(()));
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));
        runtime.expect_remove_file().returning(|_| Ok(()));

        // Symlink interactions
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/home/user/.ghri/o/r/current")))
            .returning(|_| false);
        runtime.expect_symlink().returning(|_, _| Ok(()));

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api.github.com".to_string());
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "now".into(),
            })
        });

        let download_url = format!("{}/tarball", url);
        github.expect_get_releases_at().return_once(move |_, _| {
            Ok(vec![Release {
                tag_name: "v1".into(),
                tarball_url: download_url,
                ..Default::default()
            }])
        });

        let _m = server
            .mock("GET", "/tarball")
            .with_status(200)
            .with_body("data")
            .create();

        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        let installer = Installer::new(runtime, github, Client::new(), extractor);
        installer.install(&repo, None, None).await.unwrap();
    }

    #[tokio::test]
    async fn test_get_or_fetch_meta_invalid_on_disk() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        #[cfg(not(windows))]
        let meta_path = PathBuf::from("/home/user/.ghri/o/r/meta.json");
        #[cfg(windows)]
        let meta_path = PathBuf::from("C:\\Users\\user\\.ghri\\o\\r\\meta.json");
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(|_| Ok("invalid json".into()));

        // Should fallback to fetch and then save
        runtime.expect_create_dir_all().returning(|_| Ok(()));
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api".to_string());
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "now".into(),
            })
        });
        github.expect_get_releases_at().returning(|_, _| Ok(vec![]));

        let installer = Installer::new(runtime, github, Client::new(), MockExtractor::new());
        let (meta, _) = installer.get_or_fetch_meta(&repo, None).await.unwrap();
        assert_eq!(meta.name, "o/r");
    }

    // test_find_all_packages is now in package/discovery.rs

    #[tokio::test]
    async fn test_ensure_installed_creates_dir_and_extracts() {
        let mut runtime = MockRuntime::new();
        let target = PathBuf::from("/target");
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag_name: "v1".into(),
            tarball_url: "http://mock/tar".into(),
            ..Default::default()
        };

        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| false);
        runtime
            .expect_create_dir_all()
            .with(eq(target.clone()))
            .returning(|_| Ok(()));
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));
        runtime.expect_remove_file().returning(|_| Ok(()));

        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/tar").with_status(200).create();
        let release_with_url = Release {
            tarball_url: format!("{}/tar", server.url()),
            ..release
        };

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        ensure_installed(
            &runtime,
            &target,
            &repo,
            &release_with_url,
            &Client::new(),
            &extractor,
            cleanup_ctx,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_update_all_happy_path() {
        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");
        configure_runtime_basics(&mut runtime);

        // Find one package
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|p| Ok(vec![p.join("o")]));
        runtime.expect_is_dir().returning(|_| true); // owner/repo are dirs
        runtime
            .expect_read_dir()
            .with(eq(root.join("o")))
            .returning(|p| Ok(vec![p.join("r")]));
        runtime
            .expect_exists()
            .with(eq(root.join("o/r/meta.json")))
            .returning(|_| true);

        // Load meta
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            updated_at: "old".into(),
            api_url: "api".into(),
            releases: vec![
                Release {
                    tag_name: "v1".into(),
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Update check calls fetch_meta -> github
        let mut github = MockGetReleases::new();
        github.expect_api_url().return_const("api".to_string());
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                updated_at: "new".into(),
                ..RepoInfo {
                    description: None,
                    homepage: None,
                    license: None,
                    updated_at: "".into(),
                }
            })
        });
        // Return a new version v2
        github.expect_get_releases_at().returning(|_, _| {
            Ok(vec![
                Release {
                    tag_name: "v2".into(),
                    published_at: Some("2024".into()),
                    ..Default::default()
                },
                Release {
                    tag_name: "v1".into(),
                    published_at: Some("2023".into()),
                    ..Default::default()
                },
            ])
        });

        // save_meta called twice (one for update metadata)
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let installer = Installer::new(runtime, github, Client::new(), MockExtractor::new());
        installer.update_all(None).await.unwrap();
    }

    // Meta tests are now in package/meta.rs
    // find_all_packages tests are now in package/discovery.rs

    #[tokio::test]
    async fn test_update_all_no_packages() {
        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");
        configure_runtime_basics(&mut runtime);

        // update_all calls default_install_root (mocked by basics) -> /home/user/.ghri
        // then find_all_packages checks exists and read_dir
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|_| Ok(vec![]));

        let github = MockGetReleases::new();
        let extractor = MockExtractor::new();
        let installer = Installer::new(runtime, github, Client::new(), extractor);

        let result = installer.update_all(None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_install_no_stable_release() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // Meta fetch
        runtime.expect_exists().returning(|_| false);
        runtime.expect_create_dir_all().returning(|_| Ok(()));
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api.github.com".to_string());
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "now".into(),
            })
        });
        github.expect_get_releases_at().returning(|_, _| {
            Ok(vec![Release {
                tag_name: "v1-rc".into(),
                prerelease: true,
                ..Default::default()
            }])
        });

        let installer = Installer::new(runtime, github, Client::new(), MockExtractor::new());
        let result = installer.install(&repo, None, None).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No stable release found")
        );
    }

    // default_install_root test is now in paths.rs
    
    #[tokio::test]
    async fn test_ensure_installed_cleanup_fail() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mut runtime = MockRuntime::new();

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag_name: "v1".into(),
            tarball_url: format!("{}/download", url),
            ..Default::default()
        };

        let _m = server.mock("GET", "/download").with_status(200).create();

        let target_dir = PathBuf::from("/tmp/target");

        runtime
            .expect_exists()
            .with(eq(target_dir.clone()))
            .returning(|_| false);
        runtime
            .expect_create_dir_all()
            .with(eq(target_dir.clone()))
            .returning(|_| Ok(()));
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));

        // Fail cleanup
        runtime
            .expect_remove_file()
            .returning(|_| Err(anyhow::anyhow!("fail")));

        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let result = ensure_installed(
            &runtime,
            &target_dir,
            &repo,
            &release,
            &Client::new(),
            &extractor,
            cleanup_ctx,
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to clean up temporary file")
        );
    }

    #[tokio::test]
    async fn test_ensure_installed_download_fail_cleans_up_target_dir() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mut runtime = MockRuntime::new();

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag_name: "v1".into(),
            tarball_url: format!("{}/download", url),
            ..Default::default()
        };

        // Download will fail with 404
        let _m = server.mock("GET", "/download").with_status(404).create();

        let target_dir = PathBuf::from("/tmp/target");

        runtime
            .expect_exists()
            .with(eq(target_dir.clone()))
            .returning(|_| false);
        runtime
            .expect_create_dir_all()
            .with(eq(target_dir.clone()))
            .returning(|_| Ok(()));

        // Should clean up target_dir on failure
        runtime
            .expect_remove_dir_all()
            .with(eq(target_dir.clone()))
            .times(1)
            .returning(|_| Ok(()));

        let extractor = MockExtractor::new();

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let result = ensure_installed(
            &runtime,
            &target_dir,
            &repo,
            &release,
            &Client::new(),
            &extractor,
            cleanup_ctx,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ensure_installed_extract_fail_cleans_up_target_dir() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let mut runtime = MockRuntime::new();

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag_name: "v1".into(),
            tarball_url: format!("{}/download", url),
            ..Default::default()
        };

        let _m = server.mock("GET", "/download").with_status(200).with_body("data").create();

        let target_dir = PathBuf::from("/tmp/target");

        runtime
            .expect_exists()
            .with(eq(target_dir.clone()))
            .returning(|_| false);
        runtime
            .expect_create_dir_all()
            .with(eq(target_dir.clone()))
            .returning(|_| Ok(()));
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));

        // Extraction fails
        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Err(anyhow::anyhow!("extraction failed")));

        // Should clean up target_dir and temp file on failure
        runtime
            .expect_remove_dir_all()
            .with(eq(target_dir.clone()))
            .times(1)
            .returning(|_| Ok(()));
        runtime
            .expect_remove_file()
            .times(1)
            .returning(|_| Ok(()));

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let result = ensure_installed(
            &runtime,
            &target_dir,
            &repo,
            &release,
            &Client::new(),
            &extractor,
            cleanup_ctx,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("extraction failed"));
    }

    // Meta tests (test_meta_releases_sorting, test_meta_merge_sorting, test_meta_sorting_fallback,
    // test_meta_conversions, test_meta_get_latest_stable_release*) are now in package/meta.rs
    // Symlink tests are now in symlink.rs

    #[tokio::test]
    async fn test_get_or_fetch_meta_fetch_fail() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);
        runtime.expect_exists().returning(|_| false);

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api".to_string());
        github
            .expect_get_repo_info_at()
            .returning(|_, _| Err(anyhow::anyhow!("fail")));

        let installer = Installer::new(runtime, github, Client::new(), MockExtractor::new());
        let result = installer.get_or_fetch_meta(&repo, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_invalid_repo_str() {
        let config = Config {
            runtime: MockRuntime::new(),
            github: MockGetReleases::new(),
            client: Client::new(),
            extractor: MockExtractor::new(),
            install_root: None,
        };
        let result = run("invalid", config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ensure_installed_already_exists() {
        let mut runtime = MockRuntime::new();
        let target = PathBuf::from("/target");
        runtime
            .expect_exists()
            .with(eq(target.clone()))
            .returning(|_| true);

        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let result = ensure_installed(
            &runtime,
            &target,
            &GitHubRepo {
                owner: "o".into(),
                repo: "r".into(),
            },
            &Release::default(),
            &Client::new(),
            &MockExtractor::new(),
            cleanup_ctx,
        )
        .await;
        assert!(result.is_ok());
    }

    // More Meta tests that are now in package/meta.rs

    // Symlink tests are now in symlink.rs

    #[tokio::test]
    async fn test_save_metadata_failure_warning() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // Using Sequence for write
        let mut seq = mockall::Sequence::new();

        runtime.expect_exists().returning(|_| false);
        runtime.expect_create_dir_all().returning(|_| Ok(()));

        runtime
            .expect_write()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(())); // Fetch save
        runtime
            .expect_write()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Err(anyhow::anyhow!("fail"))); // Update save

        runtime.expect_rename().returning(|_, _| Ok(()));

        // Install steps
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));
        runtime.expect_remove_file().returning(|_| Ok(()));
        runtime.expect_symlink().returning(|_, _| Ok(()));

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api".to_string());
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "".into(),
            })
        });

        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let tar_url = format!("{}/tar", url);
        let _m = server.mock("GET", "/tar").with_status(200).create();

        github.expect_get_releases_at().return_once(move |_, _| {
            Ok(vec![Release {
                tag_name: "v1".into(),
                tarball_url: tar_url,
                ..Default::default()
            }])
        });

        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        let installer = Installer::new(runtime, github, Client::new(), extractor);
        let result = installer.install(&repo, None, None).await;
        assert!(result.is_ok());
    }

    // default_install_root_privileged_mock test is now in paths.rs
    
    #[tokio::test]
    async fn test_get_or_fetch_meta_exists_interaction() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);
        runtime.expect_exists().returning(|_| true);
        runtime.expect_read_to_string().returning(|_| Ok(r#"{"name":"o/r","api_url":"https://api.github.com","repo_info_url":"","releases_url":"","description":null,"homepage":null,"license":null,"updated_at":"","current_version":"v1","releases":[]}"#.into()));

        let installer = Installer::new(
            runtime,
            MockGetReleases::new(),
            Client::new(),
            MockExtractor::new(),
        );
        let (meta, _) = installer.get_or_fetch_meta(&repo, None).await.unwrap();
        assert_eq!(meta.name, "o/r");
    }

    #[tokio::test]
    async fn test_install_uses_existing_meta() {
        // Similar to exists check but inside install flow
        let mut runtime = MockRuntime::new();
        let github = MockGetReleases::new();
        configure_runtime_basics(&mut runtime);

        // Exists
        runtime.expect_exists().returning(|_| true);
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            api_url: "api".into(),
            releases: vec![
                Release {
                    tag_name: "v1".into(),
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));
        runtime.expect_is_dir().returning(|_| false); // meta.json is not dir

        // Logic should skip fetch
        // and proceed to ensure_installed (mocked by basics)
        runtime.expect_exists().returning(|_| true); // target dir exists
        runtime.expect_is_symlink().returning(|_| true);
        runtime
            .expect_read_link()
            .returning(|_| Ok(PathBuf::from("v1")));
        runtime.expect_symlink().returning(|_, _| Ok(()));
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let installer = Installer::new(runtime, github, Client::new(), MockExtractor::new());
        installer
            .install(
                &GitHubRepo {
                    owner: "o".into(),
                    repo: "r".into(),
                },
                None,
                None,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);
        runtime.expect_exists().returning(|_| true); // cached
        runtime.expect_read_to_string().returning(|_| Ok(r#"{"name":"o/r","api_url":"","repo_info_url":"","releases_url":"","description":null,"homepage":null,"license":null,"updated_at":"","current_version":"v1","releases":[{"version":"v1","is_prerelease":false,"tarball_url":"","assets":[]}]}"#.into()));
        runtime.expect_is_dir().returning(|_| false);
        runtime.expect_is_symlink().returning(|_| true);
        runtime
            .expect_read_link()
            .returning(|_| Ok(PathBuf::from("v1")));
        runtime.expect_symlink().returning(|_, _| Ok(()));
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let config = Config {
            runtime,
            github: MockGetReleases::new(),
            client: Client::new(),
            extractor: MockExtractor::new(),
            install_root: None,
        };
        run("o/r", config).await.unwrap();
    }

    #[tokio::test]
    async fn test_update_function() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);
        runtime.expect_exists().returning(|_| false); // root empty

        update(runtime, None, None).await.unwrap();
    }

    // test_update_current_symlink_no_op_if_already_correct is now in symlink.rs

    #[tokio::test]
    async fn test_update_atomic_safety() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // Verify write -> rename sequence for metadata
        let mut seq = mockall::Sequence::new();
        runtime
            .expect_write()
            .withf(|p, _| p.to_string_lossy().ends_with(".tmp"))
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        runtime
            .expect_rename()
            .withf(|f, t| {
                f.to_string_lossy().ends_with(".tmp") && t.to_string_lossy().ends_with(".json")
            })
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));

        let meta = Meta {
            name: "o/r".into(),
            ..Default::default()
        };
        Installer::new(
            runtime,
            MockGetReleases::new(),
            Client::new(),
            MockExtractor::new(),
        )
        .save_meta(&PathBuf::from("meta.json"), &meta)
        .unwrap();
    }

    // test_update_timestamp_behavior is now in package/meta.rs

    #[test]
    fn test_determine_link_target_single_file() {
        let mut runtime = MockRuntime::new();
        let version_dir = PathBuf::from("/root/o/r/v1");

        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("/root/o/r/v1/tool")]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/o/r/v1/tool")))
            .returning(|_| false);

        let result = determine_link_target(&runtime, &version_dir).unwrap();
        assert_eq!(result, PathBuf::from("/root/o/r/v1/tool"));
    }

    #[test]
    fn test_determine_link_target_multiple_files() {
        let mut runtime = MockRuntime::new();
        let version_dir = PathBuf::from("/root/o/r/v1");

        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![
                PathBuf::from("/root/o/r/v1/tool"),
                PathBuf::from("/root/o/r/v1/README.md"),
            ]));

        let result = determine_link_target(&runtime, &version_dir).unwrap();
        assert_eq!(result, version_dir);
    }

    #[test]
    fn test_determine_link_target_single_directory() {
        let mut runtime = MockRuntime::new();
        let version_dir = PathBuf::from("/root/o/r/v1");

        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("/root/o/r/v1/subdir")]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/o/r/v1/subdir")))
            .returning(|_| true);

        let result = determine_link_target(&runtime, &version_dir).unwrap();
        assert_eq!(result, version_dir);
    }

    #[test]
    fn test_update_external_links_no_links() {
        let runtime = MockRuntime::new();
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            ..Default::default()
        };

        // Should return Ok without doing anything
        let result = update_external_links(
            &runtime,
            Path::new("/root/o/r"),
            Path::new("/root/o/r/v1"),
            &meta,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_external_links_updates_existing_symlink() {
        let mut runtime = MockRuntime::new();
        let linked_to = PathBuf::from("/usr/local/bin/tool");
        let version_dir = PathBuf::from("/root/o/r/v2");

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v2".into(),
            links: vec![LinkRule {
                dest: linked_to.clone(),
                path: None,
            }],
            ..Default::default()
        };

        // Check if linked_to exists
        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // Check if linked_to is symlink
        runtime
            .expect_is_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // Remove old symlink
        runtime
            .expect_remove_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| Ok(()));

        // Read version dir to determine link target
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("/root/o/r/v2/tool")]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/o/r/v2/tool")))
            .returning(|_| false);

        // Create new symlink
        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("/root/o/r/v2/tool")), eq(linked_to.clone()))
            .returning(|_, _| Ok(()));

        let result = update_external_links(
            &runtime,
            Path::new("/root/o/r"),
            &version_dir,
            &meta,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_dest_is_directory() {
        // When dest is an existing directory, the link should be created inside it
        // with the filename from the link target (single file case)
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let dest_dir = PathBuf::from("/usr/local/bin");
        let final_link = dest_dir.join("tool"); // /usr/local/bin/tool (filename from single file)

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Version dir exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/v1")))
            .returning(|_| true);

        // Read version dir - has single file
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner/repo/v1")))
            .returning(|_| Ok(vec![PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")))
            .returning(|_| false);

        // Check if dest exists and is a directory
        runtime
            .expect_exists()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_is_dir()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        // Check if final_link exists (it doesn't)
        runtime
            .expect_exists()
            .with(eq(final_link.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(final_link.clone()))
            .returning(|_| false);

        // Create symlink
        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")),
                eq(final_link.clone()),
            )
            .returning(|_, _| Ok(()));

        // Save meta
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = link(runtime, "owner/repo", dest_dir, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_unlink_removes_single_rule() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta with one link rule
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: link_dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Symlink exists
        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // Remove symlink
        runtime
            .expect_remove_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| Ok(()));

        // Save meta
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = unlink(runtime, "owner/repo", Some(link_dest), false, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_unlink_all_rules() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let link1 = PathBuf::from("/usr/local/bin/tool1");
        let link2 = PathBuf::from("/usr/local/bin/tool2");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta with two link rules
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![
                LinkRule { dest: link1.clone(), path: None },
                LinkRule { dest: link2.clone(), path: Some("tool2".into()) },
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Both symlinks exist
        runtime
            .expect_exists()
            .with(eq(link1.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(link1.clone()))
            .returning(|_| true);
        runtime
            .expect_remove_symlink()
            .with(eq(link1.clone()))
            .returning(|_| Ok(()));

        runtime
            .expect_exists()
            .with(eq(link2.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(link2.clone()))
            .returning(|_| true);
        runtime
            .expect_remove_symlink()
            .with(eq(link2.clone()))
            .returning(|_| Ok(()));

        // Save meta
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = unlink(runtime, "owner/repo", None, true, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_unlink_nonexistent_symlink_removes_rule() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: link_dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Symlink does not exist
        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| false);

        // Save meta (rule should still be removed)
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = unlink(runtime, "owner/repo", Some(link_dest), false, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_unlink_no_matching_rule_fails() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let existing_link = PathBuf::from("/usr/local/bin/tool");
        let nonexistent_link = PathBuf::from("/other/path/different-tool");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta with different link
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: existing_link.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        let result = unlink(runtime, "owner/repo", Some(nonexistent_link), false, Some(root));
        assert!(result.is_err());
    }

    #[test]
    fn test_unlink_requires_dest_or_all() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: PathBuf::from("/usr/local/bin/tool"),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Neither dest nor all specified
        let result = unlink(runtime, "owner/repo", None, false, Some(root));
        assert!(result.is_err());
    }

    #[test]
    fn test_unlink_empty_links() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta with no links
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        let result = unlink(runtime, "owner/repo", None, true, Some(root));
        assert!(result.is_ok()); // Should succeed with message "No link rules"
    }
}
