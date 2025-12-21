use anyhow::Result;
use log::{debug, info, warn};
use std::path::PathBuf;

use crate::{
    github::LinkSpec,
    package::{LinkRule, Meta},
    runtime::Runtime,
};

use super::paths::default_install_root;

/// Remove a link rule and its symlink
#[tracing::instrument(skip(runtime, install_root))]
pub fn unlink<R: Runtime>(
    runtime: R,
    repo_str: &str,
    dest: Option<PathBuf>,
    all: bool,
    install_root: Option<PathBuf>,
) -> Result<()> {
    debug!("Unlinking {} dest={:?} all={}", repo_str, dest, all);
    // Use LinkSpec to handle "owner/repo:path" format
    let spec = repo_str.parse::<LinkSpec>()?;
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(&runtime)?,
    };
    debug!("Using install root: {:?}", root);

    let package_dir = root.join(&spec.repo.owner).join(&spec.repo.repo);
    let meta_path = package_dir.join("meta.json");
    debug!("Loading meta from {:?}", meta_path);

    if !runtime.exists(&meta_path) {
        debug!("Meta file not found");
        anyhow::bail!(
            "Package {} is not installed.",
            spec.repo
        );
    }

    let mut meta = Meta::load(&runtime, &meta_path)?;
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
        let matching: Vec<_> = meta.links.iter()
            .filter(|r| r.dest == *dest_path)
            .cloned()
            .collect();
        if matching.is_empty() {
            // Try to find by partial match (filename)
            let dest_filename = dest_path.file_name().and_then(|s| s.to_str());
            debug!("No exact match, trying filename match: {:?}", dest_filename);
            meta.links.iter()
                .filter(|r| {
                    r.dest.file_name().and_then(|s| s.to_str()) == dest_filename
                })
                .cloned()
                .collect()
        } else {
            matching
        }
    } else if let Some(ref path) = spec.path {
        // Filter by path in the link rule (e.g., "bach-sh/bach:bach.sh")
        debug!("Looking for rule with path {:?}", path);
        meta.links.iter()
            .filter(|r| r.path.as_ref() == Some(path))
            .cloned()
            .collect()
    } else {
        debug!("No destination specified and --all not set");
        anyhow::bail!(
            "Please specify a destination path or use --all to remove all links.\n\
             Current link rules:\n{}",
            meta.links.iter()
                .map(|r| format!("  {:?}", r.dest))
                .collect::<Vec<_>>()
                .join("\n")
        );
    };

    if rules_to_remove.is_empty() {
        debug!("No matching rules found");
        let search_target = dest.as_ref()
            .map(|d| format!("{:?}", d))
            .or_else(|| spec.path.as_ref().map(|p| format!("path '{}'", p)))
            .unwrap_or_else(|| "unknown".to_string());
        anyhow::bail!(
            "No link rule found matching {}.\n\
             Current link rules:\n{}",
            search_target,
            meta.links.iter()
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

    for rule in &rules_to_remove {
        debug!("Processing rule: {:?}", rule);

        // Try to remove the symlink
        if runtime.exists(&rule.dest) || runtime.is_symlink(&rule.dest) {
            if runtime.is_symlink(&rule.dest) {
                debug!("Removing symlink {:?}", rule.dest);
                match runtime.remove_symlink(&rule.dest) {
                    Ok(()) => {
                        info!("Removed symlink {:?}", rule.dest);
                        println!("Removed symlink {:?}", rule.dest);
                        removed_count += 1;
                    }
                    Err(e) => {
                        warn!("Failed to remove symlink {:?}: {}", rule.dest, e);
                        eprintln!("Warning: Failed to remove symlink {:?}: {}", rule.dest, e);
                        error_count += 1;
                    }
                }
            } else {
                warn!("Path {:?} exists but is not a symlink, skipping removal", rule.dest);
                eprintln!("Warning: {:?} is not a symlink, skipping removal", rule.dest);
                error_count += 1;
            }
        } else {
            debug!("Symlink {:?} does not exist, removing rule only", rule.dest);
            println!("Symlink {:?} does not exist, removing rule only", rule.dest);
            removed_count += 1;
        }

        // Remove the rule from meta
        meta.links.retain(|r| r.dest != rule.dest);
        debug!("Removed rule from meta, {} rules remaining", meta.links.len());
    }

    // Save updated meta
    debug!("Saving updated meta with {} rules", meta.links.len());
    let json = serde_json::to_string_pretty(&meta)?;
    let tmp_path = meta_path.with_extension("json.tmp");
    runtime.write(&tmp_path, json.as_bytes())?;
    runtime.rename(&tmp_path, &meta_path)?;
    info!("Saved updated meta.json");

    println!(
        "Unlinked {} rule(s) from {}{}",
        removed_count,
        spec.repo,
        if error_count > 0 { format!(" ({} error(s))", error_count) } else { String::new() }
    );

    Ok(())
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
    fn test_unlink_removes_single_rule() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta with one link rule
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

        // Symlink exists
        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // Remove symlink
        runtime
            .expect_remove_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| Ok(()));

        // Save meta
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = unlink(runtime, "owner/repo", Some(link_dest), false, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_unlink_all_rules() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let link1 = PathBuf::from("/usr/local/bin/tool1");
        let link2 = PathBuf::from("/usr/local/bin/tool2");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta with two link rules
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![
                LinkRule { dest: link1.clone(), path: None },
                LinkRule { dest: link2.clone(), path: Some("tool2".into()) },
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Both symlinks exist
        runtime
            .expect_exists()
            .with(eq(link1.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(link1.clone()))
            .returning(|_| true);
        runtime
            .expect_remove_symlink()
            .with(eq(link1.clone()))
            .returning(|_| Ok(()));

        runtime
            .expect_exists()
            .with(eq(link2.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(link2.clone()))
            .returning(|_| true);
        runtime
            .expect_remove_symlink()
            .with(eq(link2.clone()))
            .returning(|_| Ok(()));

        // Save meta
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = unlink(runtime, "owner/repo", None, true, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_unlink_nonexistent_symlink_removes_rule() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta
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

        // Symlink does not exist
        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| false);

        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| false);

        // Save meta (rule should still be removed)
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let result = unlink(runtime, "owner/repo", Some(link_dest), false, Some(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_unlink_no_matching_rule_fails() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");
        let existing_link = PathBuf::from("/usr/local/bin/tool");
        let nonexistent_link = PathBuf::from("/other/path/different-tool");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta with different link
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

        let result = unlink(runtime, "owner/repo", Some(nonexistent_link), false, Some(root));
        assert!(result.is_err());
    }

    #[test]
    fn test_unlink_requires_dest_or_all() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta
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

        // Neither dest nor all specified
        let result = unlink(runtime, "owner/repo", None, false, Some(root));
        assert!(result.is_err());
    }

    #[test]
    fn test_unlink_empty_links() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = PathBuf::from("/home/user/.ghri");

        // Package exists
        runtime
            .expect_exists()
            .with(eq(root.join("owner/repo/meta.json")))
            .returning(|_| true);

        // Load meta with no links
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        let result = unlink(runtime, "owner/repo", None, true, Some(root));
        assert!(result.is_ok()); // Should succeed with message "No link rules"
    }
}
