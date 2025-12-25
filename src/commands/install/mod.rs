use anyhow::Result;
use log::warn;
use std::sync::{Arc, Mutex};

use crate::application::{InstallOperations, InstallUseCase};
use crate::cleanup::CleanupContext;
use crate::provider::Release;
use crate::runtime::Runtime;

use super::config::{Config, ConfigOverrides, InstallOptions};
use super::prune::prune_package_dir;
use super::services::RegistryServices;

mod download;
mod external_links;
mod installer;
mod repo_spec;

pub use download::{DefaultReleaseInstaller, ReleaseInstaller};
pub use repo_spec::RepoSpec;

#[cfg(test)]
#[allow(unused_imports)]
pub use download::MockReleaseInstaller;

use download::get_download_plan;
use external_links::update_external_links;

#[tracing::instrument(skip(runtime, overrides, options))]
pub async fn install<R: Runtime + 'static>(
    runtime: R,
    repo_str: &str,
    overrides: ConfigOverrides,
    options: InstallOptions,
) -> Result<()> {
    // Wrap runtime in Arc first for shared ownership
    let runtime = Arc::new(runtime);

    // Load configuration
    let config = Config::load(runtime.as_ref(), overrides)?;

    // Build services from config
    let services = RegistryServices::from_config(&config)?;

    // Create use case (borrows from Arc)
    let use_case = InstallUseCase::new(
        runtime.as_ref(),
        &services.registry,
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
        &use_case,
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
#[tracing::instrument(skip(config, runtime, use_case, release_installer, options))]
pub async fn run_install<R: Runtime + 'static>(
    config: &Config,
    runtime: Arc<R>,
    use_case: &dyn InstallOperations,
    release_installer: &dyn ReleaseInstaller,
    repo_str: &str,
    options: InstallOptions,
) -> Result<()> {
    let spec = repo_str.parse::<RepoSpec>()?;
    let repo = &spec.repo;

    println!("   resolving {}", repo);

    // Get or fetch metadata
    let source = use_case.resolve_source_for_new()?;
    let (mut meta, is_new) = use_case.get_or_fetch_meta(repo, source.as_ref()).await?;

    // Get effective filters
    let app_options = crate::application::InstallOptions {
        filters: options.filters.clone(),
        pre: options.pre,
        yes: options.yes,
        original_args: options.original_args.clone(),
    };
    let effective_filters = use_case.effective_filters(&app_options, &meta);

    // Resolve version
    let meta_release = use_case.resolve_version(&meta, spec.version.clone(), options.pre)?;
    let release: Release = meta_release.into();

    // Check if already installed
    if use_case.is_installed(repo, &release.tag) {
        println!("   {} {} is already installed", repo, release.tag);
        return Ok(());
    }

    let target_dir = use_case.version_dir(repo, &release.tag);
    let meta_path = use_case.meta_path(repo);

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
    use_case.update_current_link(repo, &release.tag)?;

    // Update external links
    if let Some(package_dir) = target_dir.parent()
        && let Err(e) = update_external_links(runtime.as_ref(), package_dir, &target_dir, &meta)
    {
        warn!("Failed to update external links: {}. Continuing.", e);
    }

    // Save metadata
    meta.current_version = release.tag.clone();
    meta.filters = effective_filters;
    if let Err(e) = use_case.save_meta(repo, &meta) {
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
    use crate::package::{Meta, MetaRelease};
    use crate::provider::{MockProvider, RepoId};
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
            releases: vec![MetaRelease {
                version: "v1.0.0".into(),
                published_at: Some("2024-01-01T00:00:00Z".into()),
                is_prerelease: false,
                assets: vec![],
                tarball_url: "https://example.com/tarball".into(),
                title: None,
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

        let mut use_case = MockInstallOperations::new();
        let mut release_installer = MockReleaseInstaller::new();

        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let meta = test_meta();
        let target_dir = PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // Setup use_case expectations
        use_case
            .expect_resolve_source_for_new()
            .returning(move || Ok(Arc::new(MockProvider::new())));

        let meta_clone = meta.clone();
        use_case.expect_get_or_fetch_meta().returning(move |_, _| {
            let m = meta_clone.clone();
            Box::pin(async move { Ok((m, true)) })
        });

        use_case.expect_effective_filters().returning(|_, _| vec![]);

        let release = meta.releases[0].clone();
        use_case
            .expect_resolve_version()
            .returning(move |_, _, _| Ok(release.clone()));

        use_case
            .expect_is_installed()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(|_, _| false);

        let target_dir_clone = target_dir.clone();
        use_case
            .expect_version_dir()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(move |_, _| target_dir_clone.clone());

        let meta_path_clone = meta_path.clone();
        use_case
            .expect_meta_path()
            .with(eq(repo.clone()))
            .returning(move |_| meta_path_clone.clone());

        // Mock the actual installation
        release_installer
            .expect_install()
            .returning(|_, _, _, _, _| Ok(()));

        // Mock update_current_link
        use_case
            .expect_update_current_link()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(|_, _| Ok(()));

        // Mock save_meta
        use_case.expect_save_meta().returning(|_, _| Ok(()));

        // Execute
        let config = test_config();
        let options = default_install_options();
        let result = run_install(
            &config,
            Arc::new(runtime),
            &use_case,
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
        let mut use_case = MockInstallOperations::new();
        let release_installer = MockReleaseInstaller::new(); // No install() call expected

        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let meta = test_meta();

        // Setup use_case expectations
        use_case
            .expect_resolve_source_for_new()
            .returning(|| Ok(Arc::new(MockProvider::new())));

        let meta_clone = meta.clone();
        use_case.expect_get_or_fetch_meta().returning(move |_, _| {
            let m = meta_clone.clone();
            Box::pin(async move { Ok((m, false)) })
        });

        use_case.expect_effective_filters().returning(|_, _| vec![]);

        let release = meta.releases[0].clone();
        use_case
            .expect_resolve_version()
            .returning(move |_, _, _| Ok(release.clone()));

        // Already installed!
        use_case
            .expect_is_installed()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(|_, _| true);

        // Execute
        let config = test_config();
        let options = default_install_options();
        let result = run_install(
            &config,
            Arc::new(runtime),
            &use_case,
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

        let mut use_case = MockInstallOperations::new();
        let release_installer = MockReleaseInstaller::new(); // No install() call expected

        let meta = test_meta();
        let target_dir = PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // Setup use_case expectations
        use_case
            .expect_resolve_source_for_new()
            .returning(|| Ok(Arc::new(MockProvider::new())));

        let meta_clone = meta.clone();
        use_case.expect_get_or_fetch_meta().returning(move |_, _| {
            let m = meta_clone.clone();
            Box::pin(async move { Ok((m, true)) })
        });

        use_case.expect_effective_filters().returning(|_, _| vec![]);

        let release = meta.releases[0].clone();
        use_case
            .expect_resolve_version()
            .returning(move |_, _, _| Ok(release.clone()));

        use_case.expect_is_installed().returning(|_, _| false);

        let target_dir_clone = target_dir.clone();
        use_case
            .expect_version_dir()
            .returning(move |_, _| target_dir_clone.clone());

        let meta_path_clone = meta_path.clone();
        use_case
            .expect_meta_path()
            .returning(move |_| meta_path_clone.clone());

        // Execute with yes=false to trigger confirmation
        let config = test_config();
        let mut options = default_install_options();
        options.yes = false;

        let result = run_install(
            &config,
            Arc::new(runtime),
            &use_case,
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
        let use_case = MockInstallOperations::new(); // No calls expected
        let release_installer = MockReleaseInstaller::new(); // No calls expected

        let config = test_config();
        let options = default_install_options();
        let result = run_install(
            &config,
            Arc::new(runtime),
            &use_case,
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
        let mut use_case = MockInstallOperations::new();
        let mut release_installer = MockReleaseInstaller::new();

        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let meta = test_meta();
        let target_dir = PathBuf::from("/home/user/.ghri/owner/repo/v1.0.0");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // Setup use_case expectations
        use_case
            .expect_resolve_source_for_new()
            .returning(|| Ok(Arc::new(MockProvider::new())));

        let meta_clone = meta.clone();
        use_case.expect_get_or_fetch_meta().returning(move |_, _| {
            let m = meta_clone.clone();
            Box::pin(async move { Ok((m, true)) })
        });

        use_case.expect_effective_filters().returning(|_, _| vec![]);

        let release = meta.releases[0].clone();
        use_case
            .expect_resolve_version()
            .returning(move |_, _, _| Ok(release.clone()));

        use_case
            .expect_is_installed()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(|_, _| false);

        let target_dir_clone = target_dir.clone();
        use_case
            .expect_version_dir()
            .with(eq(repo.clone()), eq("v1.0.0"))
            .returning(move |_, _| target_dir_clone.clone());

        let meta_path_clone = meta_path.clone();
        use_case
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
            &use_case,
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
        let mut use_case = MockInstallOperations::new();
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
                MetaRelease {
                    version: "v2.0.0".into(),
                    published_at: Some("2024-02-01T00:00:00Z".into()),
                    is_prerelease: false,
                    assets: vec![],
                    tarball_url: "https://example.com/tarball/v2".into(),
                    title: None,
                },
                MetaRelease {
                    version: "v1.0.0".into(),
                    published_at: Some("2024-01-01T00:00:00Z".into()),
                    is_prerelease: false,
                    assets: vec![],
                    tarball_url: "https://example.com/tarball/v1".into(),
                    title: None,
                },
            ],
            ..Default::default()
        };

        let target_dir = PathBuf::from("/home/user/.ghri/owner/repo/v2.0.0");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // Setup use_case expectations
        use_case
            .expect_resolve_source_for_new()
            .returning(|| Ok(Arc::new(MockProvider::new())));

        let meta_clone = meta.clone();
        use_case.expect_get_or_fetch_meta().returning(move |_, _| {
            let m = meta_clone.clone();
            Box::pin(async move { Ok((m, true)) })
        });

        use_case.expect_effective_filters().returning(|_, _| vec![]);

        // resolve_version should be called with the specified version
        let release = meta.releases[0].clone();
        use_case
            .expect_resolve_version()
            .withf(|_, version, _| version == &Some("v2.0.0".to_string()))
            .returning(move |_, _, _| Ok(release.clone()));

        use_case
            .expect_is_installed()
            .with(eq(repo.clone()), eq("v2.0.0"))
            .returning(|_, _| false);

        let target_dir_clone = target_dir.clone();
        use_case
            .expect_version_dir()
            .with(eq(repo.clone()), eq("v2.0.0"))
            .returning(move |_, _| target_dir_clone.clone());

        let meta_path_clone = meta_path.clone();
        use_case
            .expect_meta_path()
            .with(eq(repo.clone()))
            .returning(move |_| meta_path_clone.clone());

        release_installer
            .expect_install()
            .returning(|_, _, _, _, _| Ok(()));

        use_case
            .expect_update_current_link()
            .with(eq(repo.clone()), eq("v2.0.0"))
            .returning(|_, _| Ok(()));

        use_case.expect_save_meta().returning(|_, _| Ok(()));

        // Execute with version spec
        let config = test_config();
        let options = default_install_options();
        let result = run_install(
            &config,
            Arc::new(runtime),
            &use_case,
            &release_installer,
            "owner/repo@v2.0.0", // Specific version
            options,
        )
        .await;

        assert!(result.is_ok());
    }
}
