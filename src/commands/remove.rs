use anyhow::Result;
use log::{debug, info};
use std::path::{Path, PathBuf};

use crate::{
    github::RepoSpec,
    package::Meta,
    runtime::Runtime,
};

use super::paths::default_install_root;

/// Remove a package or specific version
#[tracing::instrument(skip(runtime, install_root))]
pub fn remove<R: Runtime>(
    runtime: R,
    repo_str: &str,
    force: bool,
    install_root: Option<PathBuf>,
) -> Result<()> {
    debug!("Removing {} force={}", repo_str, force);
    let spec = repo_str.parse::<RepoSpec>()?;
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(&runtime)?,
    };
    debug!("Using install root: {:?}", root);

    let owner_dir = root.join(&spec.repo.owner);
    let package_dir = owner_dir.join(&spec.repo.repo);
    let meta_path = package_dir.join("meta.json");
    debug!("Package directory: {:?}", package_dir);

    if !runtime.exists(&package_dir) {
        debug!("Package directory not found");
        anyhow::bail!(
            "Package {} is not installed.",
            spec.repo
        );
    }

    // Load meta if exists
    let meta = if runtime.exists(&meta_path) {
        Some(Meta::load(&runtime, &meta_path)?)
    } else {
        None
    };

    if let Some(ref version) = spec.version {
        // Remove specific version only
        debug!("Removing specific version: {}", version);
        remove_version(&runtime, &package_dir, version, meta.as_ref(), force)?;
    } else {
        // Remove entire package
        debug!("Removing entire package");
        remove_package(&runtime, &package_dir, meta.as_ref(), force)?;
    }

    // Check if owner directory is empty and remove it
    if runtime.exists(&owner_dir) {
        let entries = runtime.read_dir(&owner_dir)?;
        if entries.is_empty() {
            debug!("Owner directory {:?} is empty, removing", owner_dir);
            runtime.remove_dir(&owner_dir)?;
            info!("Removed empty owner directory {:?}", owner_dir);
        }
    }

    Ok(())
}

/// Remove a specific version of a package
fn remove_version<R: Runtime>(
    runtime: &R,
    package_dir: &Path,
    version: &str,
    meta: Option<&Meta>,
    force: bool,
) -> Result<()> {
    let version_dir = package_dir.join(version);
    debug!("Version directory: {:?}", version_dir);

    if !runtime.exists(&version_dir) {
        anyhow::bail!("Version {} is not installed.", version);
    }

    // Check if this is the current version
    let current_link = package_dir.join("current");
    let is_current = if runtime.is_symlink(&current_link) {
        if let Ok(target) = runtime.read_link(&current_link) {
            let target_version = target.file_name().and_then(|s| s.to_str());
            target_version == Some(version)
        } else {
            false
        }
    } else {
        false
    };

    if is_current && !force {
        anyhow::bail!(
            "Version {} is the current version. Use --force to remove it anyway.",
            version
        );
    }

    // Remove links pointing to this version (but don't modify meta.json for regular links)
    if let Some(meta) = meta {
        for rule in &meta.links {
            // For regular links, only remove if pointing to this specific version
            if runtime.is_symlink(&rule.dest) {
                if let Ok(target) = runtime.read_link(&rule.dest) {
                    let resolved_target = if target.is_relative() {
                        rule.dest.parent().unwrap_or(Path::new(".")).join(&target)
                    } else {
                        target
                    };
                    
                    // Check if link points to this version
                    if resolved_target.starts_with(&version_dir) {
                        let _ = runtime.remove_symlink_if_target_under(
                            &rule.dest,
                            &version_dir,
                            "link",
                        );
                    }
                }
            }
        }
        // Also remove versioned links for this version
        for link in &meta.versioned_links {
            if link.version == version {
                let _ = runtime.remove_symlink_if_target_under(
                    &link.dest,
                    &version_dir,
                    "versioned link",
                );
            }
        }
    }

    // Update meta.json to remove versioned_links for this version
    let meta_path = package_dir.join("meta.json");
    if runtime.exists(&meta_path) {
        if let Ok(mut meta) = Meta::load(runtime, &meta_path) {
            let original_len = meta.versioned_links.len();
            meta.versioned_links.retain(|l| l.version != version);
            if meta.versioned_links.len() != original_len {
                debug!("Removed {} versioned link(s) from meta.json", original_len - meta.versioned_links.len());
                let json = serde_json::to_string_pretty(&meta)?;
                let tmp_path = meta_path.with_extension("json.tmp");
                runtime.write(&tmp_path, json.as_bytes())?;
                runtime.rename(&tmp_path, &meta_path)?;
            }
        }
    }

    // Remove the version directory
    debug!("Removing version directory {:?}", version_dir);
    runtime.remove_dir_all(&version_dir)?;
    println!("Removed version {} from {}", version, package_dir.display());

    // If this was the current version, remove the current symlink
    if is_current {
        debug!("Removing current symlink");
        runtime.remove_symlink(&current_link)?;
        println!("Warning: Removed current version symlink. No version is now active.");
    }

    Ok(())
}

/// Remove an entire package
fn remove_package<R: Runtime>(
    runtime: &R,
    package_dir: &Path,
    meta: Option<&Meta>,
    _force: bool,
) -> Result<()> {
    // Remove all external links first
    if let Some(meta) = meta {
        debug!("Removing {} link(s)", meta.links.len());
        for rule in &meta.links {
            let _ = runtime.remove_symlink_if_target_under(
                &rule.dest,
                package_dir,
                "link",
            );
        }
        // Also remove versioned links
        debug!("Removing {} versioned link(s)", meta.versioned_links.len());
        for link in &meta.versioned_links {
            let _ = runtime.remove_symlink_if_target_under(
                &link.dest,
                package_dir,
                "versioned link",
            );
        }
    }

    // Remove the package directory
    debug!("Removing package directory {:?}", package_dir);
    runtime.remove_dir_all(package_dir)?;

    let package_name = package_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let owner_name = package_dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    println!("Removed package {}/{}", owner_name, package_name);

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
        let owner_dir = root.join("owner");                      // /home/user/.ghri/owner
        let package_dir = owner_dir.join("repo");                // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json");           // /home/user/.ghri/owner/repo/meta.json
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

        // --- 3. Remove External Symlinks ---

        // Remove symlink /usr/local/bin/tool if it points to package directory
        runtime
            .expect_remove_symlink_if_target_under()
            .with(eq(link_dest.clone()), eq(package_dir.clone()), eq("link"))
            .returning(|_, _, _| Ok(true));

        // --- 4. Remove Package Directory ---

        // Remove /home/user/.ghri/owner/repo
        runtime
            .expect_remove_dir_all()
            .with(eq(package_dir.clone()))
            .returning(|_| Ok(()));

        // --- 5. Cleanup Empty Owner Directory ---

        // Check if /home/user/.ghri/owner is empty -> yes
        runtime
            .expect_exists()
            .with(eq(owner_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(vec![]));  // Empty!

        // Remove empty owner directory /home/user/.ghri/owner
        runtime
            .expect_remove_dir()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(()));

        // --- Execute ---

        let result = remove(runtime, "owner/repo", false, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_remove_specific_version() {
        // Test removing a specific version (not the current version)

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner");                       // /home/user/.ghri/owner
        let package_dir = owner_dir.join("repo");                 // /home/user/.ghri/owner/repo
        let version_dir = package_dir.join("v1");                 // /home/user/.ghri/owner/repo/v1
        let meta_path = package_dir.join("meta.json");            // /home/user/.ghri/owner/repo/meta.json
        let current_link = package_dir.join("current");           // /home/user/.ghri/owner/repo/current
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
            current_version: "v2".into(),  // Different from version being removed
            links: vec![LinkRule {
                dest: link_dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 3. Check Version Exists ---

        // Directory exists: /home/user/.ghri/owner/repo/v1 -> true
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // --- 4. Check if v1 is Current Version ---

        // Read current symlink: -> v2 (not v1, so safe to remove)
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v2")));

        // --- 5. Check Link Target ---

        // Link /usr/local/bin/tool points to v2, not v1
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(link_dest.clone()))
            .returning(move |_| Ok(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")));

        // --- 6. Remove Version Directory ---

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
            .returning(|_| Ok(vec![PathBuf::from("repo")]));  // Not empty

        // --- Execute ---

        let result = remove(runtime, "owner/repo@v1", false, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_remove_current_version_requires_force() {
        // Test that removing current version requires --force flag

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");                // /home/user/.ghri/owner/repo
        let version_dir = package_dir.join("v1");                 // /home/user/.ghri/owner/repo/v1
        let meta_path = package_dir.join("meta.json");            // /home/user/.ghri/owner/repo/meta.json
        let current_link = package_dir.join("current");           // /home/user/.ghri/owner/repo/current

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

        // --- 3. Check Version Exists ---

        // Directory exists: /home/user/.ghri/owner/repo/v1 -> true
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // --- 4. Check if v1 is Current Version ---

        // Current symlink points to v1 (same as being removed!)
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1")));

        // --- Execute & Verify ---

        // Should fail without --force since v1 is the current version
        let result = remove(runtime, "owner/repo@v1", false, Some(root));
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
        let package_dir = root.join("owner/repo");                // /home/user/.ghri/owner/repo

        // --- 1. Check Package Exists ---

        // Directory exists: /home/user/.ghri/owner/repo -> false (not installed!)
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| false);

        // --- Execute & Verify ---

        let result = remove(runtime, "owner/repo", false, Some(root));
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
        let owner_dir = root.join("owner");                       // /home/user/.ghri/owner
        let package_dir = owner_dir.join("repo");                 // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json");            // /home/user/.ghri/owner/repo/meta.json
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

        // Link /usr/local/bin/tool points to different package -> Ok(false) (skipped)
        runtime
            .expect_remove_symlink_if_target_under()
            .with(eq(link_dest.clone()), eq(package_dir.clone()), eq("link"))
            .returning(|_, _, _| Ok(false));  // Skipped - wrong target

        // --- 4. Remove Package Directory ---

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
            .expect_remove_dir()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(()));

        // --- Execute ---

        let result = remove(runtime, "owner/repo", false, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_remove_package_link_not_symlink() {
        // Test that regular files (not symlinks) are not removed

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner");                       // /home/user/.ghri/owner
        let package_dir = owner_dir.join("repo");                 // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json");            // /home/user/.ghri/owner/repo/meta.json
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

        // /usr/local/bin/tool is not a symlink -> Ok(false) (skipped)
        runtime
            .expect_remove_symlink_if_target_under()
            .with(eq(link_dest.clone()), eq(package_dir.clone()), eq("link"))
            .returning(|_, _, _| Ok(false));  // Skipped - not a symlink

        // --- 4. Remove Package Directory ---

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
            .expect_remove_dir()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(()));

        // --- Execute ---

        let result = remove(runtime, "owner/repo", false, Some(root));
        assert!(result.is_ok());
    }
}
