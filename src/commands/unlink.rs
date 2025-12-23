use anyhow::Result;
use log::{debug, info, warn};
use std::path::PathBuf;

use crate::{
    github::LinkSpec,
    package::{LinkManager, LinkRule, PackageRepository, RemoveLinkResult},
    runtime::Runtime,
};

use super::config::{Config, ConfigOverrides};

/// Remove a link rule and its symlink
#[tracing::instrument(skip(runtime, overrides))]
pub fn unlink<R: Runtime>(
    runtime: R,
    repo_str: &str,
    dest: Option<PathBuf>,
    all: bool,
    overrides: ConfigOverrides,
) -> Result<()> {
    debug!("Unlinking {} dest={:?} all={}", repo_str, dest, all);
    // Use LinkSpec to handle "owner/repo:path" format
    let spec = repo_str.parse::<LinkSpec>()?;
    let config = Config::load(&runtime, overrides)?;
    debug!("Using install root: {:?}", config.install_root);

    let pkg_repo = PackageRepository::new(&runtime, config.install_root);
    let link_mgr = LinkManager::new(&runtime);
    let owner = &spec.repo.owner;
    let repo = &spec.repo.repo;

    if !pkg_repo.is_installed(owner, repo) {
        debug!("Package not installed");
        anyhow::bail!("Package {} is not installed.", spec.repo);
    }

    let mut meta = pkg_repo.load_required(owner, repo)?;
    debug!("Found {} link rules before unlink", meta.links.len());

    if meta.links.is_empty() {
        println!("No link rules for {}.", spec.repo);
        return Ok(());
    }

    // Determine which rules to remove
    let rules_to_remove: Vec<LinkRule> = if all {
        debug!("Removing all link rules");
        meta.links.clone()
    } else if let Some(ref dest_path) = dest {
        debug!("Looking for rule with dest {:?}", dest_path);
        // Find rules matching the destination
        let matching: Vec<_> = meta
            .links
            .iter()
            .filter(|r| r.dest == *dest_path)
            .cloned()
            .collect();
        if matching.is_empty() {
            // Try to find by partial match (filename)
            let dest_filename = dest_path.file_name().and_then(|s| s.to_str());
            debug!("No exact match, trying filename match: {:?}", dest_filename);
            meta.links
                .iter()
                .filter(|r| r.dest.file_name().and_then(|s| s.to_str()) == dest_filename)
                .cloned()
                .collect()
        } else {
            matching
        }
    } else if let Some(ref path) = spec.path {
        // Filter by path in the link rule (e.g., "bach-sh/bach:bach.sh")
        debug!("Looking for rule with path {:?}", path);
        meta.links
            .iter()
            .filter(|r| r.path.as_ref() == Some(path))
            .cloned()
            .collect()
    } else {
        debug!("No destination specified and --all not set");
        anyhow::bail!(
            "Please specify a destination path or use --all to remove all links.\n\
             Current link rules:\n{}",
            meta.links
                .iter()
                .map(|r| format!("  {:?}", r.dest))
                .collect::<Vec<_>>()
                .join("\n")
        );
    };

    if rules_to_remove.is_empty() {
        debug!("No matching rules found");
        let search_target = dest
            .as_ref()
            .map(|d| format!("{:?}", d))
            .or_else(|| spec.path.as_ref().map(|p| format!("path '{}'", p)))
            .unwrap_or_else(|| "unknown".to_string());
        anyhow::bail!(
            "No link rule found matching {}.\n\
             Current link rules:\n{}",
            search_target,
            meta.links
                .iter()
                .map(|r| {
                    if let Some(ref p) = r.path {
                        format!("  {} -> {:?}", p, r.dest)
                    } else {
                        format!("  (default) -> {:?}", r.dest)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    // Remove symlinks and rules
    let mut removed_count = 0;
    let mut error_count = 0;
    let mut skipped_external = Vec::new();
    let package_dir = pkg_repo.package_dir(owner, repo);

    for rule in &rules_to_remove {
        debug!("Processing rule: {:?}", rule);

        // Try to safely remove the symlink
        match link_mgr.remove_link_safely(&rule.dest, &package_dir)? {
            RemoveLinkResult::Removed => {
                info!("Removed symlink {:?}", rule.dest);
                println!("Removed symlink {:?}", rule.dest);
                removed_count += 1;
            }
            RemoveLinkResult::NotExists => {
                debug!("Symlink {:?} does not exist, removing rule only", rule.dest);
                println!("Symlink {:?} does not exist, removing rule only", rule.dest);
                removed_count += 1;
            }
            RemoveLinkResult::NotSymlink => {
                warn!(
                    "Path {:?} exists but is not a symlink, skipping removal",
                    rule.dest
                );
                eprintln!(
                    "Warning: {:?} is not a symlink, skipping removal",
                    rule.dest
                );
                error_count += 1;
                continue; // Don't remove this rule from meta
            }
            RemoveLinkResult::ExternalTarget => {
                if all {
                    // When using --all, skip this symlink and continue
                    warn!(
                        "Skipping symlink {:?}: points outside package directory {:?}",
                        rule.dest, package_dir
                    );
                    eprintln!(
                        "Warning: Skipping {:?} - points to external path",
                        rule.dest
                    );
                    skipped_external.push(rule.dest.clone());
                    error_count += 1;
                    continue; // Don't remove this rule from meta
                } else {
                    // When specifying a single destination, fail with error
                    anyhow::bail!(
                        "Cannot remove symlink {:?}: it points to external path which is outside the package directory {:?}",
                        rule.dest,
                        package_dir
                    );
                }
            }
            RemoveLinkResult::Unresolvable => {
                if all {
                    warn!(
                        "Cannot resolve symlink target for {:?}, skipping",
                        rule.dest
                    );
                    eprintln!("Warning: Cannot verify symlink {:?}, skipping", rule.dest);
                    error_count += 1;
                    continue; // Don't remove this rule from meta
                } else {
                    anyhow::bail!(
                        "Cannot remove symlink {:?}: unable to resolve target",
                        rule.dest
                    );
                }
            }
        }

        // Remove the rule from meta
        meta.links.retain(|r| r.dest != rule.dest);
        debug!(
            "Removed rule from meta, {} rules remaining",
            meta.links.len()
        );
    }

    // Save updated meta
    debug!("Saving updated meta with {} rules", meta.links.len());
    pkg_repo.save(owner, repo, &meta)?;
    info!("Saved updated meta.json");

    println!(
        "Unlinked {} rule(s) from {}{}",
        removed_count,
        spec.repo,
        if error_count > 0 {
            format!(" ({} error(s))", error_count)
        } else {
            String::new()
        }
    );

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
    fn test_unlink_removes_single_rule() {
        // Test that unlink removes a single symlink and its rule from meta.json

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // --- 1. is_installed + load ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: has one link rule pointing to /usr/local/bin/tool
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: link_dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Check Symlink Status ---
        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // --- 3. Verify Symlink Target (Security Check) ---
        let target = root.join("owner/repo/v1/tool");
        runtime
            .expect_resolve_link()
            .with(eq(link_dest.clone()))
            .returning(move |_| Ok(target.clone()));

        // --- 4. Remove Symlink (LinkManager checks is_symlink again) ---
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        runtime
            .expect_remove_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| Ok(()));

        // --- 5. Save Updated Metadata (save checks package_dir exists) ---
        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| true);

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = unlink(
            runtime,
            "owner/repo",
            Some(link_dest),
            false,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_unlink_all_rules() {
        // Test that unlink --all removes all symlinks and rules

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let link1 = PathBuf::from("/usr/local/bin/tool1");
        let link2 = PathBuf::from("/usr/local/bin/tool2");

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
            links: vec![
                LinkRule {
                    dest: link1.clone(),
                    path: None,
                },
                LinkRule {
                    dest: link2.clone(),
                    path: Some("tool2".into()),
                },
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Remove First Symlink: /usr/local/bin/tool1 ---
        runtime
            .expect_exists()
            .with(eq(link1.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(link1.clone()))
            .returning(|_| true);

        let target1 = root.join("owner/repo/v1/tool1");
        runtime
            .expect_resolve_link()
            .with(eq(link1.clone()))
            .returning(move |_| Ok(target1.clone()));

        // LinkManager::remove_link checks is_symlink
        runtime
            .expect_is_symlink()
            .with(eq(link1.clone()))
            .returning(|_| true);

        runtime
            .expect_remove_symlink()
            .with(eq(link1.clone()))
            .returning(|_| Ok(()));

        // --- 3. Remove Second Symlink: /usr/local/bin/tool2 ---
        runtime
            .expect_exists()
            .with(eq(link2.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(link2.clone()))
            .returning(|_| true);

        let target2 = root.join("owner/repo/v1/tool2");
        runtime
            .expect_resolve_link()
            .with(eq(link2.clone()))
            .returning(move |_| Ok(target2.clone()));

        // LinkManager::remove_link checks is_symlink
        runtime
            .expect_is_symlink()
            .with(eq(link2.clone()))
            .returning(|_| true);

        runtime
            .expect_remove_symlink()
            .with(eq(link2.clone()))
            .returning(|_| Ok(()));

        // --- 4. Save Updated Metadata ---
        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| true);

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = unlink(
            runtime,
            "owner/repo",
            None,
            true,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_unlink_nonexistent_symlink_removes_rule() {
        // Test that unlink removes the rule even if the symlink file doesn't exist

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");
        let meta_path = package_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // --- 1. is_installed + load ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json: has one link rule
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: link_dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Check Symlink Status ---
        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| false);

        // (No remove_symlink call since file doesn't exist)

        // --- 3. Save Updated Metadata ---
        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| true);

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---
        let result = unlink(
            runtime,
            "owner/repo",
            Some(link_dest),
            false,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_unlink_no_matching_rule_fails() {
        // Test that unlink fails when the specified destination doesn't match any rule

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json"); // /home/user/.ghri/owner/repo/meta.json
        let existing_link = PathBuf::from("/usr/local/bin/tool");
        let nonexistent_link = PathBuf::from("/other/path/different-tool");

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| true);

        // Read meta.json: has one rule for /usr/local/bin/tool (different from requested)
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: existing_link.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- Execute & Verify ---

        // Request unlink for /other/path/different-tool which doesn't exist in rules
        let result = unlink(
            runtime,
            "owner/repo",
            Some(nonexistent_link),
            false,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_unlink_requires_dest_or_all() {
        // Test that unlink fails when neither destination nor --all is specified

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json"); // /home/user/.ghri/owner/repo/meta.json

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| true);

        // Read meta.json: has one link rule
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: PathBuf::from("/usr/local/bin/tool"),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- Execute & Verify ---

        // Neither dest nor --all specified -> should fail
        let result = unlink(
            runtime,
            "owner/repo",
            None,
            false,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_unlink_empty_links() {
        // Test that unlink with --all succeeds gracefully when there are no link rules

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("owner/repo/meta.json"); // /home/user/.ghri/owner/repo/meta.json

        // --- 1. Load Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| true);

        // Read meta.json: has NO link rules (empty)
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![], // Empty!
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- Execute & Verify ---

        // Should succeed with message "No link rules" (no error)
        let result = unlink(
            runtime,
            "owner/repo",
            None,
            true,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_unlink_should_fail_if_symlink_points_to_external_path() {
        // Security test: unlink should NOT remove a symlink if it points to a path
        // outside of the ghri managed directory (install_root).
        // This prevents accidentally deleting important system symlinks.

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---

        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");
        let link_dest = PathBuf::from("/usr/local/bin/tool");
        let meta_path = package_dir.join("meta.json");

        // The symlink exists but points to an external path (NOT under /home/user/.ghri)
        let external_target = PathBuf::from("/opt/external/tool");

        // --- 1. Load Package Metadata ---

        // File exists: /home/user/.ghri/owner/repo/meta.json
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read Meta: Has a link rule for /usr/local/bin/tool
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(), // Set version to avoid read_link call in Meta::load
            links: vec![LinkRule {
                dest: link_dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Check Symlink Status ---

        // File exists: /usr/local/bin/tool
        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // Is Symlink: /usr/local/bin/tool -> true
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // --- 3. Validate Symlink Target (Security Check) ---

        // Resolve Link: /usr/local/bin/tool -> /opt/external/tool
        // This points OUTSIDE the ghri install root (/home/user/.ghri)!
        runtime
            .expect_resolve_link()
            .with(eq(link_dest.clone()))
            .returning(move |_| Ok(external_target.clone()));

        // --- Expected: Should NOT call remove_symlink ---
        // --- Expected: Should fail with an error message ---

        // No remove_symlink call expected (symlink points to external path)
        // No write/rename calls expected (meta.json should not be updated)

        // --- Execute & Verify ---

        let result = unlink(
            runtime,
            "owner/repo",
            Some(link_dest),
            false,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );

        // Should fail because symlink points to external path
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("external") || err_msg.contains("outside"),
            "Error message should mention external path: {}",
            err_msg
        );
    }

    #[test]
    fn test_unlink_all_should_skip_symlinks_pointing_to_external_paths() {
        // Security test: unlink --all should skip symlinks that point to external paths
        // and only remove symlinks that point to paths within the ghri managed directory.

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");
        let meta_path = package_dir.join("meta.json");

        // Two link destinations
        let internal_link = PathBuf::from("/usr/local/bin/internal-tool");
        let external_link = PathBuf::from("/usr/local/bin/external-tool");

        // Internal symlink points to ghri managed path
        let internal_target = package_dir.join("v1/internal-tool");
        // External symlink points outside ghri
        let external_target = PathBuf::from("/opt/other/external-tool");

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
            links: vec![
                LinkRule {
                    dest: internal_link.clone(),
                    path: None,
                },
                LinkRule {
                    dest: external_link.clone(),
                    path: None,
                },
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // --- 2. Check First Symlink (Internal - Should Be Removed) ---
        runtime
            .expect_exists()
            .with(eq(internal_link.clone()))
            .returning(|_| true);

        runtime
            .expect_is_symlink()
            .with(eq(internal_link.clone()))
            .returning(|_| true);

        runtime
            .expect_resolve_link()
            .with(eq(internal_link.clone()))
            .returning(move |_| Ok(internal_target.clone()));

        // LinkManager::remove_link checks is_symlink
        runtime
            .expect_is_symlink()
            .with(eq(internal_link.clone()))
            .returning(|_| true);

        runtime
            .expect_remove_symlink()
            .with(eq(internal_link.clone()))
            .returning(|_| Ok(()));

        // --- 3. Check Second Symlink (External - Should Be Skipped) ---
        runtime
            .expect_exists()
            .with(eq(external_link.clone()))
            .returning(|_| true);

        runtime
            .expect_is_symlink()
            .with(eq(external_link.clone()))
            .returning(|_| true);

        runtime
            .expect_resolve_link()
            .with(eq(external_link.clone()))
            .returning(move |_| Ok(external_target.clone()));

        // NO remove_symlink call for external_link (should be skipped)

        // --- 4. Update Meta (Only Internal Rule Removed) ---
        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| true);

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute & Verify ---
        let result = unlink(
            runtime,
            "owner/repo",
            None,
            true,
            ConfigOverrides {
                install_root: Some(root),
                ..Default::default()
            },
        );

        // Should succeed but with warning about skipped external symlink
        assert!(result.is_ok());
    }
}
