use anyhow::Result;
use log::debug;
use std::path::Path;

use crate::{
    github::RepoSpec,
    package::{LinkManager, Meta, PackageRepository},
    runtime::Runtime,
};

use super::config::{Config, ConfigOverrides};

/// Remove a package or specific version
#[tracing::instrument(skip(runtime, overrides))]
pub fn remove<R: Runtime>(
    runtime: R,
    repo_str: &str,
    force: bool,
    yes: bool,
    overrides: ConfigOverrides,
) -> Result<()> {
    debug!("Removing {} force={}", repo_str, force);
    let spec = repo_str.parse::<RepoSpec>()?;
    let config = Config::load(&runtime, overrides)?;
    debug!("Using install root: {:?}", config.install_root);

    let pkg_repo = PackageRepository::new(&runtime, config.install_root.clone());
    let package_dir = pkg_repo.package_dir(&spec.repo.owner, &spec.repo.repo);
    debug!("Package directory: {:?}", package_dir);

    if !pkg_repo.package_exists(&spec.repo.owner, &spec.repo.repo) {
        debug!("Package directory not found");
        anyhow::bail!("Package {} is not installed.", spec.repo);
    }

    // Load meta if exists
    let meta = pkg_repo.load(&spec.repo.owner, &spec.repo.repo)?;

    if let Some(ref version) = spec.version {
        // Remove specific version only
        debug!("Removing specific version: {}", version);

        // Show removal plan and confirm
        if !yes {
            show_version_removal_plan(&runtime, &pkg_repo, &spec.repo, version, meta.as_ref())?;
            if !runtime.confirm("Proceed with removal?")? {
                println!("Removal cancelled.");
                return Ok(());
            }
        }

        remove_version(
            &runtime,
            &pkg_repo,
            &spec.repo.owner,
            &spec.repo.repo,
            version,
            meta.as_ref(),
            force,
        )?;
    } else {
        // Remove entire package
        debug!("Removing entire package");

        // Show removal plan and confirm
        if !yes {
            show_package_removal_plan(&runtime, &spec.repo, &package_dir, meta.as_ref());
            if !runtime.confirm("Proceed with removal?")? {
                println!("Removal cancelled.");
                return Ok(());
            }
        }

        remove_package(
            &runtime,
            &pkg_repo,
            &spec.repo.owner,
            &spec.repo.repo,
            meta.as_ref(),
        )?;
    }

    Ok(())
}

fn show_package_removal_plan<R: Runtime>(
    runtime: &R,
    repo: &crate::github::GitHubRepo,
    package_dir: &Path,
    meta: Option<&Meta>,
) {
    println!();
    println!("=== Removal Plan ===");
    println!();
    println!("Package: {}", repo);
    println!();

    println!("Directories to remove:");
    println!("  [DEL] {}", package_dir.display());

    if let Some(meta) = meta {
        let link_manager = LinkManager::new(runtime);

        // Check regular links
        let (valid_links, invalid_links) = link_manager.check_links(&meta.links, package_dir);

        // Check versioned links
        let (valid_versioned, invalid_versioned) =
            link_manager.check_versioned_links(&meta.versioned_links, package_dir);

        // Combine valid links (only those that actually exist and point to package)
        let all_valid: Vec<_> = valid_links
            .iter()
            .chain(valid_versioned.iter())
            .filter(|l| l.status.is_valid())
            .collect();

        // Combine invalid links
        let all_invalid: Vec<_> = invalid_links
            .iter()
            .chain(invalid_versioned.iter())
            .chain(valid_links.iter().filter(|l| l.status.is_creatable()))
            .chain(valid_versioned.iter().filter(|l| l.status.is_creatable()))
            .collect();

        if !all_valid.is_empty() {
            println!();
            println!("Symlinks to remove:");
            for link in &all_valid {
                println!("  [DEL] {}", link.dest.display());
            }
        }

        if !all_invalid.is_empty() {
            println!();
            println!("Symlinks to skip (will not be removed):");
            for link in &all_invalid {
                println!(
                    "  [SKIP] {} ({})",
                    link.dest.display(),
                    link.status.reason()
                );
            }
        }
    }
    println!();
}

fn show_version_removal_plan<R: Runtime>(
    runtime: &R,
    pkg_repo: &PackageRepository<'_, R>,
    repo: &crate::github::GitHubRepo,
    version: &str,
    meta: Option<&Meta>,
) -> Result<()> {
    let package_dir = pkg_repo.package_dir(&repo.owner, &repo.repo);
    let version_dir = package_dir.join(version);

    // Check if this is the current version
    let is_current = pkg_repo.is_current_version(&repo.owner, &repo.repo, version);

    println!();
    println!("=== Removal Plan ===");
    println!();
    println!("Package: {}", repo);
    println!("Version: {}", version);
    if is_current {
        println!("  (This is the current version!)");
    }
    println!();

    println!("Directories to remove:");
    println!("  [DEL] {}", version_dir.display());

    if is_current {
        println!();
        println!("Symlinks to remove:");
        println!("  [DEL] {}/current", package_dir.display());
    }

    // Show links that will be removed
    if let Some(meta) = meta {
        let link_manager = LinkManager::new(runtime);

        // Check regular links pointing to this version
        let (valid_links, _) = link_manager.check_links(&meta.links, &version_dir);
        let regular_valid: Vec<_> = valid_links.iter().filter(|l| l.status.is_valid()).collect();

        // Check versioned links for this version
        let (valid_versioned, invalid_versioned) = link_manager.check_versioned_links_for_version(
            &meta.versioned_links,
            version,
            &version_dir,
        );
        let versioned_valid: Vec<_> = valid_versioned
            .iter()
            .filter(|l| l.status.is_valid())
            .collect();

        // Combine all valid links
        let all_valid: Vec<_> = regular_valid.iter().chain(versioned_valid.iter()).collect();

        if !all_valid.is_empty() {
            if !is_current {
                println!();
                println!("Symlinks to remove:");
            }
            for link in &all_valid {
                println!("  [DEL] {}", link.dest.display());
            }
        }

        // Show invalid versioned links (only versioned links matter here, regular links pointing elsewhere are fine)
        let all_invalid: Vec<_> = invalid_versioned
            .iter()
            .chain(valid_versioned.iter().filter(|l| l.status.is_creatable()))
            .collect();

        if !all_invalid.is_empty() {
            println!();
            println!("Symlinks to skip (will not be removed):");
            for link in &all_invalid {
                println!(
                    "  [SKIP] {} ({})",
                    link.dest.display(),
                    link.status.reason()
                );
            }
        }
    }

    println!();
    Ok(())
}

/// Remove a specific version of a package
pub(crate) fn remove_version<R: Runtime>(
    runtime: &R,
    pkg_repo: &PackageRepository<'_, R>,
    owner: &str,
    repo: &str,
    version: &str,
    meta: Option<&Meta>,
    force: bool,
) -> Result<()> {
    let package_dir = pkg_repo.package_dir(owner, repo);
    let version_dir = package_dir.join(version);
    debug!("Version directory: {:?}", version_dir);

    if !pkg_repo.is_version_installed(owner, repo, version) {
        anyhow::bail!("Version {} is not installed.", version);
    }

    // Check if this is the current version
    let is_current = pkg_repo.is_current_version(owner, repo, version);

    if is_current && !force {
        anyhow::bail!(
            "Version {} is the current version. Use --force to remove it anyway.",
            version
        );
    }

    // Remove links pointing to this version using LinkManager
    let link_manager = LinkManager::new(runtime);
    if let Some(meta) = meta {
        for rule in &meta.links {
            // For regular links, only remove if pointing to this specific version
            let _ = link_manager.remove_link_if_under(&rule.dest, &version_dir);
        }
        // Also remove versioned links for this version
        for link in &meta.versioned_links {
            if link.version == version {
                let _ = link_manager.remove_link_if_under(&link.dest, &version_dir);
            }
        }
    }

    // Update meta.json to remove versioned_links for this version
    if let Ok(Some(mut updated_meta)) = pkg_repo.load(owner, repo) {
        let original_len = updated_meta.versioned_links.len();
        updated_meta
            .versioned_links
            .retain(|l| l.version != version);
        if updated_meta.versioned_links.len() != original_len {
            debug!(
                "Removed {} versioned link(s) from meta.json",
                original_len - updated_meta.versioned_links.len()
            );
            pkg_repo.save(owner, repo, &updated_meta)?;
        }
    }

    // Remove the version directory
    debug!("Removing version directory {:?}", version_dir);
    pkg_repo.remove_version_dir(owner, repo, version)?;
    println!("Removed version {} from {}", version, package_dir.display());

    // If this was the current version, remove the current symlink
    if is_current {
        debug!("Removing current symlink");
        let current_link = pkg_repo.current_link(owner, repo);
        let _ = link_manager.remove_link(&current_link);
        println!("Warning: Removed current version symlink. No version is now active.");
    }

    Ok(())
}

/// Remove an entire package
fn remove_package<R: Runtime>(
    runtime: &R,
    pkg_repo: &PackageRepository<'_, R>,
    owner: &str,
    repo: &str,
    meta: Option<&Meta>,
) -> Result<()> {
    let package_dir = pkg_repo.package_dir(owner, repo);

    // Remove all external links first using LinkManager
    let link_manager = LinkManager::new(runtime);
    if let Some(meta) = meta {
        debug!("Removing {} link(s)", meta.links.len());
        for rule in &meta.links {
            let _ = link_manager.remove_link_if_under(&rule.dest, &package_dir);
        }
        // Also remove versioned links
        debug!("Removing {} versioned link(s)", meta.versioned_links.len());
        for link in &meta.versioned_links {
            let _ = link_manager.remove_link_if_under(&link.dest, &package_dir);
        }
    }

    // Remove the package directory (also cleans up empty owner directory)
    debug!("Removing package directory {:?}", package_dir);
    pkg_repo.remove_package_dir(owner, repo)?;

    println!("Removed package {}/{}", owner, repo);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::LinkRule;
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

    #[test]
    fn test_remove_package() {
        // Test removing an entire package (directory and all links)

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner"); // /home/user/.ghri/owner
        let package_dir = owner_dir.join("repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // --- 1. Check Package Exists ---

        // Directory exists: /home/user/.ghri/owner/repo -> true
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // --- 2. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json -> package with one link rule
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

        // --- 3. Remove External Symlinks (via LinkManager) ---

        // LinkManager.remove_link_if_under checks:
        // 1. is_symlink
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // 2. resolve_link -> points to package directory
        let resolved_target = package_dir.join("v1/tool");
        runtime
            .expect_resolve_link()
            .with(eq(link_dest.clone()))
            .returning(move |_| Ok(resolved_target.clone()));

        // 3. remove_symlink
        runtime
            .expect_remove_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| Ok(()));

        // --- 4. Remove Package Directory (via PackageRepository) ---

        // PackageRepository.remove_package_dir checks exists first
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // Remove /home/user/.ghri/owner/repo
        runtime
            .expect_remove_dir_all()
            .with(eq(package_dir.clone()))
            .returning(|_| Ok(()));

        // --- 5. Cleanup Empty Owner Directory ---

        // Check if /home/user/.ghri/owner exists and is empty -> yes
        runtime
            .expect_exists()
            .with(eq(owner_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(vec![])); // Empty!

        // Remove empty owner directory /home/user/.ghri/owner (uses remove_dir_all)
        runtime
            .expect_remove_dir_all()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(()));

        // --- Execute ---

        let result = remove(
            runtime,
            "owner/repo",
            false,
            true,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_remove_specific_version() {
        // Test removing a specific version (not the current version)

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner"); // /home/user/.ghri/owner
        let package_dir = owner_dir.join("repo"); // /home/user/.ghri/owner/repo
        let version_dir = package_dir.join("v1"); // /home/user/.ghri/owner/repo/v1
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let current_link = package_dir.join("current"); // /home/user/.ghri/owner/repo/current
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // --- 1. Check Package Exists ---

        // Directory exists: /home/user/.ghri/owner/repo -> true
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // --- 2. Load Metadata ---

        // File exists: meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json -> current version is v2 (not v1 being removed)
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v2".into(), // Different from version being removed
            links: vec![LinkRule {
                dest: link_dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 3. Check Version Exists (is_version_installed uses is_dir) ---

        // Directory exists: /home/user/.ghri/owner/repo/v1 -> true
        runtime
            .expect_is_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // --- 4. Check if v1 is Current Version ---

        // Read current symlink: -> v2 (not v1, so safe to remove)
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v2")));

        // --- 5. Check Link Target ---

        // Resolve link /usr/local/bin/tool -> /home/user/.ghri/owner/repo/v2/tool (points to v2, not v1)
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(link_dest.clone()))
            .returning(move |_| Ok(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")));

        // --- 6. Remove Version Directory (remove_version_dir checks exists then removes) ---

        // Check exists for remove_version_dir
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // Remove /home/user/.ghri/owner/repo/v1
        runtime
            .expect_remove_dir_all()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(()));

        // --- 7. Cleanup Check ---

        // Check owner directory still has content (not empty)
        runtime
            .expect_exists()
            .with(eq(owner_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("repo")])); // Not empty

        // --- Execute ---

        let result = remove(
            runtime,
            "owner/repo@v1",
            false,
            true,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_remove_current_version_requires_force() {
        // Test that removing current version requires --force flag

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let version_dir = package_dir.join("v1"); // /home/user/.ghri/owner/repo/v1
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let current_link = package_dir.join("current"); // /home/user/.ghri/owner/repo/current

        // --- 1. Check Package Exists ---

        // Directory exists: /home/user/.ghri/owner/repo -> true
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // --- 2. Load Metadata ---

        // File exists: meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json -> current version is v1 (same as being removed!)
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 3. Check Version Exists (is_version_installed uses is_dir) ---

        // Directory exists: /home/user/.ghri/owner/repo/v1 -> true
        runtime
            .expect_is_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // --- 4. Check if v1 is Current Version ---

        // Current symlink points to v1 (same as being removed!)
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1")));

        // --- Execute & Verify ---

        // Should fail without --force since v1 is the current version
        let result = remove(
            runtime,
            "owner/repo@v1",
            false,
            true,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--force"));
    }

    #[test]
    fn test_remove_nonexistent_package_fails() {
        // Test that remove fails when package is not installed

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo

        // --- 1. Check Package Exists ---

        // Directory exists: /home/user/.ghri/owner/repo -> false (not installed!)
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| false);

        // --- Execute & Verify ---

        let result = remove(
            runtime,
            "owner/repo",
            false,
            true,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_remove_package_link_points_to_wrong_location() {
        // Test that links pointing outside the package directory are not removed

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner"); // /home/user/.ghri/owner
        let package_dir = owner_dir.join("repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // --- 1. Check Package Exists ---

        // Directory exists: /home/user/.ghri/owner/repo -> true
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // --- 2. Load Metadata ---

        // File exists: meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json -> package with one link rule
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

        // --- 3. Try to Remove Link (Points to Different Package) ---

        // LinkManager.remove_link_if_under checks:
        // 1. is_symlink
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // 2. resolve_link -> points to different package (not removed)
        runtime
            .expect_resolve_link()
            .with(eq(link_dest.clone()))
            .returning(|_| Ok(PathBuf::from("/home/user/.ghri/other/package/tool")));

        // remove_symlink should NOT be called because target is not under package_dir

        // --- 4. Remove Package Directory (via PackageRepository) ---

        // PackageRepository.remove_package_dir checks exists first
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // Remove /home/user/.ghri/owner/repo
        runtime
            .expect_remove_dir_all()
            .with(eq(package_dir.clone()))
            .returning(|_| Ok(()));

        // --- 5. Cleanup Empty Owner Directory ---

        runtime
            .expect_exists()
            .with(eq(owner_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(vec![]));
        runtime
            .expect_remove_dir_all()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(()));

        // --- Execute ---

        let result = remove(
            runtime,
            "owner/repo",
            false,
            true,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_remove_package_link_not_symlink() {
        // Test that regular files (not symlinks) are not removed

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner"); // /home/user/.ghri/owner
        let package_dir = owner_dir.join("repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // --- 1. Check Package Exists ---

        // Directory exists: /home/user/.ghri/owner/repo -> true
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // --- 2. Load Metadata ---

        // File exists: meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json -> package with one link rule
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

        // --- 3. Try to Remove Link (Not a Symlink) ---

        // LinkManager.remove_link_if_under checks is_symlink first
        // /usr/local/bin/tool is not a symlink -> skipped
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| false);

        // remove_symlink should NOT be called because it's not a symlink

        // --- 4. Remove Package Directory (via PackageRepository) ---

        // PackageRepository.remove_package_dir checks exists first
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // Remove /home/user/.ghri/owner/repo
        runtime
            .expect_remove_dir_all()
            .with(eq(package_dir.clone()))
            .returning(|_| Ok(()));

        // --- 5. Cleanup Empty Owner Directory ---

        runtime
            .expect_exists()
            .with(eq(owner_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(vec![]));
        runtime
            .expect_remove_dir_all()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(()));

        // --- Execute ---

        let result = remove(
            runtime,
            "owner/repo",
            false,
            true,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_ok());
    }
}
