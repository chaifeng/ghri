use anyhow::Result;
use log::warn;
use std::sync::{Arc, Mutex};

use crate::application::{InstallAction, InstallOperations};
use crate::cleanup::CleanupContext;
use crate::provider::PackageSpec;
use crate::runtime::Runtime;

use super::config::{Config, InstallOptions};
use super::prune::prune_package_dir;
use super::services::Services;

mod installer;

pub use crate::domain::service::install_manager::{
    DefaultReleaseInstaller, ReleaseInstaller, get_download_plan,
};

#[cfg(test)]
#[allow(unused_imports)]
pub use crate::domain::service::install_manager::MockReleaseInstaller;

#[tracing::instrument(skip(runtime, config, options))]
pub async fn install<R: Runtime + 'static>(
    runtime: R,
    repo_str: &str,
    config: Config,
    options: InstallOptions,
) -> Result<()> {
    // Wrap runtime in Arc first for shared ownership
    let runtime = Arc::new(runtime);

    // Build services from config
    let services = Services::from_config(&config)?;

    // Create action (borrows from Arc)
    let action = InstallAction::new(
        runtime.as_ref(),
        &services.provider_factory,
        config.install_root.clone(),
    );

    // Create release installer
    let release_installer = DefaultReleaseInstaller::new(
        Arc::clone(&runtime),
        Arc::new(services.downloader),
        Arc::new(services.extractor),
        Arc::new(Mutex::new(CleanupContext::new())),
    );

    // Run installation - clone Arc for ownership transfer
    run_install(
        &config,
        Arc::clone(&runtime),
        &action,
        &release_installer,
        repo_str,
        options,
    )
    .await
}

/// Core installation logic - separated for testability
///
/// This function takes all dependencies as parameters, enabling:
/// - Unit tests with mock InstallOperations and ReleaseInstaller
/// - Integration tests with real implementations
#[tracing::instrument(skip(config, runtime, action, release_installer, options))]
pub async fn run_install<R: Runtime + 'static>(
    config: &Config,
    runtime: Arc<R>,
    action: &dyn InstallOperations,
    release_installer: &dyn ReleaseInstaller,
    repo_str: &str,
    options: InstallOptions,
) -> Result<()> {
    let spec = repo_str.parse::<PackageSpec>()?;
    let repo = &spec.repo;

    println!("   resolving {}", repo);

    // Get or fetch metadata
    let source = action.resolve_source_for_new()?;
    let (mut meta, is_new) = action.get_or_fetch_meta(repo, source.as_ref()).await?;

    // Get effective filters
    let effective_filters = action.effective_filters(&options, &meta);

    // Resolve version
    let release = action.resolve_version(&meta, spec.version.clone(), options.pre)?;

    // Check if already installed
    if action.is_installed(repo, &release.tag) {
        println!("   {} {} is already installed", repo, release.tag);
        return Ok(());
    }

    let target_dir = action.version_dir(repo, &release.tag);
    let meta_path = action.meta_path(repo);

    // Get download plan and show confirmation
    let plan = get_download_plan(&release, &effective_filters)?;

    if !options.yes {
        installer::show_install_plan(
            repo,
            &release,
            &target_dir,
            &meta_path,
            &plan,
            is_new,
            &meta,
        );
        if !runtime.confirm("Proceed with installation?")? {
            println!("Installation cancelled.");
            return Ok(());
        }
    }

    // Perform the actual download and extraction via ReleaseInstaller
    release_installer
        .install(
            repo,
            &release,
            &target_dir,
            &effective_filters,
            &options.original_args,
        )
        .await?;

    // Update 'current' symlink
    action.update_current_link(repo, &release.tag)?;

    // Update external links
    if let Err(e) = action.update_external_links(&meta, &target_dir) {
        warn!("Failed to update external links: {}. Continuing.", e);
    }

    // Save metadata
    meta.current_version = release.tag.clone();
    meta.filters = effective_filters;
    if let Err(e) = action.save_meta(repo, &meta) {
        warn!("Failed to save package metadata: {}. Continuing.", e);
    }

    println!(
        "   installed {} {} {}",
        repo,
        release.tag,
        target_dir.display()
    );

    // Prune old versions if requested
    if options.prune {
        prune_package_dir(
            runtime.as_ref(),
            &config.install_root,
            &repo.owner,
            &repo.repo,
            &repo.to_string(),
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::MockInstallOperations;
    use crate::package::Meta;
    use crate::provider::{MockProvider, Release, RepoId};
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;
    use std::path::PathBuf;

    fn default_install_options() -> InstallOptions {
        InstallOptions {
            filters: vec![],
            pre: false,
            yes: true, // Skip confirmation in tests
            prune: false,
            original_args: vec![],
        }
    }

    fn test_config() -> Config {
        Config {
            install_root: PathBuf::from("/home/user/.ghri"),
            api_url: "https://api.github.com".into(),
            token: None,
        }
    }

    fn test_meta() -> Meta {
        Meta {
            name: "owner/repo".into(),
            api_url: "https://api.github.com".into(),
            current_version: String::new(),
            releases: vec![Release {
                tag: "v1.0.0".into(),
                published_at: Some("2024-01-01T00:00:00Z".into()),
                prerelease: false,
                assets: vec![],
                tarball_url: "https://example.com/tarball".into(),
                name: None,
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_run_install_happy_path() {
        // Test successful installation with mocked traits
        // No need to mock low-level Runtime operations!

        let runtime = MockRuntime::new();
        // Only need to mock confirm() since we use yes: true in options
        // (confirm is skipped when yes=true)

        let mut action = MockInstallOperations::new();
        let mut release_installer = MockReleaseInstaller::new();

        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let meta = test_meta();
        let target_dir = PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // Setup action expectations
        action
            .expect_resolve_source_for_new()
            .returning(move || Ok(Arc::new(MockProvider::new())));

        let meta_clone = meta.clone();
        action.expect_get_or_fetch_meta().returning(move |_, _| {
            let m = meta_clone.clone();
            Box::pin(async move { Ok((m, true)) })
        });

        action.expect_effective_filters().returning(|_, _| vec![]);

        let release = meta.releases[0].clone();
        action
            .expect_resolve_version()
            .returning(move |_, _, _| Ok(release.clone()));

        action
            .expect_is_installed()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(|_, _| false);

        let target_dir_clone = target_dir.clone();
        action
            .expect_version_dir()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(move |_, _| target_dir_clone.clone());

        let meta_path_clone = meta_path.clone();
        action
            .expect_meta_path()
            .with(eq(repo.clone()))
            .returning(move |_| meta_path_clone.clone());

        // Mock the actual installation
        release_installer
            .expect_install()
            .returning(|_, _, _, _, _| Ok(()));

        // Mock update_current_link
        action
            .expect_update_current_link()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(|_, _| Ok(()));

        // Mock update_external_links
        action
            .expect_update_external_links()
            .returning(|_, _| Ok(()));

        // Mock save_meta
        action.expect_save_meta().returning(|_, _| Ok(()));

        // Execute
        let config = test_config();
        let options = default_install_options();
        let result = run_install(
            &config,
            Arc::new(runtime),
            &action,
            &release_installer,
            "owner/repo",
            options,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_install_already_installed() {
        // Test that installation is skipped when version is already installed

        let runtime = MockRuntime::new();
        let mut action = MockInstallOperations::new();
        let release_installer = MockReleaseInstaller::new(); // No install() call expected

        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let meta = test_meta();

        // Setup action expectations
        action
            .expect_resolve_source_for_new()
            .returning(|| Ok(Arc::new(MockProvider::new())));

        let meta_clone = meta.clone();
        action.expect_get_or_fetch_meta().returning(move |_, _| {
            let m = meta_clone.clone();
            Box::pin(async move { Ok((m, false)) })
        });

        action.expect_effective_filters().returning(|_, _| vec![]);

        let release = meta.releases[0].clone();
        action
            .expect_resolve_version()
            .returning(move |_, _, _| Ok(release.clone()));

        // Already installed!
        action
            .expect_is_installed()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(|_, _| true);

        // Execute
        let config = test_config();
        let options = default_install_options();
        let result = run_install(
            &config,
            Arc::new(runtime),
            &action,
            &release_installer,
            "owner/repo",
            options,
        )
        .await;

        assert!(result.is_ok());
        // Note: release_installer.install() should NOT be called
    }

    #[tokio::test]
    async fn test_run_install_user_cancels() {
        // Test that installation is cancelled when user declines confirmation

        let mut runtime = MockRuntime::new();
        runtime.expect_confirm().returning(|_| Ok(false)); // User says no

        let mut action = MockInstallOperations::new();
        let release_installer = MockReleaseInstaller::new(); // No install() call expected

        let meta = test_meta();
        let target_dir = PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // Setup action expectations
        action
            .expect_resolve_source_for_new()
            .returning(|| Ok(Arc::new(MockProvider::new())));

        let meta_clone = meta.clone();
        action.expect_get_or_fetch_meta().returning(move |_, _| {
            let m = meta_clone.clone();
            Box::pin(async move { Ok((m, true)) })
        });

        action.expect_effective_filters().returning(|_, _| vec![]);

        let release = meta.releases[0].clone();
        action
            .expect_resolve_version()
            .returning(move |_, _, _| Ok(release.clone()));

        action.expect_is_installed().returning(|_, _| false);

        let target_dir_clone = target_dir.clone();
        action
            .expect_version_dir()
            .returning(move |_, _| target_dir_clone.clone());

        let meta_path_clone = meta_path.clone();
        action
            .expect_meta_path()
            .returning(move |_| meta_path_clone.clone());

        // Execute with yes=false to trigger confirmation
        let config = test_config();
        let mut options = default_install_options();
        options.yes = false;

        let result = run_install(
            &config,
            Arc::new(runtime),
            &action,
            &release_installer,
            "owner/repo",
            options,
        )
        .await;

        assert!(result.is_ok()); // Cancellation is not an error
    }

    #[tokio::test]
    async fn test_run_install_invalid_repo_spec() {
        // Test that invalid repo spec returns error early
        let runtime = MockRuntime::new();
        let action = MockInstallOperations::new(); // No calls expected
        let release_installer = MockReleaseInstaller::new(); // No calls expected

        let config = test_config();
        let options = default_install_options();
        let result = run_install(
            &config,
            Arc::new(runtime),
            &action,
            &release_installer,
            "invalid-repo-string-without-slash",
            options,
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid repository")
        );
    }

    #[tokio::test]
    async fn test_run_install_download_fails() {
        // Test error handling when release_installer fails
        let runtime = MockRuntime::new();
        let mut action = MockInstallOperations::new();
        let mut release_installer = MockReleaseInstaller::new();

        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let meta = test_meta();
        let target_dir = PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // Setup action expectations
        action
            .expect_resolve_source_for_new()
            .returning(|| Ok(Arc::new(MockProvider::new())));

        let meta_clone = meta.clone();
        action.expect_get_or_fetch_meta().returning(move |_, _| {
            let m = meta_clone.clone();
            Box::pin(async move { Ok((m, true)) })
        });

        action.expect_effective_filters().returning(|_, _| vec![]);

        let release = meta.releases[0].clone();
        action
            .expect_resolve_version()
            .returning(move |_, _, _| Ok(release.clone()));

        action
            .expect_is_installed()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(|_, _| false);

        let target_dir_clone = target_dir.clone();
        action
            .expect_version_dir()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(move |_, _| target_dir_clone.clone());

        let meta_path_clone = meta_path.clone();
        action
            .expect_meta_path()
            .with(eq(repo.clone()))
            .returning(move |_| meta_path_clone.clone());

        // Release installer fails
        release_installer
            .expect_install()
            .returning(|_, _, _, _, _| Err(anyhow::anyhow!("Download failed: network error")));

        // Execute
        let config = test_config();
        let options = default_install_options();
        let result = run_install(
            &config,
            Arc::new(runtime),
            &action,
            &release_installer,
            "owner/repo",
            options,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("network error"));
    }

    #[tokio::test]
    async fn test_run_install_with_specific_version() {
        // Test installing a specific version (e.g., owner/repo@v2.0.0)
        let runtime = MockRuntime::new();
        let mut action = MockInstallOperations::new();
        let mut release_installer = MockReleaseInstaller::new();

        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };

        // Meta with multiple releases
        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://api.github.com".into(),
            current_version: String::new(),
            releases: vec![
                Release {
                    tag: "v2.0.0".into(),
                    published_at: Some("2024-02-01T00:00:00Z".into()),
                    prerelease: false,
                    assets: vec![],
                    tarball_url: "https://example.com/tarball/v2".into(),
                    name: None,
                },
                Release {
                    tag: "v1.0.0".into(),
                    published_at: Some("2024-01-01T00:00:00Z".into()),
                    prerelease: false,
                    assets: vec![],
                    tarball_url: "https://example.com/tarball/v1".into(),
                    name: None,
                },
            ],
            ..Default::default()
        };

        let target_dir = PathBuf::from("/home/user/.ghri/owner/repo/v2.0.0");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // Setup action expectations
        action
            .expect_resolve_source_for_new()
            .returning(|| Ok(Arc::new(MockProvider::new())));

        let meta_clone = meta.clone();
        action.expect_get_or_fetch_meta().returning(move |_, _| {
            let m = meta_clone.clone();
            Box::pin(async move { Ok((m, true)) })
        });

        action.expect_effective_filters().returning(|_, _| vec![]);

        // resolve_version should be called with the specified version
        let release = meta.releases[0].clone();
        action
            .expect_resolve_version()
            .withf(|_, version, _| version == &Some("v2.0.0".to_string()))
            .returning(move |_, _, _| Ok(release.clone()));

        action
            .expect_is_installed()
            .with(eq(repo.clone()), eq("v2.0.0"))
            .returning(|_, _| false);

        let target_dir_clone = target_dir.clone();
        action
            .expect_version_dir()
            .with(eq(repo.clone()), eq("v2.0.0"))
            .returning(move |_, _| target_dir_clone.clone());

        let meta_path_clone = meta_path.clone();
        action
            .expect_meta_path()
            .with(eq(repo.clone()))
            .returning(move |_| meta_path_clone.clone());

        release_installer
            .expect_install()
            .returning(|_, _, _, _, _| Ok(()));

        action
            .expect_update_current_link()
            .with(eq(repo.clone()), eq("v2.0.0"))
            .returning(|_, _| Ok(()));

        action
            .expect_update_external_links()
            .returning(|_, _| Ok(()));

        action.expect_save_meta().returning(|_, _| Ok(()));

        // Execute with version spec
        let config = test_config();
        let options = default_install_options();
        let result = run_install(
            &config,
            Arc::new(runtime),
            &action,
            &release_installer,
            "owner/repo@v2.0.0", // Specific version
            options,
        )
        .await;

        assert!(result.is_ok());
    }
}
