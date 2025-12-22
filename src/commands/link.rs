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

        // --- Setup Paths ---
        let version_dir = PathBuf::from("/root/o/r/v1");
        let tool_path = PathBuf::from("/root/o/r/v1/tool");

        // --- Read Directory ---

        // Read dir /root/o/r/v1 -> returns single entry [/root/o/r/v1/tool]
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(move |_| Ok(vec![tool_path.clone()]));

        // Check /root/o/r/v1/tool is file (not directory) -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/o/r/v1/tool")))
            .returning(|_| false);

        // --- Execute & Verify ---
        // When version dir has single file, should return that file path
        let result = determine_link_target(&runtime, &version_dir).unwrap();
        assert_eq!(result, PathBuf::from("/root/o/r/v1/tool"));
    }

    #[test]
    fn test_determine_link_target_multiple_files() {
        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let version_dir = PathBuf::from("/root/o/r/v1");

        // --- Read Directory ---

        // Read dir /root/o/r/v1 -> returns multiple entries
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![
                PathBuf::from("/root/o/r/v1/tool"),
                PathBuf::from("/root/o/r/v1/README.md"),
            ]));

        // --- Execute & Verify ---
        // When version dir has multiple files, should return the version dir itself
        let result = determine_link_target(&runtime, &version_dir).unwrap();
        assert_eq!(result, version_dir);
    }

    #[test]
    fn test_determine_link_target_single_directory() {
        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let version_dir = PathBuf::from("/root/o/r/v1");
        let subdir_path = PathBuf::from("/root/o/r/v1/subdir");

        // --- Read Directory ---

        // Read dir /root/o/r/v1 -> returns single entry [/root/o/r/v1/subdir]
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(move |_| Ok(vec![subdir_path.clone()]));

        // Check /root/o/r/v1/subdir is directory -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/o/r/v1/subdir")))
            .returning(|_| true);

        // --- Execute & Verify ---
        // When version dir has single directory (not file), should return version dir itself
        let result = determine_link_target(&runtime, &version_dir).unwrap();
        assert_eq!(result, version_dir);
    }

    #[test]
    fn test_link_dest_is_directory() {
        // When dest is an existing directory, the link should be created inside it
        // with the filename from the link target (single file case)
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json");      // /home/user/.ghri/owner/repo/meta.json
        let version_dir = root.join("owner/repo/v1");           // /home/user/.ghri/owner/repo/v1
        let tool_path = version_dir.join("tool");               // /home/user/.ghri/owner/repo/v1/tool
        let dest_dir = PathBuf::from("/usr/local/bin");
        let final_link = dest_dir.join("tool");                 // /usr/local/bin/tool

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: current version is "v1", no existing links
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Validate Source Version ---

        // Directory exists: /home/user/.ghri/owner/repo/v1 -> true
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri/owner/repo/v1 -> single file [tool]
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(move |_| Ok(vec![tool_path.clone()]));

        // Check /home/user/.ghri/owner/repo/v1/tool is file (not dir) -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")))
            .returning(|_| false);

        // --- 3. Analyze Destination ---

        // Check /usr/local/bin exists -> true
        runtime
            .expect_exists()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        // Check /usr/local/bin is directory -> true
        runtime
            .expect_is_dir()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        // Check /usr/local/bin/tool exists -> false (link doesn't exist yet)
        runtime
            .expect_exists()
            .with(eq(final_link.clone()))
            .returning(|_| false);

        // Check /usr/local/bin/tool is symlink -> false
        runtime
            .expect_is_symlink()
            .with(eq(final_link.clone()))
            .returning(|_| false);

        // --- 4. Create Link ---

        // Create symlink: /usr/local/bin/tool -> /home/user/.ghri/owner/repo/v1/tool
        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")),
                eq(final_link.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 5. Save Metadata ---

        // Write updated meta.json
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo", dest_dir, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_with_explicit_version() {
        // Test linking with explicit version (owner/repo@v2)
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json");      // /home/user/.ghri/owner/repo/meta.json
        let v2_dir = root.join("owner/repo/v2");                // /home/user/.ghri/owner/repo/v2
        let v2_tool_path = v2_dir.join("tool");                 // /home/user/.ghri/owner/repo/v2/tool
        let dest = PathBuf::from("/usr/local/bin/tool");
        let dest_parent = PathBuf::from("/usr/local/bin");

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: current version is "v1" (but we'll link to v2)
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Validate Source Version ---

        // Directory exists: /home/user/.ghri/owner/repo/v2 -> true
        runtime
            .expect_exists()
            .with(eq(v2_dir.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri/owner/repo/v2 -> single file [tool]
        runtime
            .expect_read_dir()
            .with(eq(v2_dir.clone()))
            .returning(move |_| Ok(vec![v2_tool_path.clone()]));

        // Check /home/user/.ghri/owner/repo/v2/tool is file (not dir) -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")))
            .returning(|_| false);

        // --- 3. Analyze Destination ---

        // Check /usr/local/bin/tool exists -> false
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Check /usr/local/bin/tool is symlink -> false
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Check parent /usr/local/bin exists -> true
        runtime
            .expect_exists()
            .with(eq(dest_parent))
            .returning(|_| true);

        // --- 4. Create Link ---

        // Create symlink: /usr/local/bin/tool -> /home/user/.ghri/owner/repo/v2/tool
        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 5. Save Metadata ---

        // Write updated meta.json (with versioned_links entry for v2)
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo@v2", dest, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_version_not_installed() {
        // Test error when specified version is not installed
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json");      // /home/user/.ghri/owner/repo/meta.json
        let v2_dir = root.join("owner/repo/v2");                // /home/user/.ghri/owner/repo/v2
        let dest = PathBuf::from("/usr/local/bin/tool");

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: current version is "v1"
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Validate Source Version (FAIL) ---

        // Directory exists: /home/user/.ghri/owner/repo/v2 -> false (NOT INSTALLED)
        runtime
            .expect_exists()
            .with(eq(v2_dir))
            .returning(|_| false);

        // --- Execute & Verify ---
        // Should fail because v2 is not installed
        let result = link(runtime, "owner/repo@v2", dest, Some(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_link_no_current_version() {
        // Test error when no current version is set and no version specified
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json");      // /home/user/.ghri/owner/repo/meta.json
        let current_link = root.join("owner/repo/current");     // /home/user/.ghri/owner/repo/current
        let dest = PathBuf::from("/usr/local/bin/tool");

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: current_version is EMPTY
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // apply_defaults tries to read /home/user/.ghri/owner/repo/current symlink -> not found
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into()));

        // --- Execute & Verify ---
        // Should fail because no current version and none specified
        let result = link(runtime, "owner/repo", dest, Some(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No current version"));
    }

    #[test]
    fn test_link_with_explicit_path() {
        // Test linking with explicit path (owner/repo:bin/tool)
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json");      // /home/user/.ghri/owner/repo/meta.json
        let version_dir = root.join("owner/repo/v1");           // /home/user/.ghri/owner/repo/v1
        let explicit_path = version_dir.join("bin/tool");       // /home/user/.ghri/owner/repo/v1/bin/tool
        let dest = PathBuf::from("/usr/local/bin/mytool");
        let dest_parent = PathBuf::from("/usr/local/bin");

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: current version is "v1"
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Validate Source Version ---

        // Directory exists: /home/user/.ghri/owner/repo/v1 -> true
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // Explicit path exists: /home/user/.ghri/owner/repo/v1/bin/tool -> true
        runtime
            .expect_exists()
            .with(eq(explicit_path.clone()))
            .returning(|_| true);

        // --- 3. Analyze Destination ---

        // Check /usr/local/bin/mytool exists -> false
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Check /usr/local/bin/mytool is symlink -> false
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Check parent /usr/local/bin exists -> true
        runtime
            .expect_exists()
            .with(eq(dest_parent))
            .returning(|_| true);

        // --- 4. Create Link ---

        // Create symlink: /usr/local/bin/mytool -> /home/user/.ghri/owner/repo/v1/bin/tool
        runtime
            .expect_symlink()
            .with(
                eq(explicit_path),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 5. Save Metadata ---

        // Write updated meta.json (with links entry for bin/tool)
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo:bin/tool", dest, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_explicit_path_not_found() {
        // Test error when explicit path doesn't exist
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json");      // /home/user/.ghri/owner/repo/meta.json
        let version_dir = root.join("owner/repo/v1");           // /home/user/.ghri/owner/repo/v1
        let explicit_path = version_dir.join("bin/notfound");   // /home/user/.ghri/owner/repo/v1/bin/notfound
        let dest = PathBuf::from("/usr/local/bin/tool");

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: current version is "v1"
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Validate Source Version ---

        // Directory exists: /home/user/.ghri/owner/repo/v1 -> true
        runtime
            .expect_exists()
            .with(eq(version_dir))
            .returning(|_| true);

        // Explicit path exists: /home/user/.ghri/owner/repo/v1/bin/notfound -> false (NOT FOUND)
        runtime
            .expect_exists()
            .with(eq(explicit_path))
            .returning(|_| false);

        // --- Execute & Verify ---
        // Should fail because explicit path bin/notfound doesn't exist
        let result = link(runtime, "owner/repo:bin/notfound", dest, Some(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_link_dest_exists_not_symlink() {
        // Test error when destination exists but is not a symlink
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json");      // /home/user/.ghri/owner/repo/meta.json
        let version_dir = root.join("owner/repo/v1");           // /home/user/.ghri/owner/repo/v1
        let tool_path = version_dir.join("tool");               // /home/user/.ghri/owner/repo/v1/tool
        let dest = PathBuf::from("/usr/local/bin/tool");
        let dest_parent = PathBuf::from("/usr/local/bin");

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: current version is "v1"
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Validate Source Version ---

        // Directory exists: /home/user/.ghri/owner/repo/v1 -> true
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri/owner/repo/v1 -> single file [tool]
        runtime
            .expect_read_dir()
            .with(eq(version_dir))
            .returning(move |_| Ok(vec![tool_path.clone()]));

        // Check /home/user/.ghri/owner/repo/v1/tool is file (not dir) -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")))
            .returning(|_| false);

        // --- 3. Analyze Destination (CONFLICT) ---

        // Check /usr/local/bin/tool exists -> true (file exists!)
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);

        // Check /usr/local/bin/tool is directory -> false
        runtime
            .expect_is_dir()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Check parent /usr/local/bin exists -> true
        runtime
            .expect_exists()
            .with(eq(dest_parent))
            .returning(|_| true);

        // Check /usr/local/bin/tool is symlink -> false (it's a regular file!)
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // --- Execute & Verify ---
        // Should fail because dest exists and is not a symlink
        let result = link(runtime, "owner/repo", dest, Some(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a symlink"));
    }

    #[test]
    fn test_link_dest_symlink_to_other_package() {
        // Test error when destination is a symlink to another package
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json");      // /home/user/.ghri/owner/repo/meta.json
        let version_dir = root.join("owner/repo/v1");           // /home/user/.ghri/owner/repo/v1
        let tool_path = version_dir.join("tool");               // /home/user/.ghri/owner/repo/v1/tool
        let dest = PathBuf::from("/usr/local/bin/tool");
        let dest_parent = PathBuf::from("/usr/local/bin");
        let other_package_path = PathBuf::from("/home/user/.ghri/other/package/v1/tool");

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: current version is "v1"
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Validate Source Version ---

        // Directory exists: /home/user/.ghri/owner/repo/v1 -> true
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri/owner/repo/v1 -> single file [tool]
        runtime
            .expect_read_dir()
            .with(eq(version_dir))
            .returning(move |_| Ok(vec![tool_path.clone()]));

        // Check /home/user/.ghri/owner/repo/v1/tool is file (not dir) -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")))
            .returning(|_| false);

        // --- 3. Analyze Destination (CONFLICT) ---

        // Check /usr/local/bin/tool exists -> true
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);

        // Check /usr/local/bin/tool is directory -> false
        runtime
            .expect_is_dir()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Check parent /usr/local/bin exists -> true
        runtime
            .expect_exists()
            .with(eq(dest_parent))
            .returning(|_| true);

        // Check /usr/local/bin/tool is symlink -> true
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);

        // Read symlink /usr/local/bin/tool -> /home/user/.ghri/other/package/v1/tool
        // (Points to a DIFFERENT package!)
        runtime
            .expect_read_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(other_package_path.clone()));

        // --- Execute & Verify ---
        // Should fail because symlink points to different package
        let result = link(runtime, "owner/repo", dest, Some(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not part of package"));
    }

    #[test]
    fn test_link_dest_is_directory_with_explicit_path() {
        // When dest is a directory and explicit path is specified, use the filename from the path
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json");      // /home/user/.ghri/owner/repo/meta.json
        let version_dir = root.join("owner/repo/v1");           // /home/user/.ghri/owner/repo/v1
        let explicit_path = version_dir.join("bin/tool");       // /home/user/.ghri/owner/repo/v1/bin/tool
        let dest_dir = PathBuf::from("/usr/local/bin");
        let final_link = dest_dir.join("tool");                 // /usr/local/bin/tool (filename from explicit path)

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: current version is "v1"
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Validate Source Version ---

        // Directory exists: /home/user/.ghri/owner/repo/v1 -> true
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // Explicit path exists: /home/user/.ghri/owner/repo/v1/bin/tool -> true
        runtime
            .expect_exists()
            .with(eq(explicit_path.clone()))
            .returning(|_| true);

        // --- 3. Analyze Destination ---

        // Check /usr/local/bin exists -> true
        runtime
            .expect_exists()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        // Check /usr/local/bin is directory -> true
        runtime
            .expect_is_dir()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        // Check /usr/local/bin/tool exists -> false
        runtime
            .expect_exists()
            .with(eq(final_link.clone()))
            .returning(|_| false);

        // Check /usr/local/bin/tool is symlink -> false
        runtime
            .expect_is_symlink()
            .with(eq(final_link.clone()))
            .returning(|_| false);

        // --- 4. Create Link ---

        // Create symlink: /usr/local/bin/tool -> /home/user/.ghri/owner/repo/v1/bin/tool
        runtime
            .expect_symlink()
            .with(
                eq(explicit_path),
                eq(final_link.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 5. Save Metadata ---

        // Write updated meta.json
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo:bin/tool", dest_dir, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_creates_parent_directory() {
        // Test that parent directory is created if it doesn't exist
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json");      // /home/user/.ghri/owner/repo/meta.json
        let version_dir = root.join("owner/repo/v1");           // /home/user/.ghri/owner/repo/v1
        let tool_path = version_dir.join("tool");               // /home/user/.ghri/owner/repo/v1/tool
        let dest = PathBuf::from("/opt/mytools/bin/tool");
        let dest_parent = PathBuf::from("/opt/mytools/bin");

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: current version is "v1"
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Validate Source Version ---

        // Directory exists: /home/user/.ghri/owner/repo/v1 -> true
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri/owner/repo/v1 -> single file [tool]
        runtime
            .expect_read_dir()
            .with(eq(version_dir))
            .returning(move |_| Ok(vec![tool_path.clone()]));

        // Check /home/user/.ghri/owner/repo/v1/tool is file (not dir) -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")))
            .returning(|_| false);

        // --- 3. Analyze Destination ---

        // Check /opt/mytools/bin/tool exists -> false
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Check /opt/mytools/bin/tool is symlink -> false
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Check parent /opt/mytools/bin exists -> false (NEEDS TO BE CREATED)
        runtime
            .expect_exists()
            .with(eq(dest_parent.clone()))
            .returning(|_| false);

        // --- 4. Create Parent Directory ---

        // Create directory: /opt/mytools/bin
        runtime
            .expect_create_dir_all()
            .with(eq(dest_parent))
            .returning(|_| Ok(()));

        // --- 5. Create Link ---

        // Create symlink: /opt/mytools/bin/tool -> /home/user/.ghri/owner/repo/v1/tool
        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 6. Save Metadata ---

        // Write updated meta.json
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo", dest, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_update_existing_versioned_link() {
        // Test updating an existing versioned link from v1 to v2
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json");      // /home/user/.ghri/owner/repo/meta.json
        let v1_tool_path = root.join("owner/repo/v1/tool");     // /home/user/.ghri/owner/repo/v1/tool (old)
        let v2_dir = root.join("owner/repo/v2");                // /home/user/.ghri/owner/repo/v2
        let v2_tool_path = v2_dir.join("tool");                 // /home/user/.ghri/owner/repo/v2/tool (new)
        let dest = PathBuf::from("/usr/local/bin/tool");
        let dest_parent = PathBuf::from("/usr/local/bin");

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: current version is "v1", with existing versioned link to /usr/local/bin/tool
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

        // --- 2. Validate Source Version ---

        // Directory exists: /home/user/.ghri/owner/repo/v2 -> true
        runtime
            .expect_exists()
            .with(eq(v2_dir.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri/owner/repo/v2 -> single file [tool]
        runtime
            .expect_read_dir()
            .with(eq(v2_dir))
            .returning(move |_| Ok(vec![v2_tool_path.clone()]));

        // Check /home/user/.ghri/owner/repo/v2/tool is file (not dir) -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")))
            .returning(|_| false);

        // --- 3. Analyze Destination ---

        // Check /usr/local/bin/tool exists -> true (existing symlink!)
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);

        // Check /usr/local/bin/tool is directory -> false
        runtime
            .expect_is_dir()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // Check parent /usr/local/bin exists -> true
        runtime
            .expect_exists()
            .with(eq(dest_parent))
            .returning(|_| true);

        // Check /usr/local/bin/tool is symlink -> true
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);

        // Read symlink /usr/local/bin/tool -> /home/user/.ghri/owner/repo/v1/tool
        // (Points to OLD version v1, which is part of same package - OK to update)
        runtime
            .expect_read_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(v1_tool_path.clone()));

        // --- 4. Update Link (Swap from v1 to v2) ---

        // Remove old symlink: /usr/local/bin/tool
        runtime
            .expect_remove_symlink()
            .with(eq(dest.clone()))
            .returning(|_| Ok(()));

        // Create new symlink: /usr/local/bin/tool -> /home/user/.ghri/owner/repo/v2/tool
        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 5. Save Metadata ---

        // Write updated meta.json (versioned_links updated: v1 -> v2 for /usr/local/bin/tool)
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo@v2", dest, Some(root));
        assert!(result.is_ok());
    }
}
