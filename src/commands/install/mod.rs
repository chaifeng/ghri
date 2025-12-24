use anyhow::Result;
use log::warn;
use std::sync::{Arc, Mutex};

use crate::application::InstallUseCase;
use crate::cleanup::CleanupContext;
use crate::runtime::Runtime;
use crate::source::SourceRelease;

use super::config::{Config, ConfigOverrides, InstallOptions};
use super::prune::prune_package_dir;
use super::services::RegistryServices;

mod download;
mod external_links;
mod installer;
mod repo_spec;

pub use installer::Installer;
pub use repo_spec::RepoSpec;

use download::{ensure_installed, get_download_plan};
use external_links::update_external_links;

#[tracing::instrument(skip(runtime, overrides, options))]
pub async fn install<R: Runtime + 'static>(
    runtime: R,
    repo_str: &str,
    overrides: ConfigOverrides,
    options: InstallOptions,
) -> Result<()> {
    // Load configuration
    let config = Config::load(&runtime, overrides)?;

    // Build services from config
    let services = RegistryServices::from_config(&config)?;

    // Run installation
    run(&config, runtime, services, repo_str, options).await
}

#[tracing::instrument(skip(config, runtime, services, options))]
pub async fn run<R: Runtime + 'static>(
    config: &Config,
    runtime: R,
    services: RegistryServices,
    repo_str: &str,
    options: InstallOptions,
) -> Result<()> {
    let spec = repo_str.parse::<RepoSpec>()?;
    let repo = &spec.repo;

    // Create InstallUseCase for orchestration
    let use_case = InstallUseCase::new(&runtime, &services.registry, config.install_root.clone());

    println!("   resolving {}", repo);

    // Get or fetch metadata
    let source = use_case.resolve_source(None)?;
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
    let meta_release = use_case.resolve_version(&meta, spec.version.as_deref(), options.pre)?;
    let release: SourceRelease = meta_release.clone().into();

    // Check if already installed
    if use_case.is_installed(repo, &release.tag) {
        println!("   {} {} is already installed", repo, release.tag);
        return Ok(());
    }

    let target_dir = use_case.version_dir(repo, &release.tag);
    let meta_path = use_case.package_repo().meta_path(&repo.owner, &repo.repo);

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

    // Set up cleanup context for Ctrl-C handling
    let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
    let cleanup_ctx_clone = Arc::clone(&cleanup_ctx);

    let ctrl_c_handler = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\nInterrupted, cleaning up...");
            cleanup_ctx_clone.lock().unwrap().cleanup();
            std::process::exit(130);
        }
    });

    // Perform the actual download and extraction
    let install_result = ensure_installed(
        &runtime,
        &target_dir,
        repo,
        &release,
        &services.downloader,
        &services.extractor,
        Arc::clone(&cleanup_ctx),
        &effective_filters,
        &options.original_args,
    )
    .await;

    ctrl_c_handler.abort();
    install_result?;

    // Update 'current' symlink
    use_case.update_current_link(repo, &release.tag)?;

    // Update external links
    if let Some(package_dir) = target_dir.parent()
        && let Err(e) = update_external_links(&runtime, package_dir, &target_dir, &meta)
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
            &runtime,
            &config.install_root,
            &repo.owner,
            &repo.repo,
            &repo.to_string(),
        )?;
    }

    Ok(())
}
