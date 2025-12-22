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
        let runtime = MockRuntime::new();
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            ..Default::default()
        };

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
        let mut runtime = MockRuntime::new();
        let linked_to = PathBuf::from("/usr/local/bin/tool");
        let version_dir = PathBuf::from("/root/o/r/v2");

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v2".into(),
            links: vec![LinkRule {
                dest: linked_to.clone(),
                path: None,
            }],
            ..Default::default()
        };

        // Check if linked_to exists
        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // Check if linked_to is symlink
        runtime
            .expect_is_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // Remove old symlink
        runtime
            .expect_remove_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| Ok(()));

        // Read version dir to determine link target
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("/root/o/r/v2/tool")]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/o/r/v2/tool")))
            .returning(|_| false);

        // Create new symlink
        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("/root/o/r/v2/tool")), eq(linked_to.clone()))
            .returning(|_, _| Ok(()));

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
        let mut runtime = MockRuntime::new();
        let linked_to = PathBuf::from("/usr/local/bin/mytool");
        let version_dir = PathBuf::from("/root/o/r/v1");
        let target_path = version_dir.join("bin/tool");

        let rule = LinkRule {
            dest: linked_to.clone(),
            path: Some("bin/tool".to_string()),
        };

        // Check if the specific path exists
        runtime
            .expect_exists()
            .with(eq(target_path.clone()))
            .returning(|_| true);

        // Check if linked_to exists
        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| false);

        // Check if linked_to is symlink (for broken symlink check)
        runtime
            .expect_is_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| false);

        // Parent exists
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/usr/local/bin")))
            .returning(|_| true);

        // Create symlink
        runtime
            .expect_symlink()
            .with(eq(target_path), eq(linked_to))
            .returning(|_, _| Ok(()));

        let result = update_single_link(&runtime, &version_dir, &rule);
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_single_link_path_not_exists() {
        let mut runtime = MockRuntime::new();
        let linked_to = PathBuf::from("/usr/local/bin/mytool");
        let version_dir = PathBuf::from("/root/o/r/v1");
        let target_path = version_dir.join("bin/nonexistent");

        let rule = LinkRule {
            dest: linked_to,
            path: Some("bin/nonexistent".to_string()),
        };

        // Check if the specific path exists - it doesn't
        runtime
            .expect_exists()
            .with(eq(target_path))
            .returning(|_| false);

        let result = update_single_link(&runtime, &version_dir, &rule);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_update_single_link_target_not_symlink() {
        let mut runtime = MockRuntime::new();
        let linked_to = PathBuf::from("/usr/local/bin/tool");
        let version_dir = PathBuf::from("/root/o/r/v1");

        let rule = LinkRule {
            dest: linked_to.clone(),
            path: None,
        };

        // Read version dir to determine link target
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("/root/o/r/v1/tool")]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/o/r/v1/tool")))
            .returning(|_| false);

        // linked_to exists
        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // linked_to is NOT a symlink (it's a regular file)
        runtime
            .expect_is_symlink()
            .with(eq(linked_to))
            .returning(|_| false);

        // Should succeed but skip the update (logs warning)
        let result = update_single_link(&runtime, &version_dir, &rule);
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_single_link_create_parent_dir() {
        let mut runtime = MockRuntime::new();
        let linked_to = PathBuf::from("/new/path/bin/tool");
        let version_dir = PathBuf::from("/root/o/r/v1");

        let rule = LinkRule {
            dest: linked_to.clone(),
            path: None,
        };

        // Read version dir to determine link target
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("/root/o/r/v1/tool")]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/o/r/v1/tool")))
            .returning(|_| false);

        // linked_to doesn't exist
        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| false);

        // linked_to is not a symlink
        runtime
            .expect_is_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| false);

        // Parent doesn't exist
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/new/path/bin")))
            .returning(|_| false);

        // Create parent directory
        runtime
            .expect_create_dir_all()
            .with(eq(PathBuf::from("/new/path/bin")))
            .returning(|_| Ok(()));

        // Create symlink
        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("/root/o/r/v1/tool")), eq(linked_to))
            .returning(|_, _| Ok(()));

        let result = update_single_link(&runtime, &version_dir, &rule);
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_external_links_with_legacy_linked_to() {
        let mut runtime = MockRuntime::new();
        let linked_to = PathBuf::from("/usr/local/bin/legacy-tool");
        let version_dir = PathBuf::from("/root/o/r/v1");

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            linked_to: Some(linked_to.clone()),
            linked_path: None,
            ..Default::default()
        };

        // For legacy link
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("/root/o/r/v1/tool")]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/o/r/v1/tool")))
            .returning(|_| false);

        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        runtime
            .expect_is_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        runtime
            .expect_remove_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| Ok(()));

        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("/root/o/r/v1/tool")), eq(linked_to))
            .returning(|_, _| Ok(()));

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
        let mut runtime = MockRuntime::new();
        let linked_to1 = PathBuf::from("/usr/local/bin/tool1");
        let linked_to2 = PathBuf::from("/usr/local/bin/tool2");
        let version_dir = PathBuf::from("/root/o/r/v1");

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            links: vec![
                LinkRule {
                    dest: linked_to1.clone(),
                    path: Some("nonexistent".to_string()),
                },
                LinkRule {
                    dest: linked_to2.clone(),
                    path: None,
                },
            ],
            ..Default::default()
        };

        // First link fails - path doesn't exist
        runtime
            .expect_exists()
            .with(eq(version_dir.join("nonexistent")))
            .returning(|_| false);

        // Second link succeeds
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("/root/o/r/v1/tool")]));

        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/root/o/r/v1/tool")))
            .returning(|_| false);

        runtime
            .expect_exists()
            .with(eq(linked_to2.clone()))
            .returning(|_| true);

        runtime
            .expect_is_symlink()
            .with(eq(linked_to2.clone()))
            .returning(|_| true);

        runtime
            .expect_remove_symlink()
            .with(eq(linked_to2.clone()))
            .returning(|_| Ok(()));

        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("/root/o/r/v1/tool")), eq(linked_to2))
            .returning(|_, _| Ok(()));

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
