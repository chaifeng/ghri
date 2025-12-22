use anyhow::Result;
use log::debug;
use std::path::{Path, PathBuf};

use crate::{
    github::LinkSpec,
    package::{LinkRule, Meta, VersionedLink},
    runtime::Runtime,
};

use super::paths::default_install_root;

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
pub(crate) fn determine_link_target<R: Runtime>(runtime: &R, version_dir: &Path) -> Result<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn test_link_with_explicit_version() {
        // Test linking with explicit version (owner/repo@v2)
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let dest = PathBuf::from("/usr/local/bin/tool");

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

        // Explicit version v2 exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/v2")))
            .returning(|_| true);

        // Read version dir - has single file
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner/repo/v2")))
            .returning(|_| Ok(vec![PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")))
            .returning(|_| false);

        // Dest doesn't exist as directory
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Parent exists
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/usr/local/bin")))
            .returning(|_| true);

        // Create symlink
        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // Save meta
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = link(runtime, "owner/repo@v2", dest, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_version_not_installed() {
        // Test error when specified version is not installed
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let dest = PathBuf::from("/usr/local/bin/tool");

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

        // Explicit version v2 does NOT exist
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/v2")))
            .returning(|_| false);

        let result = link(runtime, "owner/repo@v2", dest, Some(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_link_no_current_version() {
        // Test error when no current version is set and no version specified
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let dest = PathBuf::from("/usr/local/bin/tool");
        let current_link = root.join("owner/repo/current");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta with empty current_version
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // apply_defaults tries to read current symlink when current_version is empty
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into()));

        let result = link(runtime, "owner/repo", dest, Some(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No current version"));
    }

    #[test]
    fn test_link_with_explicit_path() {
        // Test linking with explicit path (owner/repo:bin/tool)
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let dest = PathBuf::from("/usr/local/bin/mytool");

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

        // Explicit path exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/v1/bin/tool")))
            .returning(|_| true);

        // Dest doesn't exist as directory
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Parent exists
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/usr/local/bin")))
            .returning(|_| true);

        // Create symlink
        runtime
            .expect_symlink()
            .with(
                eq(root.join("owner/repo/v1/bin/tool")),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // Save meta
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = link(runtime, "owner/repo:bin/tool", dest, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_explicit_path_not_found() {
        // Test error when explicit path doesn't exist
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let dest = PathBuf::from("/usr/local/bin/tool");

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

        // Explicit path does NOT exist
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/v1/bin/notfound")))
            .returning(|_| false);

        let result = link(runtime, "owner/repo:bin/notfound", dest, Some(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_link_dest_exists_not_symlink() {
        // Test error when destination exists but is not a symlink
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let dest = PathBuf::from("/usr/local/bin/tool");

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

        // Dest is not a directory
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);

        runtime
            .expect_is_dir()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Parent directory exists
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/usr/local/bin")))
            .returning(|_| true);

        // Dest exists but is not a symlink (exists returns true, so is_symlink only called in inner if)
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        let result = link(runtime, "owner/repo", dest, Some(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a symlink"));
    }

    #[test]
    fn test_link_dest_symlink_to_other_package() {
        // Test error when destination is a symlink to another package
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let dest = PathBuf::from("/usr/local/bin/tool");

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

        // Dest is not a directory
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);

        runtime
            .expect_is_dir()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Parent directory exists
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/usr/local/bin")))
            .returning(|_| true);

        // Dest is a symlink to another package (exists returns true, so is_symlink only called in inner if)
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);

        runtime
            .expect_read_link()
            .with(eq(dest.clone()))
            .returning(|_| Ok(PathBuf::from("/home/user/.ghri/other/package/v1/tool")));

        let result = link(runtime, "owner/repo", dest, Some(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not part of package"));
    }

    #[test]
    fn test_link_dest_is_directory_with_explicit_path() {
        // When dest is a directory and explicit path is specified, use the filename from the path
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let dest_dir = PathBuf::from("/usr/local/bin");
        let final_link = dest_dir.join("tool"); // filename from explicit path bin/tool

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

        // Explicit path exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/v1/bin/tool")))
            .returning(|_| true);

        // Dest is a directory
        runtime
            .expect_exists()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_is_dir()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        // Final link doesn't exist
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
                eq(root.join("owner/repo/v1/bin/tool")),
                eq(final_link.clone()),
            )
            .returning(|_, _| Ok(()));

        // Save meta
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = link(runtime, "owner/repo:bin/tool", dest_dir, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_creates_parent_directory() {
        // Test that parent directory is created if it doesn't exist
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let dest = PathBuf::from("/opt/mytools/bin/tool");

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

        // Dest is not a directory (doesn't exist)
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Parent doesn't exist
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/opt/mytools/bin")))
            .returning(|_| false);

        // Create parent directory
        runtime
            .expect_create_dir_all()
            .with(eq(PathBuf::from("/opt/mytools/bin")))
            .returning(|_| Ok(()));

        // Create symlink
        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // Save meta
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = link(runtime, "owner/repo", dest, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_update_existing_versioned_link() {
        // Test updating an existing versioned link
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let dest = PathBuf::from("/usr/local/bin/tool");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta with existing versioned link
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            versioned_links: vec![VersionedLink {
                version: "v1".into(),
                dest: dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Version v2 exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/v2")))
            .returning(|_| true);

        // Read version dir - has single file
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner/repo/v2")))
            .returning(|_| Ok(vec![PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")))
            .returning(|_| false);

        // Dest doesn't exist as directory (checked first)
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);

        runtime
            .expect_is_dir()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Parent directory exists
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/usr/local/bin")))
            .returning(|_| true);

        // Dest is a symlink to same package (exists returns true, so is_symlink only called in inner if)
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);

        runtime
            .expect_read_link()
            .with(eq(dest.clone()))
            .returning(|_| Ok(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")));

        // Remove old symlink
        runtime
            .expect_remove_symlink()
            .with(eq(dest.clone()))
            .returning(|_| Ok(()));

        // Create symlink
        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // Save meta
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = link(runtime, "owner/repo@v2", dest, Some(root));
        assert!(result.is_ok());
    }
}
