use anyhow::Result;
use log::debug;

use crate::{package::PackageRepository, runtime::Runtime};

use super::config::Config;
use super::install::RepoSpec;
use super::{print_links, print_versioned_links};

/// Show detailed information about a package
#[tracing::instrument(skip(runtime, config))]
pub fn show<R: Runtime>(runtime: R, repo_str: &str, config: Config) -> Result<()> {
    debug!("Showing info for {}", repo_str);
    let spec = repo_str.parse::<RepoSpec>()?;
    debug!("Using install root: {:?}", config.install_root);

    let pkg_repo = PackageRepository::new(&runtime, config.install_root);
    let package_dir = pkg_repo.package_dir(&spec.repo.owner, &spec.repo.repo);
    debug!("Package directory: {:?}", package_dir);

    if !pkg_repo.package_exists(&spec.repo.owner, &spec.repo.repo) {
        anyhow::bail!("Package {} is not installed.", spec.repo);
    }

    // Load meta (may be None if meta.json doesn't exist)
    let meta = pkg_repo.load(&spec.repo.owner, &spec.repo.repo)?;

    // Package name
    println!("Package: {}", spec.repo);
    println!("Directory: {}", package_dir.display());

    // Current version
    let current_link = pkg_repo.current_link(&spec.repo.owner, &spec.repo.repo);
    let current_version = pkg_repo.current_version(&spec.repo.owner, &spec.repo.repo);

    if let Some(ref version) = current_version {
        println!("Current version: {}", version);
    } else if let Some(ref meta) = meta
        && !meta.current_version.is_empty()
    {
        println!("Current version: {}", meta.current_version);
    }

    // List installed versions
    println!("\nInstalled versions:");
    let mut versions = pkg_repo.installed_versions(&spec.repo.owner, &spec.repo.repo)?;
    versions.sort();

    for version in &versions {
        if Some(version) == current_version.as_ref() {
            println!("  {} (current)", version);
        } else {
            println!("  {}", version);
        }
    }

    if versions.is_empty() {
        println!("  (none)");
    }

    // Show meta info
    if let Some(ref meta) = meta {
        // Description
        if let Some(ref desc) = meta.description {
            println!("\nDescription: {}", desc);
        }

        // Homepage
        if let Some(ref homepage) = meta.homepage {
            println!("Homepage: {}", homepage);
        }

        // License
        if let Some(ref license) = meta.license {
            println!("License: {}", license);
        }

        // Last updated
        if !meta.updated_at.is_empty() {
            println!("Last updated: {}", meta.updated_at);
        }

        // Available versions (from releases)
        // meta.releases is already sorted by published_at (newest first)
        if !meta.releases.is_empty() {
            println!("\nAvailable versions (from cache):");
            for (i, release) in meta.releases.iter().enumerate() {
                if i >= 10 {
                    println!("  ... and {} more", meta.releases.len() - 10);
                    break;
                }
                let installed = versions.contains(&release.version);
                if installed {
                    println!("  {} (installed)", release.version);
                } else {
                    println!("  {}", release.version);
                }
            }
        }

        // Links
        if !meta.links.is_empty() {
            println!();
            print_links(&runtime, &meta.links, &current_link, Some("Links:"));
        }

        // Versioned links (historical)
        if !meta.versioned_links.is_empty() {
            println!();
            print_versioned_links(
                &runtime,
                &meta.versioned_links,
                &package_dir,
                Some("Versioned links (historical):"),
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{LinkRule, Meta};
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
    fn test_show_package_info() {
        // Test showing detailed package information including description, homepage, license, links

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let current_link = package_dir.join("current"); // /home/user/.ghri/owner/repo/current
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // --- 1. Check Package Exists (package_exists calls exists on package_dir) ---

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // --- 2. Load Metadata (load calls exists on meta_path, then read_to_string) ---

        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            description: Some("Test package".into()),
            homepage: Some("https://example.com".into()),
            license: Some("MIT".into()),
            updated_at: "2023-01-01T00:00:00Z".into(),
            current_version: "v1.0.0".into(),
            links: vec![LinkRule {
                dest: link_dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 3. Get Current Version (current_version calls read_link) ---

        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        // --- 4. List Installed Versions (installed_versions calls exists, read_dir, is_dir) ---

        // exists on package_dir (for installed_versions)
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri/owner/repo -> [v1.0.0, meta.json, current]
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(move |_| {
                Ok(vec![
                    PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0"),
                    PathBuf::from("/home/user/.ghri/owner/repo/meta.json"),
                    PathBuf::from("/home/user/.ghri/owner/repo/current"),
                ])
            });

        // Is dir: /home/user/.ghri/owner/repo/v1.0.0 -> true (version directory)
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0")))
            .returning(|_| true);

        // --- 5. Check Link Status (for printing links) ---

        // File exists: /usr/local/bin/tool -> true
        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // Is symlink: /usr/local/bin/tool -> true
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // Resolve link: /usr/local/bin/tool -> /home/user/.ghri/owner/repo/current/bin/tool
        runtime
            .expect_resolve_link()
            .with(eq(link_dest.clone()))
            .returning(|_| {
                Ok(PathBuf::from(
                    "/home/user/.ghri/owner/repo/current/bin/tool",
                ))
            });

        // Canonicalize paths for link status check
        runtime
            .expect_canonicalize()
            .returning(|p| Ok(p.to_path_buf()));

        // --- Execute ---

        let result = show(runtime, "owner/repo", Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_nonexistent_package_fails() {
        // Test that show fails when package is not installed

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

        let result = show(runtime, "owner/repo", Config::for_test(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_show_without_meta() {
        // Test showing package info when meta.json doesn't exist

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let current_link = package_dir.join("current"); // /home/user/.ghri/owner/repo/current

        // --- 1. Check Package Exists (package_exists) ---

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // --- 2. Load Metadata (load checks meta_path exists -> false) ---

        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| false);

        // --- 3. Get Current Version (current_version calls read_link) ---

        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        // --- 4. List Installed Versions (installed_versions) ---

        // exists on package_dir
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri/owner/repo -> [v1.0.0]
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(move |_| Ok(vec![PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0")]));

        // Is dir: /home/user/.ghri/owner/repo/v1.0.0 -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0")))
            .returning(|_| true);

        // --- Execute ---

        let result = show(runtime, "owner/repo", Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_with_releases() {
        // Test showing package info with available releases from cache

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let current_link = package_dir.join("current"); // /home/user/.ghri/owner/repo/current

        // --- 1. Check Package Exists (package_exists) ---

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // --- 2. Load Metadata ---

        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json -> package with cached releases
        use crate::package::MetaRelease;
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1.0.0".into(),
            releases: vec![
                MetaRelease {
                    version: "v1.1.0".into(), // Newer version available
                    published_at: Some("2023-02-01T00:00:00Z".into()),
                    ..Default::default()
                },
                MetaRelease {
                    version: "v1.0.0".into(), // Current installed version
                    published_at: Some("2023-01-01T00:00:00Z".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 3. Get Current Version (current_version calls read_link) ---

        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        // --- 4. List Installed Versions (installed_versions) ---

        // exists on package_dir
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // Read dir -> [v1.0.0, meta.json, current]
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(move |_| {
                Ok(vec![
                    PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0"),
                    PathBuf::from("/home/user/.ghri/owner/repo/meta.json"),
                    PathBuf::from("/home/user/.ghri/owner/repo/current"),
                ])
            });

        // Is dir: v1.0.0 -> true (only need to check version directories)
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0")))
            .returning(|_| true);

        // --- Execute ---

        let result = show(runtime, "owner/repo", Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_no_current_symlink() {
        // Test showing package info when current symlink doesn't exist

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let current_link = package_dir.join("current"); // /home/user/.ghri/owner/repo/current

        // --- 1. Check Package Exists (package_exists) ---

        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // --- 2. Load Metadata ---

        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1.0.0".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 3. Get Current Version (current_version calls read_link, returns error) ---

        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into())
            });

        // --- 4. List Installed Versions (installed_versions) ---

        // exists on package_dir
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // Read dir -> only meta.json (no version directories)
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(move |_| Ok(vec![PathBuf::from("/home/user/.ghri/owner/repo/meta.json")]));

        // --- Execute ---

        let result = show(runtime, "owner/repo", Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_multiple_versions() {
        // Test showing package info with multiple installed versions

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
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

        // Read meta.json -> current version is v2.0.0
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v2.0.0".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 3. Get Current Version from Symlink ---

        // Is symlink: -> true
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Read link: -> v2.0.0
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v2.0.0")));

        // --- 4. List Installed Versions ---

        // Read dir -> [v1.0.0, v2.0.0, meta.json, current] (multiple versions!)
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(move |_| {
                Ok(vec![
                    PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0"),
                    PathBuf::from("/home/user/.ghri/owner/repo/v2.0.0"),
                    PathBuf::from("/home/user/.ghri/owner/repo/meta.json"),
                    PathBuf::from("/home/user/.ghri/owner/repo/current"),
                ])
            });

        // Is dir checks for each entry
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0")))
            .returning(|_| true); // v1.0.0 is a version directory
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v2.0.0")))
            .returning(|_| true); // v2.0.0 is a version directory
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/meta.json")))
            .returning(|_| false); // meta.json is a file
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/current")))
            .returning(|_| false); // current is a symlink

        // --- Execute ---

        let result = show(runtime, "owner/repo", Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_with_versioned_links() {
        // Test showing package info with versioned links (historical links to specific versions)

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let current_link = package_dir.join("current"); // /home/user/.ghri/owner/repo/current
        let link_dest = PathBuf::from("/usr/local/bin/tool-v1"); // Versioned link destination

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

        // Read meta.json -> has versioned link to v1.0.0
        use crate::package::VersionedLink;
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v2.0.0".into(), // Current is v2
            versioned_links: vec![VersionedLink {
                version: "v1.0.0".into(), // Historical link to v1
                dest: link_dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 3. Get Current Version from Symlink ---

        // Is symlink: -> true
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Read link: -> v2.0.0
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v2.0.0")));

        // --- 4. List Installed Versions ---

        // Read dir -> [v1.0.0, v2.0.0, meta.json]
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(move |_| {
                Ok(vec![
                    PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0"),
                    PathBuf::from("/home/user/.ghri/owner/repo/v2.0.0"),
                    PathBuf::from("/home/user/.ghri/owner/repo/meta.json"),
                ])
            });

        // Is dir checks
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v2.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/meta.json")))
            .returning(|_| false);

        // --- 5. Check Versioned Link Status ---

        // File exists: /usr/local/bin/tool-v1 -> true
        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // Is symlink: /usr/local/bin/tool-v1 -> true
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // Resolve link: /usr/local/bin/tool-v1 -> /home/user/.ghri/owner/repo/v1.0.0/tool
        runtime
            .expect_resolve_link()
            .with(eq(link_dest.clone()))
            .returning(|_| Ok(PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0/tool")));

        // Canonicalize paths for link status check
        runtime
            .expect_canonicalize()
            .returning(|p| Ok(p.to_path_buf()));

        // --- Execute ---

        let result = show(runtime, "owner/repo", Config::for_test(root));
        assert!(result.is_ok());
    }
}
