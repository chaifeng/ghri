use anyhow::Result;
use log::{debug, info, warn};
use std::path::{Path, PathBuf};

use crate::{
    package::{LinkRule, Meta},
    runtime::{is_path_under, Runtime},
};

use crate::commands::determine_link_target;

/// Describes a validated link operation ready to be executed
#[derive(Debug)]
struct ValidatedLink {
    /// The link target (source file in version directory)
    link_target: PathBuf,
    /// The destination path (symlink location)
    dest: PathBuf,
    /// Whether the destination already exists and needs to be removed first
    needs_removal: bool,
    /// Whether the parent directory needs to be created
    needs_parent_dir: bool,
}

/// The result of validating a single link
enum LinkValidation {
    /// Link is valid and ready to be created/updated
    Valid(ValidatedLink),
    /// Link should be skipped (e.g., destination is not a symlink, or points to external path)
    Skip { dest: PathBuf, reason: String },
    /// Link validation failed with an error
    Error { dest: PathBuf, error: anyhow::Error },
}

/// Update external links for a package after installation
/// Uses atomic approach: first validate all links, then execute all updates
#[tracing::instrument(skip(runtime, package_dir, version_dir))]
pub(crate) fn update_external_links<R: Runtime>(
    runtime: &R,
    package_dir: &Path,
    version_dir: &Path,
    meta: &Meta,
) -> Result<()> {
    // Collect all rules to process (including legacy linked_to)
    let mut all_rules: Vec<LinkRule> = meta.links.clone();
    if let Some(ref linked_to) = meta.linked_to {
        all_rules.push(LinkRule {
            dest: linked_to.clone(),
            path: meta.linked_path.clone(),
        });
    }

    if all_rules.is_empty() {
        return Ok(());
    }

    // --- Phase 1: Validate all links ---
    let mut validated_links: Vec<ValidatedLink> = Vec::new();
    let mut skipped: Vec<(PathBuf, String)> = Vec::new();
    let mut errors: Vec<(PathBuf, anyhow::Error)> = Vec::new();

    for rule in &all_rules {
        match validate_single_link(runtime, package_dir, version_dir, rule) {
            LinkValidation::Valid(validated) => {
                validated_links.push(validated);
            }
            LinkValidation::Skip { dest, reason } => {
                debug!("Skipping link {:?}: {}", dest, reason);
                eprintln!("Warning: Skipping {:?} - {}", dest, reason);
                skipped.push((dest, reason));
            }
            LinkValidation::Error { dest, error } => {
                eprintln!("Error validating link {:?}: {}", dest, error);
                errors.push((dest, error));
            }
        }
    }

    // If there are validation errors, fail before making any changes
    if !errors.is_empty() {
        let error_msgs: Vec<String> = errors
            .iter()
            .map(|(dest, e)| format!("{:?}: {}", dest, e))
            .collect();
        anyhow::bail!(
            "Link validation failed for {} link(s):\n  {}",
            errors.len(),
            error_msgs.join("\n  ")
        );
    }

    // --- Phase 2: Execute all validated link updates ---
    for validated in &validated_links {
        if let Err(e) = execute_link_update(runtime, validated) {
            // This shouldn't happen after validation, but handle it gracefully
            anyhow::bail!(
                "Failed to update link {:?} -> {:?}: {}",
                validated.dest,
                validated.link_target,
                e
            );
        }
    }

    if !skipped.is_empty() {
        warn!("{} link(s) were skipped", skipped.len());
    }

    Ok(())
}

/// Validate a single link rule without making any changes
fn validate_single_link<R: Runtime>(
    runtime: &R,
    package_dir: &Path,
    version_dir: &Path,
    rule: &LinkRule,
) -> LinkValidation {
    let linked_to = &rule.dest;

    // Determine link target based on rule.path or default behavior
    let link_target = match determine_link_target_for_rule(runtime, version_dir, rule) {
        Ok(target) => target,
        Err(e) => {
            return LinkValidation::Error {
                dest: linked_to.clone(),
                error: e,
            };
        }
    };

    // Check destination status
    let dest_exists = runtime.exists(linked_to);
    let is_symlink = runtime.is_symlink(linked_to);

    if dest_exists || is_symlink {
        if is_symlink {
            // Security check: verify the symlink points to a path within the package directory
            match runtime.resolve_link(linked_to) {
                Ok(existing_target) => {
                    if !is_path_under(&existing_target, package_dir) {
                        // Symlink points outside the package directory - skip with warning
                        return LinkValidation::Skip {
                            dest: linked_to.clone(),
                            reason: format!(
                                "points to external path {:?} (outside {:?})",
                                existing_target, package_dir
                            ),
                        };
                    }
                    debug!(
                        "Existing symlink {:?} -> {:?} is within package directory",
                        linked_to, existing_target
                    );
                }
                Err(e) => {
                    return LinkValidation::Skip {
                        dest: linked_to.clone(),
                        reason: format!("cannot resolve symlink target: {}", e),
                    };
                }
            }

            // Valid: needs removal before creating new symlink
            return LinkValidation::Valid(ValidatedLink {
                link_target,
                dest: linked_to.clone(),
                needs_removal: true,
                needs_parent_dir: false,
            });
        } else {
            // Destination exists but is not a symlink - skip with warning
            return LinkValidation::Skip {
                dest: linked_to.clone(),
                reason: "exists but is not a symlink".to_string(),
            };
        }
    }

    // Destination doesn't exist - check if parent directory exists
    let needs_parent_dir = if let Some(parent) = linked_to.parent() {
        !runtime.exists(parent)
    } else {
        false
    };

    LinkValidation::Valid(ValidatedLink {
        link_target,
        dest: linked_to.clone(),
        needs_removal: false,
        needs_parent_dir,
    })
}

/// Determine the link target for a rule
fn determine_link_target_for_rule<R: Runtime>(
    runtime: &R,
    version_dir: &Path,
    rule: &LinkRule,
) -> Result<PathBuf> {
    if let Some(ref path) = rule.path {
        let target = version_dir.join(path);
        if !runtime.exists(&target) {
            anyhow::bail!("Path '{}' does not exist in {:?}", path, version_dir);
        }
        Ok(target)
    } else {
        determine_link_target(runtime, version_dir)
    }
}

/// Execute a validated link update
fn execute_link_update<R: Runtime>(runtime: &R, validated: &ValidatedLink) -> Result<()> {
    // Create parent directory if needed
    if validated.needs_parent_dir {
        if let Some(parent) = validated.dest.parent() {
            runtime.create_dir_all(parent)?;
        }
    }

    // Remove existing symlink if needed
    if validated.needs_removal {
        runtime.remove_symlink(&validated.dest)?;
    }

    // Create new symlink
    runtime.symlink(&validated.link_target, &validated.dest)?;
    info!(
        "Updated external link {:?} -> {:?}",
        validated.dest, validated.link_target
    );

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
        let package_dir = PathBuf::from("/root/o/r");
        let linked_to = PathBuf::from("/usr/local/bin/tool");     // External symlink location
        let version_dir = PathBuf::from("/root/o/r/v2");          // New version directory
        let link_target = PathBuf::from("/root/o/r/v2/tool");     // Target binary in new version
        let old_target = PathBuf::from("/root/o/r/v1/tool");      // Old target (within package)

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v2".into(),
            links: vec![LinkRule {
                dest: linked_to.clone(),
                path: None,
            }],
            ..Default::default()
        };

        // --- 1. Determine Link Target ---

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

        // --- 2. Check if Destination Exists ---

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

        // --- 3. Verify Symlink Target (Security Check) ---

        // Resolve link: /usr/local/bin/tool -> /root/o/r/v1/tool
        // (Points to old version within package directory - allowed)
        runtime
            .expect_resolve_link()
            .with(eq(linked_to.clone()))
            .returning(move |_| Ok(old_target.clone()));

        // --- 4. Remove Old Symlink ---

        // Remove symlink: /usr/local/bin/tool
        runtime
            .expect_remove_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| Ok(()));

        // --- 5. Create New Symlink ---

        // Create symlink: /usr/local/bin/tool -> /root/o/r/v2/tool
        runtime
            .expect_symlink()
            .with(eq(link_target), eq(linked_to.clone()))
            .returning(|_, _| Ok(()));

        // --- Execute ---

        let result = update_external_links(
            &runtime,
            &package_dir,
            &version_dir,
            &meta,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_external_links_with_specific_path() {
        // Test creating a link with an explicit path specified in the rule

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let package_dir = PathBuf::from("/root/o/r");
        let version_dir = PathBuf::from("/root/o/r/v1");
        let linked_to = PathBuf::from("/usr/local/bin/mytool");   // External symlink location
        let target_path = version_dir.join("bin/tool");           // /root/o/r/v1/bin/tool

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: linked_to.clone(),
                path: Some("bin/tool".to_string()),               // Explicit path!
            }],
            ..Default::default()
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

        let result = update_external_links(&runtime, &package_dir, &version_dir, &meta);
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_external_links_path_not_exists() {
        // Test that link creation fails when the specified path doesn't exist

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let package_dir = PathBuf::from("/root/o/r");
        let version_dir = PathBuf::from("/root/o/r/v1");
        let linked_to = PathBuf::from("/usr/local/bin/mytool");
        let target_path = version_dir.join("bin/nonexistent");    // /root/o/r/v1/bin/nonexistent

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: linked_to,
                path: Some("bin/nonexistent".to_string()),        // Path doesn't exist!
            }],
            ..Default::default()
        };

        // --- 1. Check if Explicit Path Exists ---

        // File exists: /root/o/r/v1/bin/nonexistent -> false
        runtime
            .expect_exists()
            .with(eq(target_path))
            .returning(|_| false);

        // --- Execute & Verify ---

        let result = update_external_links(&runtime, &package_dir, &version_dir, &meta);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_update_external_links_target_not_symlink() {
        // Test that update is skipped when destination is a regular file (not symlink)

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let package_dir = PathBuf::from("/root/o/r");
        let version_dir = PathBuf::from("/root/o/r/v1");
        let linked_to = PathBuf::from("/usr/local/bin/tool");

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: linked_to.clone(),
                path: None,
            }],
            ..Default::default()
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
        let result = update_external_links(&runtime, &package_dir, &version_dir, &meta);
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_external_links_create_parent_dir() {
        // Test that parent directory is created when it doesn't exist

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let package_dir = PathBuf::from("/root/o/r");
        let version_dir = PathBuf::from("/root/o/r/v1");
        let linked_to = PathBuf::from("/new/path/bin/tool");
        let parent_dir = PathBuf::from("/new/path/bin");
        let link_target = PathBuf::from("/root/o/r/v1/tool");

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: linked_to.clone(),
                path: None,
            }],
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

        // --- 3. Check Parent Directory ---

        // Parent exists: /new/path/bin -> false
        runtime
            .expect_exists()
            .with(eq(parent_dir.clone()))
            .returning(|_| false);

        // --- 4. Create Parent Directory ---

        // Create parent dir: /new/path/bin
        runtime
            .expect_create_dir_all()
            .with(eq(parent_dir))
            .returning(|_| Ok(()));

        // --- 5. Create Symlink ---

        // Create symlink: /new/path/bin/tool -> /root/o/r/v1/tool
        runtime
            .expect_symlink()
            .with(eq(link_target), eq(linked_to))
            .returning(|_, _| Ok(()));

        // --- Execute ---

        let result = update_external_links(&runtime, &package_dir, &version_dir, &meta);
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

        // --- 3. Verify Symlink Target (Security Check) ---

        // Resolve link: /usr/local/bin/legacy-tool -> /root/o/r/v1/tool
        // (Points to old version within package directory - allowed)
        let old_target = PathBuf::from("/root/o/r/v1/tool");
        runtime
            .expect_resolve_link()
            .with(eq(linked_to.clone()))
            .returning(move |_| Ok(old_target.clone()));

        // --- 4. Update Symlink ---

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
    fn test_update_external_links_fails_atomically_on_validation_error() {
        // Test that update fails atomically when one link validation fails
        // (no changes should be made if any validation fails)

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let package_dir = PathBuf::from("/root/o/r");
        let version_dir = PathBuf::from("/root/o/r/v1");
        let linked_to1 = PathBuf::from("/usr/local/bin/tool1");   // Will succeed validation
        let linked_to2 = PathBuf::from("/usr/local/bin/tool2");   // Will fail validation - path doesn't exist
        let link_target = PathBuf::from("/root/o/r/v1/tool");

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            links: vec![
                LinkRule {
                    dest: linked_to1.clone(),
                    path: None,                                   // Valid - uses default path
                },
                LinkRule {
                    dest: linked_to2.clone(),
                    path: Some("nonexistent".to_string()),        // Invalid - path doesn't exist!
                },
            ],
            ..Default::default()
        };

        // --- Validation Phase ---

        // First link: determine link target
        let link_target_for_read = link_target.clone();
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(move |_| Ok(vec![link_target_for_read.clone()]));

        runtime
            .expect_is_dir()
            .with(eq(link_target.clone()))
            .returning(|_| false);

        // First link: check destination
        runtime
            .expect_exists()
            .with(eq(linked_to1.clone()))
            .returning(|_| false);
        runtime
            .expect_is_symlink()
            .with(eq(linked_to1.clone()))
            .returning(|_| false);
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/usr/local/bin")))
            .returning(|_| true);

        // Second link: check if explicit path exists -> false (FAIL!)
        runtime
            .expect_exists()
            .with(eq(version_dir.join("nonexistent")))
            .returning(|_| false);

        // --- Expected: NO execution phase calls ---
        // No symlink creation, no remove_symlink - validation failed!

        // --- Execute & Verify ---

        let result = update_external_links(
            &runtime,
            &package_dir,
            &version_dir,
            &meta,
        );

        // Should fail because second link validation failed (path doesn't exist)
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("validation failed") || err_msg.contains("does not exist"),
            "Error should mention validation failure: {}",
            err_msg
        );
    }

    #[test]
    fn test_update_external_links_should_validate_symlink_target() {
        // Security test: update_external_links should verify that existing symlink
        // points to a path within the package directory before removing it.
        // If symlink points to an external path, it should NOT be removed.

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---

        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");
        let version_dir = package_dir.join("v2");
        let linked_to = PathBuf::from("/usr/local/bin/tool");

        // Symlink points to external path (NOT under package_dir)
        let external_target = PathBuf::from("/opt/external/other-tool");

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v2".into(),
            links: vec![LinkRule {
                dest: linked_to.clone(),
                path: None,
            }],
            ..Default::default()
        };

        // --- 1. Determine Link Target in New Version ---

        // Read Directory: /home/user/.ghri/owner/repo/v2
        let new_target = version_dir.join("tool");
        runtime
            .expect_read_dir()
            .with(eq(version_dir.clone()))
            .returning(move |_| Ok(vec![new_target.clone()]));

        // Is Directory: /home/user/.ghri/owner/repo/v2/tool -> false
        runtime
            .expect_is_dir()
            .returning(|_| false);

        // --- 2. Check Destination Symlink ---

        // File exists: /usr/local/bin/tool
        runtime
            .expect_exists()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // Is Symlink: /usr/local/bin/tool -> true
        runtime
            .expect_is_symlink()
            .with(eq(linked_to.clone()))
            .returning(|_| true);

        // --- 3. Validate Symlink Target (Security Check) ---

        // Resolve Link: /usr/local/bin/tool -> /opt/external/other-tool
        // This points OUTSIDE the package directory!
        runtime
            .expect_resolve_link()
            .with(eq(linked_to.clone()))
            .returning(move |_| Ok(external_target.clone()));

        // --- Expected: Should NOT call remove_symlink ---
        // --- Expected: Should NOT call symlink ---
        // The existing symlink points to external path, should be skipped with warning

        // No remove_symlink call expected
        // No symlink call expected

        // --- Execute & Verify ---

        let result = update_external_links(
            &runtime,
            &package_dir,
            &version_dir,
            &meta,
        );

        // Should succeed (with warning) but NOT remove the external symlink
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_external_links_atomic_multiple_links_success() {
        // Test that multiple links are updated atomically when all validations pass

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let package_dir = PathBuf::from("/root/o/r");
        let version_dir = PathBuf::from("/root/o/r/v2");
        let linked_to1 = PathBuf::from("/usr/local/bin/tool1");
        let linked_to2 = PathBuf::from("/usr/local/bin/tool2");
        let old_target1 = PathBuf::from("/root/o/r/v1/tool1");
        let old_target2 = PathBuf::from("/root/o/r/v1/tool2");
        let new_target1 = PathBuf::from("/root/o/r/v2/tool1");
        let new_target2 = PathBuf::from("/root/o/r/v2/tool2");

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v2".into(),
            links: vec![
                LinkRule {
                    dest: linked_to1.clone(),
                    path: Some("tool1".to_string()),
                },
                LinkRule {
                    dest: linked_to2.clone(),
                    path: Some("tool2".to_string()),
                },
            ],
            ..Default::default()
        };

        // --- Phase 1: Validation (all checks happen first) ---

        // First link: check source exists
        runtime
            .expect_exists()
            .with(eq(new_target1.clone()))
            .returning(|_| true);

        // First link: check destination
        runtime
            .expect_exists()
            .with(eq(linked_to1.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(linked_to1.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(linked_to1.clone()))
            .returning(move |_| Ok(old_target1.clone()));

        // Second link: check source exists
        runtime
            .expect_exists()
            .with(eq(new_target2.clone()))
            .returning(|_| true);

        // Second link: check destination
        runtime
            .expect_exists()
            .with(eq(linked_to2.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(linked_to2.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(linked_to2.clone()))
            .returning(move |_| Ok(old_target2.clone()));

        // --- Phase 2: Execution (all updates happen after validation) ---

        // First link: remove old, create new
        runtime
            .expect_remove_symlink()
            .with(eq(linked_to1.clone()))
            .returning(|_| Ok(()));
        runtime
            .expect_symlink()
            .with(eq(new_target1), eq(linked_to1))
            .returning(|_, _| Ok(()));

        // Second link: remove old, create new
        runtime
            .expect_remove_symlink()
            .with(eq(linked_to2.clone()))
            .returning(|_| Ok(()));
        runtime
            .expect_symlink()
            .with(eq(new_target2), eq(linked_to2))
            .returning(|_, _| Ok(()));

        // --- Execute ---

        let result = update_external_links(&runtime, &package_dir, &version_dir, &meta);
        assert!(result.is_ok());
    }
}
