use anyhow::Result;
use std::path::PathBuf;

use crate::application::LinkAction;
use crate::domain::service::PackageRepository;
use crate::runtime::Runtime;

use super::config::Config;
use super::link_spec::LinkSpec;

/// Remove a link rule and its symlink
#[tracing::instrument(skip(runtime, config))]
pub fn unlink<R: Runtime>(
    runtime: R,
    repo_str: &str,
    dest: Option<PathBuf>,
    all: bool,
    config: Config,
) -> Result<()> {
    // Use LinkSpec to handle "owner/repo:path" format
    let spec = repo_str.parse::<LinkSpec>()?;

    let pkg_repo = PackageRepository::new(&runtime, config.install_root.clone());
    let action = LinkAction::new(&runtime, config.install_root);

    // Load package context - version is always resolved (user-specified or current)
    let mut ctx =
        pkg_repo.load_context(&spec.repo.owner, &spec.repo.repo, spec.version.as_deref())?;

    // Delegate to LinkAction for the actual work
    let result = action.remove_package_links(&mut ctx, dest, spec.path, all)?;

    // Display result
    println!(
        "Unlinked {} rule(s) from {}{}",
        result.removed_count,
        ctx.display_name,
        if result.error_count > 0 {
            format!(" ({} error(s))", result.error_count)
        } else {
            String::new()
        }
    );

    // Show warnings for skipped external links
    for path in &result.skipped_external {
        eprintln!("Warning: Skipped {:?} - points to external path", path);
    }

    Ok(())
}

// Note: Most tests for unlink functionality have been moved to application/link.rs
// The command layer tests focus on argument parsing and user interface, not implementation details.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{LinkRule, Meta};
    use crate::runtime::MockRuntime;
    use crate::test_utils::{configure_mock_runtime_basics, test_bin_dir, test_home, test_root};
    use mockall::predicate::*;

    fn configure_runtime_basics(runtime: &mut MockRuntime) {
        configure_mock_runtime_basics(runtime);
    }

    #[test]
    fn test_unlink_empty_links() {
        // Test that unlink with --all succeeds gracefully when there are no link rules

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = test_root();
        let meta_path = root.join("owner").join("repo").join("meta.json");

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

        // Should fail because no links
        let result = unlink(runtime, "owner/repo", None, true, Config::for_test(root));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("No link rules"));
    }

    #[test]
    fn test_unlink_requires_dest_or_all() {
        // Test that unlink fails when neither destination nor --all is specified

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = test_root();
        let meta_path = root.join("owner").join("repo").join("meta.json");

        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| true);

        // Read meta.json: has one link rule
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1".into(),
            links: vec![LinkRule {
                dest: test_bin_dir().join("tool"),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Neither dest nor --all specified -> should fail
        let result = unlink(runtime, "owner/repo", None, false, Config::for_test(root));
        assert!(result.is_err());
    }

    #[test]
    fn test_unlink_no_matching_rule_fails() {
        // Test that unlink fails when the specified destination doesn't match any rule

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = test_root();
        let meta_path = root.join("owner").join("repo").join("meta.json");
        let existing_link = test_bin_dir().join("tool");
        let nonexistent_link = test_bin_dir().join("different-tool");

        // Need current_dir for relative path resolution
        runtime.expect_current_dir().returning(|| Ok(test_home()));

        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| true);

        // Read meta.json: has one rule for bin/tool (different from requested)
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

        // Request unlink for different-tool which doesn't exist in rules
        let result = unlink(
            runtime,
            "owner/repo",
            Some(nonexistent_link),
            false,
            Config::for_test(root),
        );
        assert!(result.is_err());
    }
}
