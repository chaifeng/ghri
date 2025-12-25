use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::application::LinkAction;
use crate::package::{LinkRule, VersionedLink};
use crate::runtime::{Runtime, resolve_relative_path};

use super::config::Config;
use super::link_spec::LinkSpec;

/// Link a package's current version to a destination directory
#[tracing::instrument(skip(runtime, config))]
pub fn link<R: Runtime>(runtime: R, repo_str: &str, dest: PathBuf, config: Config) -> Result<()> {
    let spec = repo_str.parse::<LinkSpec>()?;

    // Convert relative dest path to absolute using current working directory
    let dest = if dest.is_relative() {
        let cwd = runtime.current_dir()?;
        resolve_relative_path(&cwd, &dest)
    } else {
        dest
    };

    let action = LinkAction::new(&runtime, config.install_root);

    // Check package is installed
    if !action.is_installed(&spec.repo.owner, &spec.repo.repo) {
        anyhow::bail!(
            "Package {} is not installed. Install it first with: ghri install {}",
            spec.repo,
            spec.repo
        );
    }

    let mut meta = action.load_meta(&spec.repo.owner, &spec.repo.repo)?;
    let package_dir = action.package_dir(&spec.repo.owner, &spec.repo.repo);

    // Determine which version to link
    let version = if let Some(ref v) = spec.version {
        // Check if specified version exists
        if !action.is_version_installed(&spec.repo.owner, &spec.repo.repo, v) {
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

    let version_dir = action.version_dir(&spec.repo.owner, &spec.repo.repo, &version);
    if !action.exists(&version_dir) {
        anyhow::bail!(
            "Version directory {:?} does not exist. The package may be corrupted.",
            version_dir
        );
    }

    // Determine link target based on spec.path or default behavior
    let link_target = if let Some(ref path) = spec.path {
        let target = version_dir.join(path);
        if !action.exists(&target) {
            anyhow::bail!(
                "Path '{}' does not exist in version {} of {}",
                path,
                version,
                spec.repo
            );
        }
        target
    } else {
        action.find_default_target(&version_dir)?
    };

    // If dest is an existing directory, create link inside it
    // Use the filename from the link target (either specified path or detected file)
    let final_dest = if action.exists(&dest) && action.is_dir(&dest) {
        let filename = if let Some(ref path) = spec.path {
            // Use the filename from the specified path
            Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| spec.repo.repo.clone())
        } else if link_target == version_dir {
            // When linking to version directory (multiple files), use repo name
            spec.repo.repo.clone()
        } else {
            // When linking to single file, use that filename
            link_target
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| spec.repo.repo.clone())
        };
        dest.join(filename)
    } else {
        dest
    };

    // Prepare destination (check if can update and remove existing if needed)
    action.prepare_link_destination(&final_dest, &package_dir)?;

    // Create the symlink
    action.create_link(&link_target, &final_dest)?;

    // Add or update link rule in meta.json
    // If a version was explicitly specified, save to versioned_links (not updated on install/update)
    // Otherwise save to links (updated on install/update)
    // Ensure uniqueness: when adding to one list, remove from the other list
    if spec.version.is_some() {
        let new_link = VersionedLink {
            version: version.clone(),
            rule: LinkRule {
                dest: final_dest.clone(),
                path: spec.path.clone(),
            },
        };

        // Remove any existing entry with same dest from links (default version links)
        meta.links.retain(|l| l.dest != final_dest);

        // Check if a versioned link with the same dest already exists
        if let Some(existing) = meta
            .versioned_links
            .iter_mut()
            .find(|l| l.dest == final_dest)
        {
            // Update existing versioned link
            existing.version = new_link.version;
            existing.path = new_link.rule.path;
        } else {
            // Add new versioned link
            meta.versioned_links.push(new_link);
        }
    } else {
        let new_rule = LinkRule {
            dest: final_dest.clone(),
            path: spec.path.clone(),
        };

        // Remove any existing entry with same dest from versioned_links
        meta.versioned_links.retain(|l| l.dest != final_dest);

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

    // Save metadata
    action.save_meta(&spec.repo.owner, &spec.repo.repo, &meta)?;

    println!("Linked {} {} -> {:?}", spec.repo, version, final_dest);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Meta;
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
    fn test_link_dest_is_directory() {
        // When dest is an existing directory, the link should be created inside it
        // with the filename from the link target (single file case)
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let version_dir = package_dir.join("v1"); // /home/user/.ghri/owner/repo/v1
        let tool_path = version_dir.join("tool"); // /home/user/.ghri/owner/repo/v1/tool
        let dest_dir = PathBuf::from("/usr/local/bin");
        let final_link = dest_dir.join("tool"); // /usr/local/bin/tool

        // --- 1. is_installed (exists on meta_path) ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // --- 2. load_required -> load (exists + read_to_string) ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 3. exists check for version_dir ---
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // --- 4. find_default_target (read_dir + is_dir) ---
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(move |_| Ok(vec![tool_path.clone()]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")))
            .returning(|_| false);

        // --- 5. Analyze Destination ---
        runtime
            .expect_exists()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_is_dir()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_exists()
            .with(eq(final_link.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(final_link.clone()))
            .returning(|_| false);

        // --- 6. Create Link (create_link checks parent exists) ---
        runtime
            .expect_exists()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("../../../home/user/.ghri/owner/repo/v1/tool")),
                eq(final_link.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 7. Save Metadata (save checks package_dir exists) ---
        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| true);

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo", dest_dir, Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_with_explicit_version() {
        // Test linking with explicit version (owner/repo@v2)
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let v2_dir = package_dir.join("v2"); // /home/user/.ghri/owner/repo/v2
        let v2_tool_path = v2_dir.join("tool"); // /home/user/.ghri/owner/repo/v2/tool
        let dest = PathBuf::from("/usr/local/bin/tool");
        let dest_parent = PathBuf::from("/usr/local/bin");

        // --- 1. Check is_installed (exists on meta_path) ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // --- 2. Load Metadata (load_required -> load -> exists + read_to_string) ---
        // exists is called again inside load()
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 3. is_version_installed (is_dir on version_dir) ---
        runtime
            .expect_is_dir()
            .with(eq(v2_dir.clone()))
            .returning(|_| true);

        // --- 4. exists check for version_dir ---
        runtime
            .expect_exists()
            .with(eq(v2_dir.clone()))
            .returning(|_| true);

        // --- 5. find_default_target (read_dir + is_dir) ---
        runtime
            .expect_read_dir()
            .with(eq(v2_dir.clone()))
            .returning(move |_| Ok(vec![v2_tool_path.clone()]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")))
            .returning(|_| false);

        // --- 6. Analyze Destination ---
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // --- 7. Create Link (create_link checks parent exists) ---
        runtime
            .expect_exists()
            .with(eq(dest_parent))
            .returning(|_| true);

        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("../../../home/user/.ghri/owner/repo/v2/tool")),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 8. Save Metadata (save checks package_dir exists) ---
        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| true);

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo@v2", dest, Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_version_not_installed() {
        // Test error when specified version is not installed
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let v2_dir = root.join("owner/repo/v2"); // /home/user/.ghri/owner/repo/v2
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

        // is_version_installed calls is_dir: /home/user/.ghri/owner/repo/v2 -> false (NOT INSTALLED)
        runtime
            .expect_is_dir()
            .with(eq(v2_dir))
            .returning(|_| false);

        // --- Execute & Verify ---
        // Should fail because v2 is not installed
        let result = link(runtime, "owner/repo@v2", dest, Config::for_test(root));
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
        let meta_path = root.join("owner/repo/meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let current_link = root.join("owner/repo/current"); // /home/user/.ghri/owner/repo/current
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
            .returning(|_| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into())
            });

        // --- Execute & Verify ---
        // Should fail because no current version and none specified
        let result = link(runtime, "owner/repo", dest, Config::for_test(root));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No current version")
        );
    }

    #[test]
    fn test_link_with_explicit_path() {
        // Test linking with explicit path (owner/repo:bin/tool)
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let version_dir = package_dir.join("v1"); // /home/user/.ghri/owner/repo/v1
        let explicit_path = version_dir.join("bin/tool"); // /home/user/.ghri/owner/repo/v1/bin/tool
        let dest = PathBuf::from("/usr/local/bin/mytool");
        let dest_parent = PathBuf::from("/usr/local/bin");

        // --- 1. is_installed (exists on meta_path) ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // --- 2. load_required -> load (exists + read_to_string) ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 3. exists check for version_dir ---
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // --- 4. Explicit path exists ---
        runtime
            .expect_exists()
            .with(eq(explicit_path.clone()))
            .returning(|_| true);

        // --- 5. Analyze Destination ---
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // --- 6. Create Link (create_link checks parent exists) ---
        runtime
            .expect_exists()
            .with(eq(dest_parent))
            .returning(|_| true);

        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from(
                    "../../../home/user/.ghri/owner/repo/v1/bin/tool",
                )),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 7. Save Metadata (save checks package_dir exists) ---
        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| true);

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo:bin/tool", dest, Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_explicit_path_not_found() {
        // Test error when explicit path doesn't exist
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let version_dir = package_dir.join("v1"); // /home/user/.ghri/owner/repo/v1
        let explicit_path = version_dir.join("bin/notfound"); // /home/user/.ghri/owner/repo/v1/bin/notfound
        let dest = PathBuf::from("/usr/local/bin/tool");

        // --- 1. is_installed + load ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

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
        let result = link(
            runtime,
            "owner/repo:bin/notfound",
            dest,
            Config::for_test(root),
        );
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
        let meta_path = root.join("owner/repo/meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let version_dir = root.join("owner/repo/v1"); // /home/user/.ghri/owner/repo/v1
        let tool_path = version_dir.join("tool"); // /home/user/.ghri/owner/repo/v1/tool
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
        let result = link(runtime, "owner/repo", dest, Config::for_test(root));
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
        let meta_path = root.join("owner/repo/meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let version_dir = root.join("owner/repo/v1"); // /home/user/.ghri/owner/repo/v1
        let tool_path = version_dir.join("tool"); // /home/user/.ghri/owner/repo/v1/tool
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

        // Resolve symlink /usr/local/bin/tool -> /home/user/.ghri/other/package/v1/tool
        // (Points to a DIFFERENT package!)
        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(other_package_path.clone()));

        // --- Execute & Verify ---
        // Should fail because symlink points to different package
        let result = link(runtime, "owner/repo", dest, Config::for_test(root));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not managed by this package")
        );
    }

    #[test]
    fn test_link_dest_is_directory_with_explicit_path() {
        // When dest is a directory and explicit path is specified, use the filename from the path
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let version_dir = package_dir.join("v1"); // /home/user/.ghri/owner/repo/v1
        let explicit_path = version_dir.join("bin/tool"); // /home/user/.ghri/owner/repo/v1/bin/tool
        let dest_dir = PathBuf::from("/usr/local/bin");
        let final_link = dest_dir.join("tool"); // /usr/local/bin/tool (filename from explicit path)

        // --- 1. is_installed + load ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. exists check for version_dir ---
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // --- 3. Explicit path exists ---
        runtime
            .expect_exists()
            .with(eq(explicit_path.clone()))
            .returning(|_| true);

        // --- 4. Analyze Destination ---
        runtime
            .expect_exists()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_is_dir()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_exists()
            .with(eq(final_link.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(final_link.clone()))
            .returning(|_| false);

        // --- 5. Create Link (create_link checks parent exists) ---
        runtime
            .expect_exists()
            .with(eq(dest_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from(
                    "../../../home/user/.ghri/owner/repo/v1/bin/tool",
                )),
                eq(final_link.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 6. Save Metadata (save checks package_dir exists) ---
        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| true);

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(
            runtime,
            "owner/repo:bin/tool",
            dest_dir,
            Config::for_test(root),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_creates_parent_directory() {
        // Test that parent directory is created if it doesn't exist
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let version_dir = package_dir.join("v1"); // /home/user/.ghri/owner/repo/v1
        let tool_path = version_dir.join("tool"); // /home/user/.ghri/owner/repo/v1/tool
        let dest = PathBuf::from("/opt/mytools/bin/tool");
        let dest_parent = PathBuf::from("/opt/mytools/bin");

        // --- 1. is_installed + load ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. exists check for version_dir ---
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // --- 3. find_default_target ---
        runtime
            .expect_read_dir()
            .with(eq(version_dir))
            .returning(move |_| Ok(vec![tool_path.clone()]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")))
            .returning(|_| false);

        // --- 4. Analyze Destination ---
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);

        // --- 5. Create Link (create_link checks parent, creates if needed) ---
        runtime
            .expect_exists()
            .with(eq(dest_parent.clone()))
            .returning(|_| false);

        runtime
            .expect_create_dir_all()
            .with(eq(dest_parent))
            .returning(|_| Ok(()));

        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("../../../home/user/.ghri/owner/repo/v1/tool")),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 6. Save Metadata (save checks package_dir exists) ---
        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| true);

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo", dest, Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_update_existing_versioned_link() {
        // Test updating an existing versioned link from v1 to v2
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo"); // /home/user/.ghri/owner/repo
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let v1_tool_path = package_dir.join("v1/tool"); // /home/user/.ghri/owner/repo/v1/tool (old)
        let v2_dir = package_dir.join("v2"); // /home/user/.ghri/owner/repo/v2
        let v2_tool_path = v2_dir.join("tool"); // /home/user/.ghri/owner/repo/v2/tool (new)
        let dest = PathBuf::from("/usr/local/bin/tool");
        let dest_parent = PathBuf::from("/usr/local/bin");

        // --- 1. is_installed + load ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            versioned_links: vec![VersionedLink {
                version: "v1".into(),
                rule: LinkRule {
                    dest: dest.clone(),
                    path: None,
                },
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. is_version_installed (is_dir) ---
        runtime
            .expect_is_dir()
            .with(eq(v2_dir.clone()))
            .returning(|_| true);

        // --- 3. exists check for version_dir ---
        runtime
            .expect_exists()
            .with(eq(v2_dir.clone()))
            .returning(|_| true);

        // --- 4. find_default_target ---
        runtime
            .expect_read_dir()
            .with(eq(v2_dir))
            .returning(move |_| Ok(vec![v2_tool_path.clone()]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")))
            .returning(|_| false);

        // --- 5. Analyze Destination ---
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);

        // Check if dest is a directory (it's a symlink, not a directory)
        runtime
            .expect_is_dir()
            .with(eq(dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);

        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(v1_tool_path.clone()));

        // --- 6. remove_link (LinkManager) checks is_symlink, removes ---
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);

        runtime
            .expect_remove_symlink()
            .with(eq(dest.clone()))
            .returning(|_| Ok(()));

        // --- 7. Create Link (create_link checks parent exists) ---
        runtime
            .expect_exists()
            .with(eq(dest_parent))
            .returning(|_| true);

        runtime
            .expect_symlink()
            .with(
                eq(PathBuf::from("../../../home/user/.ghri/owner/repo/v2/tool")),
                eq(dest.clone()),
            )
            .returning(|_, _| Ok(()));

        // --- 8. Save Metadata (save checks package_dir exists) ---
        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| true);

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo@v2", dest, Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_versioned_removes_from_default_links() {
        // Test that creating a versioned link removes any existing entry from default links
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");
        let meta_path = package_dir.join("meta.json");
        let v1_dir = package_dir.join("v1");
        let v2_dir = package_dir.join("v2");
        let v2_tool_path = v2_dir.join("tool");
        let dest = PathBuf::from("/usr/local/bin/tool");
        let dest_parent = PathBuf::from("/usr/local/bin");

        // --- 1. is_installed + load ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Meta has an existing DEFAULT link (in meta.links) to this dest
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: dest.clone(),
                path: None,
            }],
            versioned_links: vec![],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. is_version_installed (is_dir) ---
        runtime
            .expect_is_dir()
            .with(eq(v2_dir.clone()))
            .returning(|_| true);

        // --- 3. exists check for version_dir ---
        runtime
            .expect_exists()
            .with(eq(v2_dir.clone()))
            .returning(|_| true);

        // --- 4. find_default_target ---
        runtime
            .expect_read_dir()
            .with(eq(v2_dir))
            .returning(move |_| Ok(vec![v2_tool_path.clone()]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v2/tool")))
            .returning(|_| false);

        // --- 5. Analyze Destination ---
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);

        runtime
            .expect_is_dir()
            .with(eq(dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);

        // Existing symlink points to v1 (same package)
        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(v1_dir.join("tool")));

        // --- 6. remove_link (LinkManager) ---
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);

        runtime
            .expect_remove_symlink()
            .with(eq(dest.clone()))
            .returning(|_| Ok(()));

        // --- 7. Create Link ---
        runtime
            .expect_exists()
            .with(eq(dest_parent))
            .returning(|_| true);

        runtime.expect_symlink().returning(|_, _| Ok(()));

        // --- 8. Save Metadata ---
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // The written meta should have:
        // - Empty links (default link removed)
        // - versioned_links with v2 entry
        runtime
            .expect_write()
            .withf(|_, data| {
                let json_str = std::str::from_utf8(data).unwrap();
                let saved_meta: Meta = serde_json::from_str(json_str).unwrap();
                // Default links should be empty (removed when versioned link was created)
                saved_meta.links.is_empty() &&
            // Should have one versioned link
            saved_meta.versioned_links.len() == 1 &&
            saved_meta.versioned_links[0].version == "v2"
            })
            .returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = link(runtime, "owner/repo@v2", dest, Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_default_removes_from_versioned_links() {
        // Test that creating a default link removes any existing entry from versioned_links
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");
        let meta_path = package_dir.join("meta.json");
        let v1_dir = package_dir.join("v1");
        let v1_tool_path = v1_dir.join("tool");
        let v2_dir = package_dir.join("v2");
        let dest = PathBuf::from("/usr/local/bin/tool");
        let dest_parent = PathBuf::from("/usr/local/bin");

        // --- 1. is_installed + load ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Meta has an existing VERSIONED link to this dest
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![],
            versioned_links: vec![VersionedLink {
                version: "v2".into(),
                rule: LinkRule {
                    dest: dest.clone(),
                    path: None,
                },
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. exists check for version_dir (default link, no is_version_installed) ---
        runtime
            .expect_exists()
            .with(eq(v1_dir.clone()))
            .returning(|_| true);

        // --- 3. find_default_target ---
        runtime
            .expect_read_dir()
            .with(eq(v1_dir.clone()))
            .returning(move |_| Ok(vec![v1_tool_path.clone()]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool")))
            .returning(|_| false);

        // --- 4. Analyze Destination ---
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);

        runtime
            .expect_is_dir()
            .with(eq(dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);

        // Existing symlink points to v2 (same package)
        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(v2_dir.join("tool")));

        // --- 5. remove_link (LinkManager) ---
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);

        runtime
            .expect_remove_symlink()
            .with(eq(dest.clone()))
            .returning(|_| Ok(()));

        // --- 6. Create Link ---
        runtime
            .expect_exists()
            .with(eq(dest_parent))
            .returning(|_| true);

        runtime.expect_symlink().returning(|_, _| Ok(()));

        // --- 7. Save Metadata ---
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        // The written meta should have:
        // - links with one entry (default link)
        // - Empty versioned_links (versioned link removed)
        runtime
            .expect_write()
            .withf(|_, data| {
                let json_str = std::str::from_utf8(data).unwrap();
                let saved_meta: Meta = serde_json::from_str(json_str).unwrap();
                // Should have one default link
                saved_meta.links.len() == 1 &&
            // Versioned links should be empty (removed when default link was created)
            saved_meta.versioned_links.is_empty()
            })
            .returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        // No version specified -> creates default link
        let result = link(runtime, "owner/repo", dest, Config::for_test(root));
        assert!(result.is_ok());
    }
}
