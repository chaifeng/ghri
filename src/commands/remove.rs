use anyhow::Result;
use log::debug;

use crate::application::RemoveAction;
use crate::domain::model::PackageContext;
use crate::provider::PackageSpec;
use crate::runtime::Runtime;

use super::config::Config;

/// Remove a package or specific version
#[tracing::instrument(skip(runtime, config))]
pub fn remove<R: Runtime>(
    runtime: R,
    repo_str: &str,
    force: bool,
    yes: bool,
    config: Config,
) -> Result<()> {
    debug!("Removing {} force={}", repo_str, force);
    let spec = repo_str.parse::<PackageSpec>()?;
    debug!("Using install root: {:?}", config.install_root);

    let action = RemoveAction::new(&runtime, &config.install_root);

    // Load package context - version may be None if not specified and no current
    let ctx = action.package_repo().load_context_any(
        &spec.repo.owner,
        &spec.repo.repo,
        spec.version.as_deref(),
    )?;

    if ctx.version_specified {
        // Remove specific version
        let version = ctx.version();
        debug!("Removing specific version: {}", version);

        // Show removal plan and confirm
        if !yes {
            show_version_removal_plan(&ctx, &action);
            if !runtime.confirm("Proceed with removal?")? {
                println!("Removal cancelled.");
                return Ok(());
            }
        }

        // Check if this is current before removal (for warning message)
        let was_current =
            action
                .package_repo()
                .is_current_version(&ctx.owner, &ctx.repo, version.as_str());

        action.remove_version(&ctx, force)?;
        println!(
            "Removed version {} from {}",
            version,
            ctx.package_dir.display()
        );

        if was_current {
            println!("Warning: Removed current version symlink. No version is now active.");
        }
    } else {
        // Remove entire package
        debug!("Removing entire package");

        // Show removal plan and confirm
        if !yes {
            show_package_removal_plan(&ctx);
            if !runtime.confirm("Proceed with removal?")? {
                println!("Removal cancelled.");
                return Ok(());
            }
        }

        action.remove_package(&ctx)?;
        println!("Removed package {}", ctx.display_name);
    }

    Ok(())
}

fn show_package_removal_plan(ctx: &PackageContext) {
    println!();
    println!("=== Removal Plan ===");
    println!();
    println!("Package: {}", ctx.display_name);
    println!();

    println!("Directories to remove:");
    println!("  [DEL] {}", ctx.package_dir.display());

    if !ctx.meta.links.is_empty() || !ctx.meta.versioned_links.is_empty() {
        println!();
        println!("Link rules that will be removed:");
        for link in &ctx.meta.links {
            println!("  {:?}", link.dest);
        }
        for link in &ctx.meta.versioned_links {
            println!("  {:?} (version {})", link.dest, link.version);
        }
    }
    println!();
}

fn show_version_removal_plan<R: Runtime>(ctx: &PackageContext, action: &RemoveAction<'_, R>) {
    let version = ctx.version();
    let version_dir = ctx.version_dir();

    // Check if this is the current version
    let is_current =
        action
            .package_repo()
            .is_current_version(&ctx.owner, &ctx.repo, version.as_str());

    println!();
    println!("=== Removal Plan ===");
    println!();
    println!("Package: {}", ctx.display_name);
    println!("Version: {}", version);
    if is_current {
        println!("  (This is the current version!)");
    }
    println!();

    println!("Directories to remove:");
    println!("  [DEL] {}", version_dir.display());

    if is_current {
        println!();
        println!("Symlinks to remove:");
        println!("  [DEL] {}/current", ctx.package_dir.display());
    }

    // Show versioned links for this version
    let versioned_for_this: Vec<_> = ctx
        .meta
        .versioned_links
        .iter()
        .filter(|l| l.version == version.as_str())
        .collect();

    if !versioned_for_this.is_empty() {
        println!();
        println!("Versioned links to remove:");
        for link in versioned_for_this {
            println!("  {:?}", link.dest);
        }
    }

    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{LinkRule, Meta};
    use crate::runtime::MockRuntime;
    use crate::test_utils::{configure_mock_runtime_basics, test_bin_dir, test_root};
    use mockall::predicate::*;
    use std::path::PathBuf;

    fn configure_runtime_basics(runtime: &mut MockRuntime) {
        configure_mock_runtime_basics(runtime);
    }

    #[test]
    fn test_remove_package() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = test_root();
        let owner_dir = root.join("owner");
        let package_dir = owner_dir.join("repo");
        let meta_path = package_dir.join("meta.json");
        let link_dest = test_bin_dir().join("tool");

        // Load Metadata
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

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

        // Remove link (via LinkManager)
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        let resolved_target = package_dir.join("v1").join("tool");
        runtime
            .expect_resolve_link()
            .with(eq(link_dest.clone()))
            .returning(move |_| Ok(resolved_target.clone()));

        runtime
            .expect_remove_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| Ok(()));

        // Remove package directory
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_remove_dir_all()
            .with(eq(package_dir.clone()))
            .returning(|_| Ok(()));

        // Cleanup empty owner directory
        runtime
            .expect_exists()
            .with(eq(owner_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(vec![]));

        runtime
            .expect_remove_dir_all()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(()));

        let result = remove(runtime, "owner/repo", false, true, Config::for_test(root));
        assert!(result.is_ok());
    }

    #[test]
    fn test_remove_specific_version() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = test_root();
        let owner_dir = root.join("owner");
        let package_dir = owner_dir.join("repo");
        let version_dir = package_dir.join("v1");
        let meta_path = package_dir.join("meta.json");
        let current_link = package_dir.join("current");
        let link_dest = test_bin_dir().join("tool");

        // Load Metadata
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v2".into(), // Different from version being removed
            links: vec![LinkRule {
                dest: link_dest.clone(),
                path: None,
            }],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Check version installed
        runtime
            .expect_is_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // Check if v1 is current version
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v2")));

        // Check link target (points to v2, not v1)
        let v2_target = root.join("owner").join("repo").join("v2").join("tool");
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(link_dest.clone()))
            .returning(move |_| Ok(v2_target.clone()));

        // Remove version directory
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        runtime
            .expect_remove_dir_all()
            .with(eq(version_dir.clone()))
            .returning(|_| Ok(()));

        // Cleanup check (not empty)
        runtime
            .expect_exists()
            .with(eq(owner_dir.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(owner_dir.clone()))
            .returning(|_| Ok(vec![PathBuf::from("repo")]));

        let result = remove(
            runtime,
            "owner/repo@v1",
            false,
            true,
            Config::for_test(root),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_remove_current_version_requires_force() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = test_root();
        let package_dir = root.join("owner").join("repo");
        let version_dir = package_dir.join("v1");
        let meta_path = package_dir.join("meta.json");
        let current_link = package_dir.join("current");

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

        runtime
            .expect_is_dir()
            .with(eq(version_dir.clone()))
            .returning(|_| true);

        // Current symlink points to v1 (same as being removed)
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1")));

        let result = remove(
            runtime,
            "owner/repo@v1",
            false,
            true,
            Config::for_test(root),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--force"));
    }

    #[test]
    fn test_remove_nonexistent_package_fails() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = test_root();
        let package_dir = root.join("owner").join("repo");
        let meta_path = package_dir.join("meta.json");

        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| false);

        let result = remove(runtime, "owner/repo", false, true, Config::for_test(root));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not installed") || err_msg.contains("not found"),
            "Expected 'not installed' or 'not found' in error, got: {}",
            err_msg
        );
    }
}
