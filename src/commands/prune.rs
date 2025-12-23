use anyhow::Result;
use log::{debug, info};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::{
    github::RepoSpec,
    package::{Meta, find_all_packages},
    runtime::Runtime,
};

use super::paths::default_install_root;
use super::remove_version;

/// Prune unused versions, keeping only the current version
#[tracing::instrument(skip(runtime, install_root))]
pub fn prune<R: Runtime>(
    runtime: R,
    repos: Vec<String>,
    yes: bool,
    install_root: Option<PathBuf>,
) -> Result<()> {
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(&runtime)?,
    };
    debug!("Using install root: {:?}", root);

    if repos.is_empty() {
        // Prune all packages
        prune_all(&runtime, &root, yes)
    } else {
        // Prune specific packages
        for repo_str in &repos {
            let spec = repo_str.parse::<RepoSpec>()?;
            let package_dir = root.join(&spec.repo.owner).join(&spec.repo.repo);
            prune_package(&runtime, &package_dir, &spec.repo.to_string(), yes)?;
        }
        Ok(())
    }
}

fn prune_all<R: Runtime>(runtime: &R, root: &Path, yes: bool) -> Result<()> {
    let meta_files = find_all_packages(runtime, root)?;
    if meta_files.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    debug!("Found {} package(s)", meta_files.len());

    let mut total_pruned = 0;
    for meta_path in meta_files {
        if let Some(package_dir) = meta_path.parent() {
            let name = match Meta::load(runtime, &meta_path) {
                Ok(meta) => meta.name,
                Err(_) => {
                    // Try to derive name from path
                    let repo = package_dir.file_name().and_then(|s| s.to_str());
                    let owner = package_dir
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|s| s.to_str());
                    match (owner, repo) {
                        (Some(o), Some(r)) => format!("{}/{}", o, r),
                        _ => continue,
                    }
                }
            };
            let pruned = prune_package(runtime, package_dir, &name, yes)?;
            total_pruned += pruned;
        }
    }

    if total_pruned == 0 {
        println!("No unused versions to prune.");
    }

    Ok(())
}

/// Find versions that can be pruned (all versions except current)
fn find_versions_to_prune<R: Runtime>(
    runtime: &R,
    package_dir: &Path,
) -> Result<(Option<String>, Vec<String>)> {
    // Get current version from symlink
    let current_link = package_dir.join("current");
    let current_version = if runtime.is_symlink(&current_link) {
        runtime
            .read_link(&current_link)
            .ok()
            .and_then(|t| t.file_name().and_then(|s| s.to_str()).map(String::from))
    } else {
        None
    };

    let Some(ref current) = current_version else {
        return Ok((None, vec![]));
    };

    // Find all version directories (excluding meta.json and current)
    let entries = runtime.read_dir(package_dir)?;
    let versions_to_prune: Vec<String> = entries
        .iter()
        .filter_map(|entry| {
            let entry_name = entry.file_name()?.to_str()?.to_string();
            // Skip meta.json and current symlink
            if entry_name == "meta.json" || entry_name == "current" {
                return None;
            }
            if runtime.is_dir(entry) && entry_name != *current {
                Some(entry_name)
            } else {
                None
            }
        })
        .collect();

    Ok((current_version, versions_to_prune))
}

fn prune_package<R: Runtime>(
    runtime: &R,
    package_dir: &Path,
    name: &str,
    yes: bool,
) -> Result<usize> {
    debug!("Pruning package at {:?}", package_dir);

    if !runtime.exists(package_dir) {
        anyhow::bail!("Package {} is not installed.", name);
    }

    let (current_version, versions_to_prune) = find_versions_to_prune(runtime, package_dir)?;

    let Some(current_version) = current_version else {
        debug!("No current version symlink found for {}", name);
        return Ok(0);
    };

    if versions_to_prune.is_empty() {
        debug!("No versions to prune for {}", name);
        return Ok(0);
    }

    // Show prune plan
    println!();
    println!("Package: {}", name);
    println!("Current version: {}", current_version);
    println!("Versions to remove:");
    for version in &versions_to_prune {
        println!("  {}", version);
    }

    // Confirm
    if !yes {
        print!("Proceed with pruning? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let response = input.trim().to_lowercase();
        if response != "y" && response != "yes" {
            println!("Skipped.");
            return Ok(0);
        }
    }

    // Load meta for remove_version
    let meta_path = package_dir.join("meta.json");
    let meta = if runtime.exists(&meta_path) {
        Meta::load(runtime, &meta_path).ok()
    } else {
        None
    };

    // Remove each version using remove_version
    let mut pruned_count = 0;
    for version in &versions_to_prune {
        // force=true because we already confirmed, and these are not current versions
        remove_version(runtime, package_dir, version, meta.as_ref(), true)?;
        pruned_count += 1;
    }

    info!("Pruned {} version(s) from {}", pruned_count, name);
    Ok(pruned_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;
    use std::path::PathBuf;

    /// Test find_versions_to_prune identifies correct versions
    #[test]
    fn test_find_versions_to_prune_identifies_old_versions() {
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/pkg");
        let current_link = package_dir.join("current");

        // Current version is v2.0.0
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v2.0.0")));

        // Directory contains v1.0.0, v2.0.0, meta.json, current
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| {
                Ok(vec![
                    PathBuf::from("/pkg/v1.0.0"),
                    PathBuf::from("/pkg/v2.0.0"),
                    PathBuf::from("/pkg/meta.json"),
                    PathBuf::from("/pkg/current"),
                ])
            });

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/v2.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/meta.json")))
            .returning(|_| false);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/current")))
            .returning(|_| false);

        let (current, to_prune) = find_versions_to_prune(&runtime, &package_dir).unwrap();

        assert_eq!(current, Some("v2.0.0".to_string()));
        assert_eq!(to_prune, vec!["v1.0.0"]);
    }

    #[test]
    fn test_find_versions_to_prune_no_current_symlink() {
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/pkg");
        let current_link = package_dir.join("current");

        // No current symlink
        runtime
            .expect_is_symlink()
            .with(eq(current_link))
            .returning(|_| false);

        let (current, to_prune) = find_versions_to_prune(&runtime, &package_dir).unwrap();

        assert_eq!(current, None);
        assert!(to_prune.is_empty());
    }

    #[test]
    fn test_find_versions_to_prune_only_current_version() {
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/pkg");
        let current_link = package_dir.join("current");

        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        // Only current version installed
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| {
                Ok(vec![
                    PathBuf::from("/pkg/v1.0.0"),
                    PathBuf::from("/pkg/meta.json"),
                ])
            });

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/meta.json")))
            .returning(|_| false);

        let (current, to_prune) = find_versions_to_prune(&runtime, &package_dir).unwrap();

        assert_eq!(current, Some("v1.0.0".to_string()));
        assert!(to_prune.is_empty());
    }

    #[test]
    fn test_find_versions_to_prune_multiple_old_versions() {
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/pkg");
        let current_link = package_dir.join("current");

        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v3.0.0")));

        // Multiple old versions
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| {
                Ok(vec![
                    PathBuf::from("/pkg/v1.0.0"),
                    PathBuf::from("/pkg/v2.0.0"),
                    PathBuf::from("/pkg/v3.0.0"),
                ])
            });

        runtime.expect_is_dir().returning(|_| true);

        let (current, to_prune) = find_versions_to_prune(&runtime, &package_dir).unwrap();

        assert_eq!(current, Some("v3.0.0".to_string()));
        assert_eq!(to_prune.len(), 2);
        assert!(to_prune.contains(&"v1.0.0".to_string()));
        assert!(to_prune.contains(&"v2.0.0".to_string()));
    }

    #[test]
    fn test_prune_package_not_installed() {
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/pkg");

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| false);

        let result = prune_package(&runtime, &package_dir, "owner/repo", true);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_prune_package_no_current_symlink() {
        // Test that prune does nothing when no current symlink exists

        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/pkg");
        let current_link = package_dir.join("current");

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_is_symlink()
            .with(eq(current_link))
            .returning(|_| false);

        let result = prune_package(&runtime, &package_dir, "owner/repo", true);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_prune_package_no_versions_to_prune() {
        // Test that prune does nothing when only current version exists

        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/pkg");
        let current_link = package_dir.join("current");

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| {
                Ok(vec![
                    PathBuf::from("/pkg/v1.0.0"),
                    PathBuf::from("/pkg/meta.json"),
                ])
            });

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/meta.json")))
            .returning(|_| false);

        let result = prune_package(&runtime, &package_dir, "owner/repo", true);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_prune_package_removes_old_version() {
        // Test that prune removes old versions via remove_version

        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/pkg");
        let meta_path = package_dir.join("meta.json");
        let current_link = package_dir.join("current");
        let v1_dir = package_dir.join("v1.0.0");

        // 1. Package exists
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // 2. Current version is v2.0.0
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v2.0.0")));

        // 3. List versions
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| {
                Ok(vec![
                    PathBuf::from("/pkg/v1.0.0"),
                    PathBuf::from("/pkg/v2.0.0"),
                    PathBuf::from("/pkg/meta.json"),
                ])
            });

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/v2.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/meta.json")))
            .returning(|_| false);

        // 4. Load meta (for prune_package)
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v2.0.0".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // 5. remove_version checks version exists
        runtime
            .expect_exists()
            .with(eq(v1_dir.clone()))
            .returning(|_| true);

        // 6. remove_version removes directory
        runtime
            .expect_remove_dir_all()
            .with(eq(v1_dir))
            .returning(|_| Ok(()));

        let result = prune_package(&runtime, &package_dir, "owner/repo", true);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1);
    }

    #[test]
    fn test_prune_package_with_versioned_links() {
        // Test that prune removes versioned links for pruned versions

        let mut runtime = MockRuntime::new();
        use crate::package::VersionedLink;

        let package_dir = PathBuf::from("/pkg");
        let meta_path = package_dir.join("meta.json");
        let current_link = package_dir.join("current");
        let v1_dir = package_dir.join("v1.0.0");
        let link_dest = PathBuf::from("/usr/local/bin/tool-v1");
        let tmp_meta_path = meta_path.with_extension("json.tmp");

        // 1. Package exists
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // 2. Current version is v2.0.0
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v2.0.0")));

        // 3. List versions
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| {
                Ok(vec![
                    PathBuf::from("/pkg/v1.0.0"),
                    PathBuf::from("/pkg/v2.0.0"),
                    PathBuf::from("/pkg/meta.json"),
                ])
            });

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/v2.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/meta.json")))
            .returning(|_| false);

        // 4. Load meta with versioned link
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v2.0.0".into(),
            versioned_links: vec![VersionedLink {
                version: "v1.0.0".into(),
                dest: link_dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // 5. remove_version checks version exists
        runtime
            .expect_exists()
            .with(eq(v1_dir.clone()))
            .returning(|_| true);

        // 6. remove_version removes versioned link
        runtime
            .expect_remove_symlink_if_target_under()
            .with(eq(link_dest), eq(v1_dir.clone()), eq("versioned link"))
            .returning(|_, _, _| Ok(true));

        // 7. remove_version removes directory
        runtime
            .expect_remove_dir_all()
            .with(eq(v1_dir))
            .returning(|_| Ok(()));

        // 8. remove_version updates meta.json
        runtime
            .expect_write()
            .with(eq(tmp_meta_path.clone()), always())
            .returning(|_, _| Ok(()));
        runtime
            .expect_rename()
            .with(eq(tmp_meta_path), eq(meta_path))
            .returning(|_, _| Ok(()));

        let result = prune_package(&runtime, &package_dir, "owner/repo", true);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1);
    }

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
    fn test_prune_with_specific_repo() {
        // Test prune command with a specific repo argument

        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");
        let package_dir = root.join("owner/repo");
        let current_link = package_dir.join("current");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // No current symlink - nothing to prune
        runtime
            .expect_is_symlink()
            .with(eq(current_link))
            .returning(|_| false);

        let result = prune(runtime, vec!["owner/repo".to_string()], true, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_prune_all_no_packages() {
        // Test prune_all when no packages are installed

        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");

        // Root exists but is empty
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|_| Ok(vec![]));

        let result = prune_all(&runtime, &root, true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_prune_all_with_packages() {
        // Test prune_all with packages that have no versions to prune

        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");
        let owner_dir = root.join("owner");
        let package_dir = owner_dir.join("repo");
        let meta_path = package_dir.join("meta.json");
        let current_link = package_dir.join("current");

        // 1. find_all_packages
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(move |_| Ok(vec![owner_dir.clone()]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner")))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(PathBuf::from("/root/owner")))
            .returning(|_| Ok(vec![PathBuf::from("/root/owner/repo")]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner/repo")))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // 2. Load meta for name
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1.0.0".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // 3. prune_package - package exists
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // 4. find_versions_to_prune - no current symlink
        runtime
            .expect_is_symlink()
            .with(eq(current_link))
            .returning(|_| false);

        let result = prune_all(&runtime, &root, true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_prune_all_meta_load_error_fallback() {
        // Test prune_all falls back to deriving name from path when meta load fails

        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");
        let owner_dir = root.join("owner");
        let package_dir = owner_dir.join("repo");
        let meta_path = package_dir.join("meta.json");
        let current_link = package_dir.join("current");

        // 1. find_all_packages
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(move |_| Ok(vec![owner_dir.clone()]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner")))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(PathBuf::from("/root/owner")))
            .returning(|_| Ok(vec![PathBuf::from("/root/owner/repo")]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner/repo")))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // 2. Load meta fails - return invalid JSON
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(|_| Ok("invalid json".to_string()));

        // 3. prune_package - package exists
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // 4. find_versions_to_prune - no current symlink
        runtime
            .expect_is_symlink()
            .with(eq(current_link))
            .returning(|_| false);

        let result = prune_all(&runtime, &root, true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_prune_calls_prune_all_when_no_repos() {
        // Test that prune() calls prune_all() when repos is empty

        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");

        // Root exists but is empty - no packages
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|_| Ok(vec![]));

        let result = prune(runtime, vec![], true, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_prune_with_default_install_root() {
        // Test prune() uses default_install_root when install_root is None

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");

        // Root exists but is empty - no packages
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root))
            .returning(|_| Ok(vec![]));

        let result = prune(runtime, vec![], true, None); // None triggers default_install_root
        assert!(result.is_ok());
    }

    #[test]
    fn test_prune_package_without_meta() {
        // Test prune_package when meta.json doesn't exist

        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/pkg");
        let meta_path = package_dir.join("meta.json");
        let current_link = package_dir.join("current");
        let v1_dir = package_dir.join("v1.0.0");

        // 1. Package exists
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // 2. Current version is v2.0.0
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v2.0.0")));

        // 3. List versions
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| {
                Ok(vec![
                    PathBuf::from("/pkg/v1.0.0"),
                    PathBuf::from("/pkg/v2.0.0"),
                ])
            });

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/pkg/v2.0.0")))
            .returning(|_| true);

        // 4. meta.json doesn't exist
        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| false);

        // 5. remove_version checks version exists
        runtime
            .expect_exists()
            .with(eq(v1_dir.clone()))
            .returning(|_| true);

        // 6. remove_version removes directory
        runtime
            .expect_remove_dir_all()
            .with(eq(v1_dir))
            .returning(|_| Ok(()));

        let result = prune_package(&runtime, &package_dir, "owner/repo", true);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1);
    }
}
