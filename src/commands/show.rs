use anyhow::Result;
use log::debug;
use std::path::PathBuf;

use crate::{
    github::RepoSpec,
    package::Meta,
    runtime::Runtime,
};

use super::paths::default_install_root;
use super::{print_links, print_versioned_links};

/// Show detailed information about a package
#[tracing::instrument(skip(runtime, install_root))]
pub fn show<R: Runtime>(
    runtime: R,
    repo_str: &str,
    install_root: Option<PathBuf>,
) -> Result<()> {
    debug!("Showing info for {}", repo_str);
    let spec = repo_str.parse::<RepoSpec>()?;
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(&runtime)?,
    };
    debug!("Using install root: {:?}", root);

    let package_dir = root.join(&spec.repo.owner).join(&spec.repo.repo);
    let meta_path = package_dir.join("meta.json");
    debug!("Package directory: {:?}", package_dir);

    if !runtime.exists(&package_dir) {
        anyhow::bail!(
            "Package {} is not installed.",
            spec.repo
        );
    }

    // Load meta
    let meta = if runtime.exists(&meta_path) {
        Some(Meta::load(&runtime, &meta_path)?)
    } else {
        None
    };

    // Package name
    println!("Package: {}", spec.repo);
    println!("Directory: {}", package_dir.display());

    // Current version
    let current_link = package_dir.join("current");
    if runtime.is_symlink(&current_link) {
        if let Ok(target) = runtime.read_link(&current_link) {
            let current_version = target.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            println!("Current version: {}", current_version);
        }
    } else if let Some(ref meta) = meta {
        println!("Current version: {}", meta.current_version);
    }

    // List installed versions
    println!("\nInstalled versions:");
    let entries = runtime.read_dir(&package_dir)?;
    let mut versions: Vec<String> = entries
        .iter()
        .filter_map(|entry| {
            let name = entry.file_name()?.to_str()?.to_string();
            // Skip meta.json and current symlink
            if name == "meta.json" || name == "current" {
                return None;
            }
            if runtime.is_dir(entry) {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    versions.sort();

    let current_version = if runtime.is_symlink(&current_link) {
        runtime.read_link(&current_link).ok()
            .and_then(|t| t.file_name().and_then(|s| s.to_str()).map(String::from))
    } else {
        None
    };

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
                let installed = versions.iter().any(|iv| *iv == release.version);
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
            print_versioned_links(&runtime, &meta.versioned_links, &package_dir, Some("Versioned links (historical):"));
        }
    }

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
    fn test_show_package_info() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");
        let meta_path = package_dir.join("meta.json");
        let current_link = package_dir.join("current");
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // Meta exists
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Load meta with full info
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

        // Current symlink exists
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        // Read directory for versions
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

        // Check if entries are directories
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0")))
            .returning(|_| true);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/meta.json")))
            .returning(|_| false);
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/current")))
            .returning(|_| false);

        // Check link status - exists and is_symlink checks for link dest
        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(link_dest.clone()))
            .returning(|_| Ok(PathBuf::from("/home/user/.ghri/owner/repo/current/bin/tool")));

        let result = show(runtime, "owner/repo", Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_nonexistent_package_fails() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");

        // Package does not exist
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| false);

        let result = show(runtime, "owner/repo", Some(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_show_without_meta() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");
        let meta_path = package_dir.join("meta.json");
        let current_link = package_dir.join("current");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // Meta does not exist
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| false);

        // Current symlink exists
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        // Read directory for versions
        runtime
            .expect_read_dir()
            .with(eq(package_dir.clone()))
            .returning(move |_| {
                Ok(vec![
                    PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0"),
                ])
            });

        // Check if entry is directory
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0")))
            .returning(|_| true);

        let result = show(runtime, "owner/repo", Some(root));
        assert!(result.is_ok());
    }
}
