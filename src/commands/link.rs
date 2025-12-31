use anyhow::Result;
use std::path::PathBuf;

use crate::application::LinkAction;
use crate::domain::service::PackageRepository;
use crate::runtime::Runtime;

use super::config::Config;
use super::link_spec::LinkSpec;

/// Link a package's current version to a destination directory
#[tracing::instrument(skip(runtime, config))]
pub fn link<R: Runtime>(runtime: R, repo_str: &str, dest: PathBuf, config: Config) -> Result<()> {
    let spec = repo_str.parse::<LinkSpec>()?;

    let pkg_repo = PackageRepository::new(&runtime, config.install_root.clone());
    let action = LinkAction::new(&runtime, config.install_root);

    // Load package context - this handles version normalization
    let mut ctx =
        pkg_repo.load_context(&spec.repo.owner, &spec.repo.repo, spec.version.as_deref())?;

    // Check if specified version exists
    if ctx.version_specified
        && !pkg_repo.is_version_installed(&ctx.owner, &ctx.repo, ctx.version().as_str())
    {
        anyhow::bail!(
            "Version {} is not installed for {}. Install it first with: ghri install {}@{}",
            ctx.version(),
            ctx.display_name,
            ctx.display_name,
            spec.version.as_ref().unwrap()
        );
    }

    // Delegate to LinkAction for the actual work
    let result = action.create_package_link(&mut ctx, dest, spec.path)?;

    // Display result
    println!(
        "Linked {} {} -> {:?}",
        ctx.display_name,
        ctx.version(),
        result.dest
    );

    Ok(())
}

// Note: Most tests for link functionality have been moved to application/link.rs
// The command layer tests focus on argument parsing and user interface, not implementation details.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::Meta;
    use crate::runtime::MockRuntime;
    use crate::test_utils::{configure_mock_runtime_basics, test_bin_dir, test_root};
    use mockall::predicate::*;

    fn configure_runtime_basics(runtime: &mut MockRuntime) {
        configure_mock_runtime_basics(runtime);
    }

    #[test]
    fn test_link_version_not_installed() {
        // Test error when specified version is not installed
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = test_root();
        let meta_path = root.join("owner").join("repo").join("meta.json");
        let v2_dir = root.join("owner").join("repo").join("v2");
        let dest = test_bin_dir().join("tool");

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
            .with(eq(v2_dir))
            .returning(|_| false);

        let result = link(runtime, "owner/repo@v2", dest, Config::for_test(root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_link_no_current_version() {
        // Test error when no current version is set and no version specified
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        let root = test_root();
        let meta_path = root.join("owner").join("repo").join("meta.json");
        let current_link = root.join("owner").join("repo").join("current");
        let dest = test_bin_dir().join("tool");

        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "".into(),
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into())
            });

        let result = link(runtime, "owner/repo", dest, Config::for_test(root));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No current version")
        );
    }
}
