use anyhow::Result;
use log::{debug, info};
use std::path::Path;

use crate::{package::PackageRepository, runtime::Runtime};

use super::config::Config;
use super::install::RepoSpec;
use super::remove_version;

/// Prune unused versions, keeping only the current version
#[tracing::instrument(skip(runtime, config))]
pub fn prune<R: Runtime>(runtime: R, repos: Vec<String>, yes: bool, config: Config) -> Result<()> {
    debug!("Using install root: {:?}", config.install_root);

    if repos.is_empty() {
        // Prune all packages
        prune_all(&runtime, &config.install_root, yes)
    } else {
        // Prune specific packages
        let pkg_repo = PackageRepository::new(&runtime, config.install_root.clone());
        for repo_str in &repos {
            let spec = repo_str.parse::<RepoSpec>()?;
            prune_package(
                &runtime,
                &pkg_repo,
                &spec.repo.owner,
                &spec.repo.repo,
                &spec.repo.to_string(),
                yes,
            )?;
        }
        Ok(())
    }
}

fn prune_all<R: Runtime>(runtime: &R, root: &Path, yes: bool) -> Result<()> {
    let pkg_repo = PackageRepository::new(runtime, root.to_path_buf());
    let packages = pkg_repo.find_all_with_meta()?;

    if packages.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    debug!("Found {} package(s)", packages.len());

    let mut total_pruned = 0;
    for (meta_path, meta) in packages {
        if let Some(package_dir) = meta_path.parent() {
            // Extract owner/repo from path
            let repo = package_dir.file_name().and_then(|s| s.to_str());
            let owner = package_dir
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str());
            if let (Some(owner), Some(repo)) = (owner, repo) {
                let name = meta.name.clone();
                let pruned = prune_package(runtime, &pkg_repo, owner, repo, &name, yes)?;
                total_pruned += pruned;
            }
        }
    }

    if total_pruned == 0 {
        println!("No unused versions to prune.");
    }

    Ok(())
}

/// Find versions that can be pruned (all versions except current)
fn find_versions_to_prune<R: Runtime>(
    pkg_repo: &PackageRepository<'_, R>,
    owner: &str,
    repo: &str,
) -> Result<(Option<String>, Vec<String>)> {
    // Get current version from symlink
    let current_version = pkg_repo.current_version(owner, repo);

    let Some(ref current) = current_version else {
        return Ok((None, vec![]));
    };

    // Get all installed versions and filter out the current one
    let versions_to_prune: Vec<String> = pkg_repo
        .installed_versions(owner, repo)?
        .into_iter()
        .filter(|v| v != current)
        .collect();

    Ok((current_version, versions_to_prune))
}

fn prune_package<R: Runtime>(
    runtime: &R,
    pkg_repo: &PackageRepository<'_, R>,
    owner: &str,
    repo: &str,
    name: &str,
    yes: bool,
) -> Result<usize> {
    let package_dir = pkg_repo.package_dir(owner, repo);
    debug!("Pruning package at {:?}", package_dir);

    if !pkg_repo.package_exists(owner, repo) {
        anyhow::bail!("Package {} is not installed.", name);
    }

    let (current_version, versions_to_prune) = find_versions_to_prune(pkg_repo, owner, repo)?;

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
    if !yes && !runtime.confirm("Proceed with pruning?")? {
        println!("Skipped.");
        return Ok(0);
    }

    // Load meta for remove_version
    let meta = pkg_repo.load(owner, repo)?;

    // Remove each version using remove_version
    let mut pruned_count = 0;
    for version in &versions_to_prune {
        // force=true because we already confirmed, and these are not current versions
        remove_version(runtime, pkg_repo, owner, repo, version, meta.as_ref(), true)?;
        pruned_count += 1;
    }

    info!("Pruned {} version(s) from {}", pruned_count, name);
    Ok(pruned_count)
}

/// Prune old versions from a package directory (no confirmation, for use after install/upgrade)
pub fn prune_package_dir<R: Runtime>(
    runtime: &R,
    install_root: &Path,
    owner: &str,
    repo: &str,
    name: &str,
) -> Result<()> {
    let pkg_repo = PackageRepository::new(runtime, install_root.to_path_buf());

    if !pkg_repo.package_exists(owner, repo) {
        return Ok(()); // Package not installed, nothing to prune
    }

    let (current_version, versions_to_prune) = find_versions_to_prune(&pkg_repo, owner, repo)?;

    let Some(current_version) = current_version else {
        return Ok(()); // No current version, nothing to prune
    };

    if versions_to_prune.is_empty() {
        return Ok(()); // Nothing to prune
    }

    // Load meta for remove_version
    let meta = pkg_repo.load(owner, repo)?;

    // Remove each version
    println!(
        "Pruning {} old version(s) from {}...",
        versions_to_prune.len(),
        name
    );
    for version in &versions_to_prune {
        if version == &current_version {
            continue; // Safety check
        }
        remove_version(
            runtime,
            &pkg_repo,
            owner,
            repo,
            version,
            meta.as_ref(),
            true,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Meta;
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;
    use std::path::PathBuf;

    /// Test find_versions_to_prune identifies correct versions
    #[test]
    fn test_find_versions_to_prune_identifies_old_versions() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");
        let current_link = PathBuf::from("/root/owner/repo/current");
        let package_dir = PathBuf::from("/root/owner/repo");

        // Current version is v2.0.0
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v2.0.0")));

        // Directory contains v1.0.0, v2.0.0, meta.json, current
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| {
                Ok(vec![
                    PathBuf::from("/root/owner/repo/v1.0.0"),
                    PathBuf::from("/root/owner/repo/v2.0.0"),
                    PathBuf::from("/root/owner/repo/meta.json"),
                    PathBuf::from("/root/owner/repo/current"),
                ])
            });

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner/repo/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner/repo/v2.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner/repo/meta.json")))
            .returning(|_| false);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner/repo/current")))
            .returning(|_| false);

        let pkg_repo = PackageRepository::new(&runtime, root);
        let (current, to_prune) = find_versions_to_prune(&pkg_repo, "owner", "repo").unwrap();

        assert_eq!(current, Some("v2.0.0".to_string()));
        assert_eq!(to_prune, vec!["v1.0.0"]);
    }

    #[test]
    fn test_find_versions_to_prune_no_current_symlink() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");
        let current_link = PathBuf::from("/root/owner/repo/current");

        // No current symlink
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into())
            });

        let pkg_repo = PackageRepository::new(&runtime, root);
        let (current, to_prune) = find_versions_to_prune(&pkg_repo, "owner", "repo").unwrap();

        assert_eq!(current, None);
        assert!(to_prune.is_empty());
    }

    #[test]
    fn test_find_versions_to_prune_only_current_version() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");
        let current_link = PathBuf::from("/root/owner/repo/current");
        let package_dir = PathBuf::from("/root/owner/repo");

        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        // Only current version installed
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| {
                Ok(vec![
                    PathBuf::from("/root/owner/repo/v1.0.0"),
                    PathBuf::from("/root/owner/repo/meta.json"),
                ])
            });

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner/repo/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner/repo/meta.json")))
            .returning(|_| false);

        let pkg_repo = PackageRepository::new(&runtime, root);
        let (current, to_prune) = find_versions_to_prune(&pkg_repo, "owner", "repo").unwrap();

        assert_eq!(current, Some("v1.0.0".to_string()));
        assert!(to_prune.is_empty());
    }

    #[test]
    fn test_find_versions_to_prune_multiple_old_versions() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");
        let current_link = PathBuf::from("/root/owner/repo/current");
        let package_dir = PathBuf::from("/root/owner/repo");

        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v3.0.0")));

        // Multiple old versions
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(|_| {
                Ok(vec![
                    PathBuf::from("/root/owner/repo/v1.0.0"),
                    PathBuf::from("/root/owner/repo/v2.0.0"),
                    PathBuf::from("/root/owner/repo/v3.0.0"),
                ])
            });

        runtime.expect_is_dir().returning(|_| true);

        let pkg_repo = PackageRepository::new(&runtime, root);
        let (current, to_prune) = find_versions_to_prune(&pkg_repo, "owner", "repo").unwrap();

        assert_eq!(current, Some("v3.0.0".to_string()));
        assert_eq!(to_prune.len(), 2);
        assert!(to_prune.contains(&"v1.0.0".to_string()));
        assert!(to_prune.contains(&"v2.0.0".to_string()));
    }

    #[test]
    fn test_prune_package_not_installed() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");
        let package_dir = PathBuf::from("/root/owner/repo");

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| false);

        let pkg_repo = PackageRepository::new(&runtime, root);
        let result = prune_package(&runtime, &pkg_repo, "owner", "repo", "owner/repo", true);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_prune_package_no_current_symlink() {
        // Test that prune does nothing when no current symlink exists

        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");
        let package_dir = PathBuf::from("/root/owner/repo");
        let current_link = package_dir.join("current");

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into())
            });

        let pkg_repo = PackageRepository::new(&runtime, root);
        let result = prune_package(&runtime, &pkg_repo, "owner", "repo", "owner/repo", true);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_prune_package_no_versions_to_prune() {
        // Test that prune does nothing when only current version exists

        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");
        let package_dir = PathBuf::from("/root/owner/repo");
        let current_link = package_dir.join("current");

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
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
                    PathBuf::from("/root/owner/repo/v1.0.0"),
                    PathBuf::from("/root/owner/repo/meta.json"),
                ])
            });

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner/repo/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/owner/repo/meta.json")))
            .returning(|_| false);

        let pkg_repo = PackageRepository::new(&runtime, root);
        let result = prune_package(&runtime, &pkg_repo, "owner", "repo", "owner/repo", true);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_prune_with_specific_repo() {
        // Test prune command with a specific repo argument

        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");
        let package_dir = root.join("owner/repo");
        let current_link = package_dir.join("current");

        // Config::load needs GITHUB_TOKEN
        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        // Package exists
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // No current symlink - nothing to prune
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into())
            });

        let result = prune(
            runtime,
            vec!["owner/repo".to_string()],
            true,
            Config::for_test(root),
        );
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
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into())
            });

        let result = prune_all(&runtime, &root, true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_prune_calls_prune_all_when_no_repos() {
        // Test that prune() calls prune_all() when repos is empty

        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/root");

        // Config::load needs GITHUB_TOKEN
        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        // Root exists but is empty - no packages
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|_| Ok(vec![]));

        let result = prune(runtime, vec![], true, Config::for_test(root));
        assert!(result.is_ok());
    }
}
