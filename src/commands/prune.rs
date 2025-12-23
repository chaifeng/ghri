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
}
