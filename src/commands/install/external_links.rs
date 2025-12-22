use anyhow::Result;
use log::{info, warn};
use std::path::Path;

use crate::{
    package::{LinkRule, Meta},
    runtime::Runtime,
};

use crate::commands::determine_link_target;

/// Update external links for a package after installation
/// Iterates through all link rules and updates each symlink
#[tracing::instrument(skip(runtime, _package_dir, version_dir))]
pub(crate) fn update_external_links<R: Runtime>(
    runtime: &R,
    _package_dir: &Path,
    version_dir: &Path,
    meta: &Meta,
) -> Result<()> {
    let mut errors = Vec::new();

    for rule in &meta.links {
        if let Err(e) = update_single_link(runtime, version_dir, rule) {
            // Log error but continue with other links
            eprintln!(
                "Error updating link {:?}: {}",
                rule.dest, e
            );
            errors.push((rule.dest.clone(), e));
        }
    }

    // Also handle legacy linked_to field for backward compatibility
    if let Some(ref linked_to) = meta.linked_to {
        let legacy_rule = LinkRule {
            dest: linked_to.clone(),
            path: meta.linked_path.clone(),
        };
        if let Err(e) = update_single_link(runtime, version_dir, &legacy_rule) {
            eprintln!(
                "Error updating legacy link {:?}: {}",
                linked_to, e
            );
            errors.push((linked_to.clone(), e));
        }
    }

    if !errors.is_empty() {
        warn!(
            "{} link(s) failed to update, but continuing",
            errors.len()
        );
    }

    Ok(())
}

/// Update a single link according to a link rule
fn update_single_link<R: Runtime>(
    runtime: &R,
    version_dir: &Path,
    rule: &LinkRule,
) -> Result<()> {
    // Determine link target based on rule.path or default behavior
    let link_target = if let Some(ref path) = rule.path {
        let target = version_dir.join(path);
        if !runtime.exists(&target) {
            anyhow::bail!(
                "Path '{}' does not exist in {:?}",
                path, version_dir
            );
        }
        target
    } else {
        determine_link_target(runtime, version_dir)?
    };

    let linked_to = &rule.dest;

    if runtime.exists(linked_to) || runtime.is_symlink(linked_to) {
        if runtime.is_symlink(linked_to) {
            // Remove the old symlink
            runtime.remove_symlink(linked_to)?;

            // Create new symlink to the new version
            runtime.symlink(&link_target, linked_to)?;

            info!("Updated external link {:?} -> {:?}", linked_to, link_target);
        } else {
            warn!(
                "External link target {:?} exists but is not a symlink, skipping update",
                linked_to
            );
        }
    } else {
        // linked_to path doesn't exist anymore, create it
        if let Some(parent) = linked_to.parent() {
            if !runtime.exists(parent) {
                runtime.create_dir_all(parent)?;
            }
        }

        runtime.symlink(&link_target, linked_to)?;
        info!("Recreated external link {:?} -> {:?}", linked_to, link_target);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;
    use std::path::PathBuf;

    #[test]
    fn test_update_external_links_no_links() {
        // Test that update_external_links succeeds when there are no link rules

        let runtime = MockRuntime::new();

        // --- Setup ---
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            links: vec![],  // No links!
            ..Default::default()
        };

        // --- Execute ---

        // Should return Ok without doing anything
        let result = update_external_links(
            &runtime,
            Path::new("/root/o/r"),
            Path::new("/root/o/r/v1"),
            &meta,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_external_links_updates_existing_symlink() {
        // Test updating an existing symlink to point to a new version

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let linked_to = PathBuf::from("/usr/local/bin/tool");     // External symlink location
        let version_dir = PathBuf::from("/root/o/r/v2");          // New version directory
        let link_target = PathBuf::from("/root/o/r/v2/tool");     // Target binary in new version

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v2".into(),
            links: vec![LinkRule {
                dest: linked_to.clone(),
                path: None,
            }],
            ..Default::default()
        };

        // --- 1. Check if Destination Exists ---

        // File exists: /usr/local/bin/tool -> true
        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // Is symlink: /usr/local/bin/tool -> true
        runtime
            .expect_is_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // --- 2. Remove Old Symlink ---

        // Remove symlink: /usr/local/bin/tool
        runtime
            .expect_remove_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| Ok(()));

        // --- 3. Determine Link Target ---

        // Read dir /root/o/r/v2 -> [tool]
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("/root/o/r/v2/tool")]));

        // Is dir: /root/o/r/v2/tool -> false (it's a file)
        runtime
            .expect_is_dir()
            .with(eq(link_target.clone()))
            .returning(|_| false);

        // --- 4. Create New Symlink ---

        // Create symlink: /usr/local/bin/tool -> /root/o/r/v2/tool
        runtime
            .expect_symlink()
            .with(eq(link_target), eq(linked_to.clone()))
            .returning(|_, _| Ok(()));

        // --- Execute ---

        let result = update_external_links(
            &runtime,
            Path::new("/root/o/r"),
            &version_dir,
            &meta,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_single_link_with_specific_path() {
        // Test creating a link with an explicit path specified in the rule

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let linked_to = PathBuf::from("/usr/local/bin/mytool");   // External symlink location
        let version_dir = PathBuf::from("/root/o/r/v1");          // Version directory
        let target_path = version_dir.join("bin/tool");           // /root/o/r/v1/bin/tool

        let rule = LinkRule {
            dest: linked_to.clone(),
            path: Some("bin/tool".to_string()),                   // Explicit path!
        };

        // --- 1. Check if Explicit Path Exists ---

        // File exists: /root/o/r/v1/bin/tool -> true
        runtime
            .expect_exists()
            .with(eq(target_path.clone()))
            .returning(|_| true);

        // --- 2. Check if Destination Exists ---

        // File exists: /usr/local/bin/mytool -> false
        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| false);

        // Is symlink: /usr/local/bin/mytool -> false (for broken symlink check)
        runtime
            .expect_is_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| false);

        // --- 3. Check Parent Directory ---

        // Parent exists: /usr/local/bin -> true
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/usr/local/bin")))
            .returning(|_| true);

        // --- 4. Create Symlink ---

        // Create symlink: /usr/local/bin/mytool -> /root/o/r/v1/bin/tool
        runtime
            .expect_symlink()
            .with(eq(target_path), eq(linked_to))
            .returning(|_, _| Ok(()));

        // --- Execute ---

        let result = update_single_link(&runtime, &version_dir, &rule);
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_single_link_path_not_exists() {
        // Test that link creation fails when the specified path doesn't exist

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let linked_to = PathBuf::from("/usr/local/bin/mytool");
        let version_dir = PathBuf::from("/root/o/r/v1");
        let target_path = version_dir.join("bin/nonexistent");    // /root/o/r/v1/bin/nonexistent

        let rule = LinkRule {
            dest: linked_to,
            path: Some("bin/nonexistent".to_string()),            // Path doesn't exist!
        };

        // --- 1. Check if Explicit Path Exists ---

        // File exists: /root/o/r/v1/bin/nonexistent -> false
        runtime
            .expect_exists()
            .with(eq(target_path))
            .returning(|_| false);

        // --- Execute & Verify ---

        let result = update_single_link(&runtime, &version_dir, &rule);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_update_single_link_target_not_symlink() {
        // Test that update is skipped when destination is a regular file (not symlink)

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let linked_to = PathBuf::from("/usr/local/bin/tool");
        let version_dir = PathBuf::from("/root/o/r/v1");

        let rule = LinkRule {
            dest: linked_to.clone(),
            path: None,
        };

        // --- 1. Determine Link Target ---

        // Read dir /root/o/r/v1 -> [tool]
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("/root/o/r/v1/tool")]));

        // Is dir: /root/o/r/v1/tool -> false
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/o/r/v1/tool")))
            .returning(|_| false);

        // --- 2. Check Destination ---

        // File exists: /usr/local/bin/tool -> true
        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // Is symlink: /usr/local/bin/tool -> false (it's a regular file!)
        runtime
            .expect_is_symlink()
            .with(eq(linked_to))
            .returning(|_| false);

        // --- Execute ---

        // Should succeed but skip the update (logs warning)
        let result = update_single_link(&runtime, &version_dir, &rule);
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_single_link_create_parent_dir() {
        // Test that parent directory is created when it doesn't exist

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let linked_to = PathBuf::from("/new/path/bin/tool");
        let parent_dir = PathBuf::from("/new/path/bin");
        let version_dir = PathBuf::from("/root/o/r/v1");
        let link_target = PathBuf::from("/root/o/r/v1/tool");

        let rule = LinkRule {
            dest: linked_to.clone(),
            path: None,
        };

        // --- 1. Determine Link Target ---

        // Read dir /root/o/r/v1 -> [tool]
        let link_target_for_read = link_target.clone();
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(move |_| Ok(vec![link_target_for_read.clone()]));

        // Is dir: /root/o/r/v1/tool -> false
        runtime
            .expect_is_dir()
            .with(eq(link_target.clone()))
            .returning(|_| false);

        // --- 2. Check Destination ---

        // File exists: /new/path/bin/tool -> false
        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| false);

        // Is symlink: /new/path/bin/tool -> false
        runtime
            .expect_is_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| false);

        // --- 3. Create Parent Directory ---

        // Parent exists: /new/path/bin -> false
        runtime
            .expect_exists()
            .with(eq(parent_dir.clone()))
            .returning(|_| false);

        // Create parent dir: /new/path/bin
        runtime
            .expect_create_dir_all()
            .with(eq(parent_dir))
            .returning(|_| Ok(()));

        // --- 4. Create Symlink ---

        // Create symlink: /new/path/bin/tool -> /root/o/r/v1/tool
        runtime
            .expect_symlink()
            .with(eq(link_target), eq(linked_to))
            .returning(|_, _| Ok(()));

        // --- Execute ---

        let result = update_single_link(&runtime, &version_dir, &rule);
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_external_links_with_legacy_linked_to() {
        // Test backward compatibility with legacy linked_to field

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let linked_to = PathBuf::from("/usr/local/bin/legacy-tool");
        let version_dir = PathBuf::from("/root/o/r/v1");
        let link_target = PathBuf::from("/root/o/r/v1/tool");

        // Meta uses legacy linked_to field instead of links array
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            linked_to: Some(linked_to.clone()),                   // Legacy field!
            linked_path: None,
            links: vec![],                                        // Empty links array
            ..Default::default()
        };

        // --- 1. Determine Link Target ---

        // Read dir /root/o/r/v1 -> [tool]
        let link_target_for_read = link_target.clone();
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(move |_| Ok(vec![link_target_for_read.clone()]));

        // Is dir: /root/o/r/v1/tool -> false
        runtime
            .expect_is_dir()
            .with(eq(link_target.clone()))
            .returning(|_| false);

        // --- 2. Check Destination ---

        // File exists: /usr/local/bin/legacy-tool -> true
        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // Is symlink: /usr/local/bin/legacy-tool -> true
        runtime
            .expect_is_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // --- 3. Update Symlink ---

        // Remove old symlink
        runtime
            .expect_remove_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| Ok(()));

        // Create new symlink
        runtime
            .expect_symlink()
            .with(eq(link_target), eq(linked_to))
            .returning(|_, _| Ok(()));

        // --- Execute ---

        let result = update_external_links(
            &runtime,
            Path::new("/root/o/r"),
            &version_dir,
            &meta,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_external_links_continues_on_error() {
        // Test that update continues even when one link fails

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let linked_to1 = PathBuf::from("/usr/local/bin/tool1");   // Will fail
        let linked_to2 = PathBuf::from("/usr/local/bin/tool2");   // Will succeed
        let version_dir = PathBuf::from("/root/o/r/v1");
        let link_target = PathBuf::from("/root/o/r/v1/tool");

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            links: vec![
                LinkRule {
                    dest: linked_to1.clone(),
                    path: Some("nonexistent".to_string()),        // Will fail - path doesn't exist
                },
                LinkRule {
                    dest: linked_to2.clone(),
                    path: None,                                   // Will succeed
                },
            ],
            ..Default::default()
        };

        // --- First Link FAILS ---

        // File exists: /root/o/r/v1/nonexistent -> false
        runtime
            .expect_exists()
            .with(eq(version_dir.join("nonexistent")))
            .returning(|_| false);

        // --- Second Link SUCCEEDS ---

        // Read dir /root/o/r/v1 -> [tool]
        let link_target_for_read = link_target.clone();
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(move |_| Ok(vec![link_target_for_read.clone()]));

        // Is dir: /root/o/r/v1/tool -> false
        runtime
            .expect_is_dir()
            .with(eq(link_target.clone()))
            .returning(|_| false);

        // File exists: /usr/local/bin/tool2 -> true
        runtime
            .expect_exists()
            .with(eq(linked_to2.clone()))
            .returning(|_| true);

        // Is symlink: /usr/local/bin/tool2 -> true
        runtime
            .expect_is_symlink()
            .with(eq(linked_to2.clone()))
            .returning(|_| true);

        // Remove old symlink
        runtime
            .expect_remove_symlink()
            .with(eq(linked_to2.clone()))
            .returning(|_| Ok(()));

        // Create new symlink
        runtime
            .expect_symlink()
            .with(eq(link_target), eq(linked_to2))
            .returning(|_, _| Ok(()));

        // --- Execute ---

        // Should still succeed even though first link failed
        let result = update_external_links(
            &runtime,
            Path::new("/root/o/r"),
            &version_dir,
            &meta,
        );
        assert!(result.is_ok());
    }
}
