use anyhow::Result;
use log::{info, warn};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::{
    application::InstallUseCase,
    archive::ArchiveExtractor,
    cleanup::CleanupContext,
    download::Downloader,
    package::{LinkManager, Meta},
    runtime::Runtime,
    source::{RepoId, Source, SourceRegistry, SourceRelease},
};

use crate::commands::config::{Config, InstallOptions};

use super::download::{DownloadPlan, ensure_installed_impl, get_download_plan};
use super::external_links::update_external_links;

/// Show installation plan to user (standalone function for use from mod.rs)
#[allow(clippy::too_many_arguments)]
pub fn show_install_plan(
    repo: &RepoId,
    release: &SourceRelease,
    target_dir: &Path,
    meta_path: &Path,
    plan: &DownloadPlan,
    needs_save: bool,
    meta: &Meta,
) {
    println!();
    println!("=== Installation Plan ===");
    println!();
    println!("Package:  {}", repo);
    println!("Version:  {}", release.tag);
    println!();

    // Show files to download
    println!("Files to download:");
    match plan {
        DownloadPlan::Tarball { url } => {
            println!("  - {} (source tarball)", url);
        }
        DownloadPlan::Assets { assets } => {
            for asset in assets {
                println!("  - {} ({} bytes)", asset.name, asset.size);
            }
        }
    }
    println!();

    // Show files/directories to create
    println!("Files/directories to create:");
    println!("  [DIR]  {}", target_dir.display());
    if needs_save {
        println!("  [FILE] {}", meta_path.display());
    } else {
        println!("  [MOD]  {} (update)", meta_path.display());
    }
    if let Some(parent) = target_dir.parent() {
        println!("  [LINK] {}/current -> {}", parent.display(), release.tag);
    }

    // Note: External link validation requires runtime, handled separately if needed
    if !meta.links.is_empty() {
        println!();
        println!("External links configured: {} link(s)", meta.links.len());
        for link in &meta.links {
            let source = link
                .path
                .as_ref()
                .map(|p| format!(":{}", p))
                .unwrap_or_default();
            println!(
                "  [LINK] {} -> {}{}/{}",
                link.dest.display(),
                repo,
                source,
                release.tag
            );
        }
    }

    // Show versioned links (these won't be updated)
    if !meta.versioned_links.is_empty() {
        println!();
        println!("Versioned links (unchanged):");
        for link in &meta.versioned_links {
            println!(
                "  [LINK] {} -> {}@{}",
                link.dest.display(),
                repo,
                link.version
            );
        }
    }

    println!();
}

pub struct Installer<R: Runtime, S: Source, E: ArchiveExtractor, D: Downloader> {
    pub runtime: R,
    pub source: S,
    pub downloader: D,
    pub extractor: E,
}

impl<R: Runtime + 'static, S: Source, E: ArchiveExtractor, D: Downloader> Installer<R, S, E, D> {
    #[tracing::instrument(skip(runtime, source, downloader, extractor))]
    pub fn new(runtime: R, source: S, downloader: D, extractor: E) -> Self {
        Self {
            runtime,
            source,
            downloader,
            extractor,
        }
    }

    /// Install a package using InstallUseCase for orchestration
    #[tracing::instrument(skip(self, config, registry, repo, version, options))]
    pub async fn install(
        &self,
        config: &Config,
        registry: &SourceRegistry,
        repo: &RepoId,
        version: Option<&str>,
        options: &InstallOptions,
    ) -> Result<()> {
        // Create InstallUseCase for orchestration
        let use_case = InstallUseCase::new(&self.runtime, registry, config.install_root.clone());

        println!("   resolving {}", repo);

        // Get or fetch metadata using UseCase
        let source = use_case.resolve_source(None)?;
        let (mut meta, is_new) = use_case.get_or_fetch_meta(repo, source.as_ref()).await?;

        // Get effective filters using UseCase
        let app_options = crate::application::InstallOptions {
            filters: options.filters.clone(),
            pre: options.pre,
            yes: options.yes,
            original_args: options.original_args.clone(),
        };
        let effective_filters = use_case.effective_filters(&app_options, &meta);

        // Resolve version using UseCase
        let meta_release = use_case.resolve_version(&meta, version, options.pre)?;
        info!("Found version: {}", meta_release.version);
        let release: SourceRelease = meta_release.clone().into();

        // Check if already installed using UseCase
        if use_case.is_installed(repo, &release.tag) {
            println!("   {} {} is already installed", repo, release.tag);
            return Ok(());
        }

        let target_dir = use_case.version_dir(repo, &release.tag);
        let meta_path = use_case.package_repo().meta_path(&repo.owner, &repo.repo);

        // Get download plan and show confirmation
        let plan = get_download_plan(&release, &effective_filters)?;

        if !options.yes {
            self.show_install_plan(
                repo,
                &release,
                &target_dir,
                &meta_path,
                &plan,
                is_new,
                &meta,
            );
            if !self.runtime.confirm("Proceed with installation?")? {
                println!("Installation cancelled.");
                return Ok(());
            }
        }

        // Perform the actual download and extraction
        self.do_install(
            repo,
            &release,
            &target_dir,
            &effective_filters,
            &options.original_args,
        )
        .await?;

        // Update 'current' symlink using UseCase
        use_case.update_current_link(repo, &release.tag)?;

        // Update external links using the proper function
        if let Some(package_dir) = target_dir.parent()
            && let Err(e) = update_external_links(&self.runtime, package_dir, &target_dir, &meta)
        {
            warn!("Failed to update external links: {}. Continuing.", e);
        }

        // Save metadata using UseCase
        meta.current_version = release.tag.clone();
        meta.filters = effective_filters;
        if let Err(e) = use_case.save_meta(repo, &meta) {
            warn!("Failed to save package metadata: {}. Continuing.", e);
        }

        self.print_install_success(repo, &release.tag, &target_dir);

        Ok(())
    }

    /// Perform the actual download and extraction
    async fn do_install(
        &self,
        repo: &RepoId,
        release: &SourceRelease,
        target_dir: &Path,
        filters: &[String],
        original_args: &[String],
    ) -> Result<()> {
        // Set up cleanup context for Ctrl-C handling
        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let cleanup_ctx_clone = Arc::clone(&cleanup_ctx);

        // Register Ctrl-C handler
        let ctrl_c_handler = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\nInterrupted, cleaning up...");
                cleanup_ctx_clone.lock().unwrap().cleanup();
                std::process::exit(130);
            }
        });

        let result = ensure_installed_impl(
            &self.runtime,
            target_dir,
            repo,
            release,
            &self.downloader,
            &self.extractor,
            Arc::clone(&cleanup_ctx),
            filters,
            original_args,
        )
        .await;

        ctrl_c_handler.abort();
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn show_install_plan(
        &self,
        repo: &RepoId,
        release: &SourceRelease,
        target_dir: &Path,
        meta_path: &Path,
        plan: &DownloadPlan,
        needs_save: bool,
        meta: &Meta,
    ) {
        println!();
        println!("=== Installation Plan ===");
        println!();
        println!("Package:  {}", repo);
        println!("Version:  {}", release.tag);
        println!();

        // Show files to download
        println!("Files to download:");
        match plan {
            DownloadPlan::Tarball { url } => {
                println!("  - {} (source tarball)", url);
            }
            DownloadPlan::Assets { assets } => {
                for asset in assets {
                    println!("  - {} ({} bytes)", asset.name, asset.size);
                }
            }
        }
        println!();

        // Show files/directories to create
        println!("Files/directories to create:");
        println!("  [DIR]  {}", target_dir.display());
        if needs_save {
            println!("  [FILE] {}", meta_path.display());
        } else {
            println!("  [MOD]  {} (update)", meta_path.display());
        }
        if let Some(parent) = target_dir.parent() {
            println!("  [LINK] {}/current -> {}", parent.display(), release.tag);
        }

        // Show external links that will be updated (with validity check)
        if !meta.links.is_empty()
            && let Some(package_dir) = target_dir.parent()
        {
            let link_manager = LinkManager::new(&self.runtime);
            let (valid_links, invalid_links) = link_manager.check_links(&meta.links, package_dir);

            // Show valid links (existing or to be created)
            if !valid_links.is_empty() {
                println!();
                println!("External links to update:");
                for link in &valid_links {
                    let source = link
                        .path
                        .as_ref()
                        .map(|p| format!(":{}", p))
                        .unwrap_or_default();
                    if link.status.is_valid() {
                        println!(
                            "  [LINK] {} -> {}{}/{}",
                            link.dest.display(),
                            repo,
                            source,
                            release.tag
                        );
                    } else if link.status.is_creatable() {
                        println!(
                            "  [NEW]  {} -> {}{}/{}",
                            link.dest.display(),
                            repo,
                            source,
                            release.tag
                        );
                    }
                }
            }

            // Show invalid links
            if !invalid_links.is_empty() {
                println!();
                println!("External links to skip (will not be updated):");
                for link in &invalid_links {
                    println!(
                        "  [SKIP] {} ({})",
                        link.dest.display(),
                        link.status.reason()
                    );
                }
            }
        }

        // Show versioned links (these won't be updated)
        if !meta.versioned_links.is_empty() {
            println!();
            println!("Versioned links (unchanged):");
            for link in &meta.versioned_links {
                println!(
                    "  [LINK] {} -> {}@{}",
                    link.dest.display(),
                    repo,
                    link.version
                );
            }
        }

        println!();
    }

    #[tracing::instrument(skip(self, repo, tag, target_dir))]
    fn print_install_success(&self, repo: &RepoId, tag: &str, target_dir: &Path) {
        println!("   installed {} {} {}", repo, tag, target_dir.display());
    }

    /// Get or fetch meta, returning (meta, meta_path, needs_save)
    /// needs_save is true if meta was newly fetched and needs to be saved after successful install
    #[cfg(test)]
    #[tracing::instrument(skip(self, config, repo))]
    pub(crate) async fn get_or_fetch_meta(
        &self,
        config: &Config,
        repo: &RepoId,
    ) -> Result<(Meta, std::path::PathBuf, bool)> {
        use crate::package::PackageRepository;
        let pkg_repo = PackageRepository::new(&self.runtime, config.install_root.clone());
        let meta_path = pkg_repo.meta_path(&repo.owner, &repo.repo);

        match pkg_repo.load(&repo.owner, &repo.repo) {
            Ok(Some(meta)) => return Ok((meta, meta_path, false)),
            Ok(None) => {} // Not installed, will fetch
            Err(e) => {
                warn!(
                    "Failed to load existing meta.json at {:?}: {}. Re-fetching.",
                    meta_path, e
                );
            }
        }

        let meta = self.fetch_meta(repo, "", Some(&config.api_url)).await?;

        // Don't save meta here - let the caller save it after successful install
        Ok((meta, meta_path, true))
    }

    #[cfg(test)]
    #[tracing::instrument(skip(self, repo, current_version, api_url))]
    async fn fetch_meta(
        &self,
        repo: &RepoId,
        current_version: &str,
        api_url: Option<&str>,
    ) -> Result<Meta> {
        let api_url = api_url.unwrap_or_else(|| self.source.api_url());
        let repo_info = self.source.get_repo_metadata_at(repo, api_url).await?;
        let releases = self.source.get_releases_at(repo, api_url).await?;
        Ok(Meta::from(
            repo.clone(),
            repo_info,
            releases,
            current_version,
            api_url,
        ))
    }

    #[cfg(test)]
    #[tracing::instrument(skip(self, meta_path, meta))]
    pub(crate) fn save_meta(&self, meta_path: &Path, meta: &Meta) -> Result<()> {
        let json = serde_json::to_string_pretty(meta)?;
        let tmp_path = meta_path.with_extension("json.tmp");

        self.runtime.write(&tmp_path, json.as_bytes())?;
        self.runtime.rename(&tmp_path, meta_path)?;
        Ok(())
    }

    /// Legacy install method for tests only
    #[cfg(test)]
    #[tracing::instrument(skip(self, config, repo, version, options))]
    pub async fn install_legacy(
        &self,
        config: &Config,
        repo: &RepoId,
        version: Option<&str>,
        options: &InstallOptions,
    ) -> Result<()> {
        println!("   resolving {}", repo);
        let (mut meta, meta_path, needs_save) = self.get_or_fetch_meta(config, repo).await?;

        // Use saved filters from meta if user didn't provide any
        let effective_filters = if options.filters.is_empty() && !meta.filters.is_empty() {
            info!("Using saved filters from meta: {:?}", meta.filters);
            meta.filters.clone()
        } else {
            options.filters.clone()
        };

        // Resolve version
        let meta_release = if let Some(ver) = version {
            meta.releases
                .iter()
                .find(|r| {
                    r.version == ver
                        || r.version == format!("v{}", ver)
                        || r.version.trim_start_matches('v') == ver.trim_start_matches('v')
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Version '{}' not found for {}. Available versions: {}",
                        ver,
                        repo,
                        meta.releases
                            .iter()
                            .take(5)
                            .map(|r| r.version.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?
        } else if options.pre {
            meta.get_latest_release()
                .ok_or_else(|| anyhow::anyhow!("No release found for {}.", repo))?
        } else {
            meta.get_latest_stable_release()
                .ok_or_else(|| anyhow::anyhow!("No stable release found for {}. If you want to install a pre-release, specify the version with @version or use --pre.", repo))?
        };

        info!("Found version: {}", meta_release.version);
        let release: SourceRelease = meta_release.clone().into();

        let target_dir = config.version_dir(&repo.owner, &repo.repo, &release.tag);

        // Check if already installed
        if self.runtime.exists(&target_dir) {
            println!("   {} {} is already installed", repo, release.tag);
            return Ok(());
        }

        // Get download plan and show confirmation
        let plan = get_download_plan(&release, &effective_filters)?;

        if !options.yes {
            self.show_install_plan(
                repo,
                &release,
                &target_dir,
                &meta_path,
                &plan,
                needs_save,
                &meta,
            );
            if !self.runtime.confirm("Proceed with installation?")? {
                println!("Installation cancelled.");
                return Ok(());
            }
        }

        // Perform the actual download and extraction
        self.do_install(
            repo,
            &release,
            &target_dir,
            &effective_filters,
            &options.original_args,
        )
        .await?;

        // Update 'current' symlink to point to the new version
        let link_manager = LinkManager::new(&self.runtime);
        if let Some(package_dir) = target_dir.parent() {
            link_manager.update_current_link(package_dir, &release.tag)?;
        }

        // Update external links if configured
        if let Some(parent) = target_dir.parent()
            && let Err(e) = update_external_links(&self.runtime, parent, &target_dir, &meta)
        {
            warn!("Failed to update external links: {}. Continuing.", e);
        }

        // Metadata handling - save meta only after successful install
        meta.current_version = release.tag.clone();
        meta.filters = effective_filters;
        if needs_save && let Some(parent) = meta_path.parent() {
            self.runtime.create_dir_all(parent)?;
        }
        if let Err(e) = self.save_meta(&meta_path, &meta) {
            warn!("Failed to save package metadata: {}. Continuing.", e);
        }

        self.print_install_success(repo, &release.tag, &target_dir);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::MockArchiveExtractor;
    use crate::commands::config::Config;
    use crate::download::mock::MockDownloader;
    use crate::runtime::MockRuntime;
    use crate::source::{MockSource, RepoMetadata, SourceKind};
    use mockall::predicate::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn test_config() -> Config {
        #[cfg(not(windows))]
        let install_root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let install_root = PathBuf::from("C:\\Users\\user\\.ghri");

        Config {
            install_root,
            api_url: Config::DEFAULT_API_URL.to_string(),
            token: None,
        }
    }

    fn default_install_options() -> InstallOptions {
        InstallOptions {
            filters: vec![],
            pre: false,
            yes: true,
            prune: false,
            original_args: vec![],
        }
    }

    /// Create a test SourceRegistry with a mock source
    #[allow(dead_code)]
    fn test_registry_with_source(source: MockSource) -> SourceRegistry {
        let mut registry = SourceRegistry::new();
        registry.register(Arc::new(source));
        registry
    }

    /// Create a basic mock source that returns the given API URL
    #[allow(dead_code)]
    fn basic_mock_source(api_url: &str) -> MockSource {
        let api_url = api_url.to_string();
        let mut source = MockSource::new();
        source.expect_kind().return_const(SourceKind::GitHub);
        source.expect_api_url().return_const(api_url);
        source
    }

    /// Create a mock source configured for a successful install test
    fn mock_source_for_install(download_url: String) -> MockSource {
        let mut source = MockSource::new();
        source.expect_kind().return_const(SourceKind::GitHub);
        source
            .expect_api_url()
            .return_const("https://api.github.com".to_string());

        // Return repo metadata
        source.expect_get_repo_metadata_at().returning(|_, _| {
            Ok(RepoMetadata {
                description: None,
                homepage: None,
                license: None,
                updated_at: Some("now".into()),
            })
        });

        // Return one release
        source.expect_get_releases_at().return_once(move |_, _| {
            Ok(vec![SourceRelease {
                tag: "v1".into(),
                tarball_url: download_url,
                ..Default::default()
            }])
        });

        source
    }

    /// Setup runtime mocks for a new package installation
    fn setup_runtime_for_new_install(
        runtime: &mut MockRuntime,
        meta_path: PathBuf,
        package_dir: PathBuf,
        version_dir: PathBuf,
        current_link: PathBuf,
    ) {
        // temp_dir for downloads
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // 1. Check meta exists -> false (new install)
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| false);

        // 2. Check package_dir exists -> false (need to create)
        runtime
            .expect_exists()
            .with(eq(package_dir.clone()))
            .returning(|_| false);

        // 3. Create package directory
        runtime
            .expect_create_dir_all()
            .with(eq(package_dir.clone()))
            .returning(|_| Ok(()));

        // 4. Write meta directly (InstallUseCase.save_meta writes to meta.json directly)
        runtime
            .expect_write()
            .with(eq(meta_path.clone()), always())
            .returning(|_, _| Ok(()));

        // 5. Check version dir exists -> false
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| false);

        // 6. Create version dir
        runtime
            .expect_create_dir_all()
            .with(eq(version_dir))
            .returning(|_| Ok(()));

        // 7. Download file operations
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));
        runtime.expect_remove_file().returning(|_| Ok(()));

        // 8. Check current symlink exists -> false
        runtime
            .expect_exists()
            .with(eq(current_link))
            .returning(|_| false);

        // 9. Create current symlink
        runtime.expect_symlink().returning(|_, _| Ok(()));

        // 10. Check package_dir exists for final save -> true (already created)
        runtime
            .expect_exists()
            .with(eq(package_dir))
            .returning(|_| true);

        // 11. Write meta again (final save after install)
        runtime
            .expect_write()
            .with(eq(meta_path), always())
            .returning(|_, _| Ok(()));
    }

    // Tests for get_target_dir and update_current_symlink are now in paths.rs and symlink.rs

    /// New simplified test using SourceRegistry and high-level mocks
    #[cfg(not(windows))]
    #[tokio::test]
    async fn test_install_with_registry() {
        // This test demonstrates the simplified testing approach:
        // - Mock Source returns predefined metadata and releases
        // - Mock Downloader/Extractor handle the actual download
        // - Runtime mocks are organized in a helper function

        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("o/r/meta.json");
        let package_dir = root.join("o/r");
        let version_dir = root.join("o/r/v1");
        let current_link = root.join("o/r/current");

        // --- Setup Runtime Mock (using helper) ---
        let mut runtime = MockRuntime::new();
        setup_runtime_for_new_install(
            &mut runtime,
            meta_path,
            package_dir,
            version_dir,
            current_link,
        );

        // --- Setup Source Mock ---
        let download_url = format!("{}/tarball", url);
        let source = mock_source_for_install(download_url);

        // --- Setup Registry ---
        let registry = test_registry_with_source(source);

        // --- Setup HTTP Mock ---
        let _m = server
            .mock("GET", "/tarball")
            .with_status(200)
            .with_body("data")
            .create();

        // --- Setup Extractor Mock ---
        let mut extractor = MockArchiveExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- Execute ---
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();

        // Note: We pass a dummy source here since install() uses registry's source
        let dummy_source = MockSource::new();
        let installer = Installer::new(runtime, dummy_source, downloader, extractor);

        installer
            .install(&config, &registry, &repo, None, &default_install_options())
            .await
            .unwrap();
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn test_install_happy_path() {
        // Test successful installation of a new package from scratch
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("o/r/meta.json"); // /home/user/.ghri/o/r/meta.json
        let meta_tmp = root.join("o/r/meta.json.tmp"); // /home/user/.ghri/o/r/meta.json.tmp
        let package_dir = root.join("o/r"); // /home/user/.ghri/o/r
        let version_dir = root.join("o/r/v1"); // /home/user/.ghri/o/r/v1
        let current_link = root.join("o/r/current"); // /home/user/.ghri/o/r/current

        // --- 1. Check for Existing Metadata ---

        // File exists: /home/user/.ghri/o/r/meta.json -> false (new install)
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| false);

        // --- 2. Fetch and Save Metadata ---

        // Create directory: /home/user/.ghri/o/r
        runtime
            .expect_create_dir_all()
            .with(eq(package_dir))
            .returning(|_| Ok(()));

        // Write meta to: /home/user/.ghri/o/r/meta.json.tmp
        runtime
            .expect_write()
            .with(eq(meta_tmp.clone()), always())
            .returning(|_, _| Ok(()));

        // Rename: /home/user/.ghri/o/r/meta.json.tmp -> /home/user/.ghri/o/r/meta.json
        runtime
            .expect_rename()
            .with(eq(meta_tmp.clone()), eq(meta_path.clone()))
            .returning(|_, _| Ok(()));

        // --- 3. Download and Extract Release ---

        // Check version dir exists: /home/user/.ghri/o/r/v1 -> false (needs download)
        runtime
            .expect_exists()
            .with(eq(version_dir.clone()))
            .returning(|_| false);

        // Create version dir: /home/user/.ghri/o/r/v1
        runtime
            .expect_create_dir_all()
            .with(eq(version_dir))
            .returning(|_| Ok(()));

        // Create temp file for download, then remove after extraction
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));
        runtime.expect_remove_file().returning(|_| Ok(()));

        // --- 4. Update Current Symlink ---

        // Check symlink exists: /home/user/.ghri/o/r/current -> false (new)
        runtime
            .expect_exists()
            .with(eq(current_link))
            .returning(|_| false);

        // Create symlink: /home/user/.ghri/o/r/current -> v1
        runtime.expect_symlink().returning(|_, _| Ok(()));

        // --- 5. Save Updated Metadata ---

        // Write final meta: /home/user/.ghri/o/r/meta.json.tmp
        runtime
            .expect_write()
            .with(eq(meta_tmp.clone()), always())
            .returning(|_, _| Ok(()));

        // Rename: /home/user/.ghri/o/r/meta.json.tmp -> /home/user/.ghri/o/r/meta.json
        runtime
            .expect_rename()
            .with(eq(meta_tmp), eq(meta_path))
            .returning(|_, _| Ok(()));

        // --- Setup Source API Mock ---

        let mut source = MockSource::new();
        source
            .expect_api_url()
            .return_const("https://api.github.com".to_string());

        // API returns repo info
        source.expect_get_repo_metadata_at().returning(|_, _| {
            Ok(RepoMetadata {
                description: None,
                homepage: None,
                license: None,
                updated_at: Some("now".into()),
            })
        });

        // API returns one release v1 with tarball URL
        let download_url = format!("{}/tarball", url);
        source.expect_get_releases_at().return_once(move |_, _| {
            Ok(vec![SourceRelease {
                tag: "v1".into(),
                tarball_url: download_url,
                ..Default::default()
            }])
        });

        // --- Setup HTTP Server Mock ---

        // GET /tarball returns dummy data
        let _m = server
            .mock("GET", "/tarball")
            .with_status(200)
            .with_body("data")
            .create();

        // --- Setup Extractor Mock ---

        let mut extractor = MockArchiveExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- Execute ---

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        let installer = Installer::new(runtime, source, downloader, extractor);
        installer
            .install_legacy(&config, &repo, None, &default_install_options())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_get_or_fetch_meta_invalid_on_disk() {
        // Test that invalid meta.json on disk triggers re-fetch from GitHub API
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup Paths ---
        #[cfg(not(windows))]
        let meta_path = PathBuf::from("/home/user/.ghri/o/r/meta.json");
        #[cfg(windows)]
        let meta_path = PathBuf::from("C:\\Users\\user\\.ghri\\o\\r\\meta.json");

        // --- 1. Check for Existing Metadata ---

        // File exists: meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json -> returns INVALID JSON (triggers re-fetch)
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(|_| Ok("invalid json".into()));

        // --- 2. Re-fetch and Save Metadata (fallback) ---

        // Create package directory (for saving fetched meta)
        runtime.expect_create_dir_all().returning(|_| Ok(()));

        // Write fetched meta to temp file
        runtime.expect_write().returning(|_, _| Ok(()));

        // Rename temp file to meta.json
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Setup GitHub API Mock ---

        let mut github = MockSource::new();

        // API returns repo info
        github.expect_get_repo_metadata_at().returning(|_, _| {
            Ok(RepoMetadata {
                description: None,
                homepage: None,
                license: None,
                updated_at: Some("now".into()),
            })
        });

        // API returns empty releases list
        github.expect_get_releases_at().returning(|_, _| Ok(vec![]));

        // --- Execute & Verify ---

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        let installer = Installer::new(runtime, github, downloader, MockArchiveExtractor::new());
        let (meta, _, needs_save) = installer.get_or_fetch_meta(&config, &repo).await.unwrap();

        // Should have fetched fresh metadata (needs_save = true)
        assert_eq!(meta.name, "o/r");
        assert!(needs_save, "Fresh fetch should need saving");
    }

    // test_find_all_packages is now in package/discovery.rs

    // Meta tests are now in package/meta.rs
    // find_all_packages tests are now in package/discovery.rs
    // update_all tests are now in update.rs

    #[tokio::test]
    async fn test_install_no_stable_release() {
        // Test that install fails when only pre-release versions are available
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- 1. Fetch Metadata (no cached meta) ---

        // File exists: meta.json -> false (need to fetch)
        runtime.expect_exists().returning(|_| false);

        // Create package directory
        runtime.expect_create_dir_all().returning(|_| Ok(()));

        // Write and rename meta.json
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Setup GitHub API Mock ---

        let mut github = MockSource::new();
        github
            .expect_api_url()
            .return_const("https://api.github.com".to_string());

        // API returns repo info
        github.expect_get_repo_metadata_at().returning(|_, _| {
            Ok(RepoMetadata {
                description: None,
                homepage: None,
                license: None,
                updated_at: Some("now".into()),
            })
        });

        // API returns ONLY a pre-release version (no stable release!)
        github.expect_get_releases_at().returning(|_, _| {
            Ok(vec![SourceRelease {
                tag: "v1-rc".into(),
                prerelease: true, // This is a pre-release
                ..Default::default()
            }])
        });

        // --- Execute & Verify ---

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        let installer = Installer::new(runtime, github, downloader, MockArchiveExtractor::new());

        // Should fail because no stable release found
        let result = installer
            .install_legacy(&config, &repo, None, &default_install_options())
            .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No stable release found")
        );
    }

    #[tokio::test]
    async fn test_install_version_not_found() {
        // Test that install fails with descriptive error when specified version doesn't exist
        // Simplified: mock cached meta.json instead of GitHub API
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Mock cached meta.json with available versions ---

        // Prepare meta.json content with versions v1.0.0, v2.0.0, v3.0.0
        // Note: current_version must be set to avoid read_link call in Meta::load
        let meta_json = serde_json::json!({
            "name": "o/r",
            "api_url": "https://api.github.com",
            "current_version": "v3.0.0",
            "releases": [
                { "version": "v3.0.0", "published_at": "2024-03-01T00:00:00Z", "prerelease": false, "assets": [] },
                { "version": "v2.0.0", "published_at": "2024-02-01T00:00:00Z", "prerelease": false, "assets": [] },
                { "version": "v1.0.0", "published_at": "2024-01-01T00:00:00Z", "prerelease": false, "assets": [] }
            ]
        });

        // meta.json exists -> true (use cached)
        runtime.expect_exists().returning(|_| true);

        // Read meta.json -> returns valid JSON
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(meta_json.to_string()));

        // --- Execute & Verify ---

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        // No GitHub mock needed - meta is loaded from "cached" file
        let installer = Installer::new(
            runtime,
            MockSource::new(),
            downloader,
            MockArchiveExtractor::new(),
        );

        // Try to install version "v999.0.0" which doesn't exist
        let result = installer
            .install_legacy(&config, &repo, Some("v999.0.0"), &default_install_options())
            .await;

        // Should fail because requested version not found
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Version 'v999.0.0' not found"),
            "Error should mention the missing version, got: {}",
            err
        );
        assert!(
            err.contains("o/r"),
            "Error should mention the repo, got: {}",
            err
        );
        assert!(
            err.contains("v3.0.0") && err.contains("v2.0.0") && err.contains("v1.0.0"),
            "Error should list available versions, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_install_prerelease_with_pre_flag() {
        // Test that --pre flag allows selecting pre-release when no stable release exists
        // This test verifies that with pre=true, get_latest_release() is used instead of get_latest_stable_release()
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup Runtime Mocks ---

        // Metadata file doesn't exist yet
        runtime.expect_exists().returning(|_| false);

        // Create directories - allow any number of times
        runtime.expect_create_dir_all().returning(|_| Ok(()));

        // Write meta.json
        runtime.expect_write().returning(|_, _| Ok(()));

        // Rename meta.json.tmp -> meta.json
        runtime.expect_rename().returning(|_, _| Ok(()));

        // Create temp file for download
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));
        runtime.expect_remove_file().returning(|_| Ok(()));

        // Symlink operations
        runtime.expect_symlink().returning(|_, _| Ok(()));

        // --- Setup GitHub API Mock ---

        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mut github = MockSource::new();

        // API returns repo info
        github.expect_get_repo_metadata_at().returning(|_, _| {
            Ok(RepoMetadata {
                description: None,
                homepage: None,
                license: None,
                updated_at: Some("now".into()),
            })
        });

        // API returns ONLY a pre-release version (no stable release!)
        let download_url = format!("{}/tarball", url);
        github.expect_get_releases_at().return_once(move |_, _| {
            Ok(vec![SourceRelease {
                tag: "v1-rc".into(),
                prerelease: true, // This is a pre-release
                tarball_url: download_url,
                ..Default::default()
            }])
        });

        // --- Setup HTTP Server Mock ---

        let _m = server
            .mock("GET", "/tarball")
            .with_status(200)
            .with_body("data")
            .create();

        // --- Setup Extractor Mock ---

        let mut extractor = MockArchiveExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- Execute & Verify ---

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        let installer = Installer::new(runtime, github, downloader, extractor);

        // With pre=true, should succeed and install the pre-release
        let options = InstallOptions {
            pre: true,
            ..default_install_options()
        };
        let result = installer
            .install_legacy(&config, &repo, None, &options)
            .await;
        assert!(result.is_ok());
    }

    // default_install_root test is now in paths.rs

    // Meta tests (test_meta_releases_sorting, test_meta_merge_sorting, test_meta_sorting_fallback,
    // test_meta_conversions, test_meta_get_latest_stable_release*) are now in package/meta.rs
    // Symlink tests are now in symlink.rs

    #[tokio::test]
    async fn test_get_or_fetch_meta_fetch_fail() {
        // Test that get_or_fetch_meta fails when GitHub API returns error
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- 1. Check for Existing Metadata ---

        // File exists: meta.json -> false (need to fetch)
        runtime.expect_exists().returning(|_| false);

        // --- Setup GitHub API Mock (FAILS) ---

        let mut github = MockSource::new();
        github
            .expect_api_url()
            .return_const("https://api".to_string());

        // API call fails with error
        github
            .expect_get_repo_metadata_at()
            .returning(|_, _| Err(anyhow::anyhow!("fail")));

        // --- Execute & Verify ---

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        let installer = Installer::new(runtime, github, downloader, MockArchiveExtractor::new());

        // Should fail because API call failed
        let result = installer.get_or_fetch_meta(&config, &repo).await;
        assert!(result.is_err());
    }

    // Note: test_run_invalid_repo_str removed - tests run() entry point which now uses RegistryServices
    // The repo parsing logic is tested via RepoSpec::from_str in repo_spec.rs

    // More Meta tests that are now in package/meta.rs

    // Symlink tests are now in symlink.rs

    #[tokio::test]
    async fn test_save_metadata_failure_warning() {
        // Test that installation succeeds even when final metadata save fails
        // (save failure is just a warning, not a fatal error)
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- 1. Fetch Metadata (no cached meta) ---

        // File exists: meta.json -> false (triggers fresh fetch)
        runtime.expect_exists().returning(|_| false);

        // Create package directory (for version directory and meta.json)
        runtime.expect_create_dir_all().returning(|_| Ok(()));

        // --- 2. Download and Install ---

        // Create temp file for download
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));

        // Remove temp file after extraction
        runtime.expect_remove_file().returning(|_| Ok(()));

        // Create current symlink
        runtime.expect_symlink().returning(|_, _| Ok(()));

        // --- 3. Save Metadata After Install -> FAILS ---

        // Write meta.json -> FAILS (simulates disk full, permission denied, etc.)
        runtime
            .expect_write()
            .times(1)
            .returning(|_, _| Err(anyhow::anyhow!("disk full")));

        // --- Setup GitHub API Mock ---

        let mut github = MockSource::new();
        github
            .expect_api_url()
            .return_const("https://api".to_string());

        // API returns repo info
        github.expect_get_repo_metadata_at().returning(|_, _| {
            Ok(RepoMetadata {
                description: None,
                homepage: None,
                license: None,
                updated_at: None,
            })
        });

        // API returns one release v1
        let tar_url = format!("{}/tar", url);
        github.expect_get_releases_at().return_once(move |_, _| {
            Ok(vec![SourceRelease {
                tag: "v1".into(),
                tarball_url: tar_url,
                ..Default::default()
            }])
        });

        // --- Setup HTTP Server Mock ---

        let _m = server.mock("GET", "/tar").with_status(200).create();

        // --- Setup Extractor Mock ---

        let mut extractor = MockArchiveExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- Execute & Verify ---

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        let installer = Installer::new(runtime, github, downloader, extractor);

        // Should succeed despite metadata save failure (it's just a warning)
        let result = installer
            .install_legacy(&config, &repo, None, &default_install_options())
            .await;
        assert!(result.is_ok());
    }

    // default_install_root_privileged_mock test is now in paths.rs

    #[tokio::test]
    async fn test_get_or_fetch_meta_exists_interaction() {
        // Test that valid cached meta.json is used without fetching from API
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- 1. Load Existing Metadata (cache hit) ---

        // File exists: meta.json -> true (use cache)
        runtime.expect_exists().returning(|_| true);

        // Read meta.json -> valid JSON with current_version "v1"
        runtime.expect_read_to_string().returning(|_| Ok(r#"{"name":"o/r","api_url":"https://api.github.com","repo_info_url":"","releases_url":"","description":null,"homepage":null,"license":null,"updated_at":"","current_version":"v1","releases":[]}"#.into()));

        // --- Execute & Verify ---

        // No GitHub API mock needed - should use cached meta

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        let installer = Installer::new(
            runtime,
            MockSource::new(),
            downloader,
            MockArchiveExtractor::new(),
        );
        let (meta, _, needs_save) = installer.get_or_fetch_meta(&config, &repo).await.unwrap();

        // Should return cached metadata (needs_save = false)
        assert_eq!(meta.name, "o/r");
        assert!(!needs_save, "Cached meta should not need saving");
    }

    #[tokio::test]
    async fn test_install_uses_existing_meta() {
        // Test that install uses cached metadata and skips API fetch
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let github = MockSource::new();

        // --- 1. Load Existing Metadata (cache hit) ---

        // File exists: meta.json -> true (use cache)
        runtime.expect_exists().returning(|_| true);

        // Read meta.json -> valid JSON with v1 release
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            api_url: "api".into(),
            releases: vec![
                SourceRelease {
                    tag: "v1".into(),
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Check if meta.json is a directory -> false
        runtime.expect_is_dir().returning(|_| false);

        // --- 2. Check Version Already Installed ---

        // Version dir exists: already installed, skip download
        runtime.expect_exists().returning(|_| true);

        // --- 3. Update Current Symlink ---

        // Check if current symlink exists -> true
        runtime.expect_is_symlink().returning(|_| true);

        // Read current symlink -> points to v1 (same version)
        runtime
            .expect_read_link()
            .returning(|_| Ok(PathBuf::from("v1")));

        // Symlink update (may be called for refresh)
        runtime.expect_symlink().returning(|_, _| Ok(()));

        // --- 4. Save Updated Metadata ---

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        let installer = Installer::new(runtime, github, downloader, MockArchiveExtractor::new());
        installer
            .install_legacy(&config, &repo, None, &default_install_options())
            .await
            .unwrap();
    }

    // Note: test_run removed - tests run() entry point which now uses RegistryServices
    // The install logic is tested via Installer::install tests above

    // test_update_current_symlink_no_op_if_already_correct is now in symlink.rs

    #[tokio::test]
    async fn test_update_atomic_safety() {
        // Test that metadata is saved atomically (write to .tmp then rename)
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));

        // --- Setup Write Sequence ---
        // Must write to .tmp file FIRST, then rename to final .json file

        let mut seq = mockall::Sequence::new();

        // Step 1: Write to meta.json.tmp
        runtime
            .expect_write()
            .withf(|p, _| p.to_string_lossy().ends_with(".tmp"))
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));

        // Step 2: Rename meta.json.tmp -> meta.json (atomic on most filesystems)
        runtime
            .expect_rename()
            .withf(|from, to| {
                from.to_string_lossy().ends_with(".tmp") && to.to_string_lossy().ends_with(".json")
            })
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));

        // --- Execute ---

        let meta = Meta {
            name: "o/r".into(),
            ..Default::default()
        };
        let downloader = MockDownloader::new();
        Installer::new(
            runtime,
            MockSource::new(),
            downloader,
            MockArchiveExtractor::new(),
        )
        .save_meta(&PathBuf::from("meta.json"), &meta)
        .unwrap();
    }

    // test_update_timestamp_behavior is now in package/meta.rs

    #[tokio::test]
    async fn test_install_user_filters_override_saved_filters() {
        // Test that user-provided --filter args completely override saved filters in meta.json
        // When user specifies filters, meta.json filters should be ignored
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let github = MockSource::new();

        // --- Setup Paths ---
        #[cfg(not(windows))]
        let meta_path = PathBuf::from("/home/user/.ghri/o/r/meta.json");
        #[cfg(windows)]
        let meta_path = PathBuf::from("C:\\Users\\user\\.ghri\\o\\r\\meta.json");

        // --- 1. Load Existing Metadata with SAVED filters ---

        // File exists: meta.json -> true (use cache)
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json -> has saved filters ["*old-filter*"]
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            api_url: "api".into(),
            filters: vec!["*old-filter*".to_string()], // Saved filter from previous install
            releases: vec![
                SourceRelease {
                    tag: "v1".into(),
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Check if meta.json is a directory -> false
        runtime.expect_is_dir().returning(|_| false);

        // --- 2. Version Already Installed (skip download) ---

        runtime.expect_exists().returning(|_| true);

        // --- 3. Update Current Symlink ---

        runtime.expect_is_symlink().returning(|_| true);
        runtime
            .expect_read_link()
            .returning(|_| Ok(PathBuf::from("v1")));
        runtime.expect_symlink().returning(|_, _| Ok(()));

        // --- 4. Save Updated Metadata with NEW filters ---

        // Verify that new user-provided filters are saved (not the old ones)
        runtime
            .expect_write()
            .withf(|_, content| {
                let saved_meta: Meta = serde_json::from_slice(content).unwrap();
                // New filters should be saved, NOT the old "*old-filter*"
                saved_meta.filters == vec!["*new-user-filter*"]
            })
            .returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        let installer = Installer::new(runtime, github, downloader, MockArchiveExtractor::new());

        // User provides NEW filter, should override saved "*old-filter*"
        let options = InstallOptions {
            filters: vec!["*new-user-filter*".to_string()],
            ..default_install_options()
        };
        installer
            .install_legacy(&config, &repo, None, &options)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_install_uses_saved_filters_when_user_provides_none() {
        // Test that saved filters from meta.json are used when user doesn't provide any
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let github = MockSource::new();

        // --- Setup Paths ---
        #[cfg(not(windows))]
        let meta_path = PathBuf::from("/home/user/.ghri/o/r/meta.json");
        #[cfg(windows)]
        let meta_path = PathBuf::from("C:\\Users\\user\\.ghri\\o\\r\\meta.json");

        // --- 1. Load Existing Metadata with SAVED filters ---

        // File exists: meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Read meta.json -> has saved filters ["*linux*", "*x86_64*"]
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            api_url: "api".into(),
            filters: vec!["*linux*".to_string(), "*x86_64*".to_string()],
            releases: vec![
                SourceRelease {
                    tag: "v1".into(),
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        // Check if meta.json is a directory -> false
        runtime.expect_is_dir().returning(|_| false);

        // --- 2. Version Already Installed ---

        runtime.expect_exists().returning(|_| true);

        // --- 3. Update Current Symlink ---

        runtime.expect_is_symlink().returning(|_| true);
        runtime
            .expect_read_link()
            .returning(|_| Ok(PathBuf::from("v1")));
        runtime.expect_symlink().returning(|_, _| Ok(()));

        // --- 4. Save Metadata (filters should remain the same) ---

        runtime
            .expect_write()
            .withf(|_, content| {
                let saved_meta: Meta = serde_json::from_slice(content).unwrap();
                // Saved filters should be preserved
                saved_meta.filters == vec!["*linux*", "*x86_64*"]
            })
            .returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        let installer = Installer::new(runtime, github, downloader, MockArchiveExtractor::new());

        // User provides NO filters -> should use saved filters from meta.json
        installer
            .install_legacy(&config, &repo, None, &default_install_options())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_install_clears_saved_filters_when_user_provides_empty() {
        // Test edge case: if user explicitly wants to clear filters, empty vec should clear saved filters
        // Note: Currently there's no way to distinguish "user didn't specify --filter" from
        // "user wants to clear filters". This test documents the current behavior where
        // empty filters from user will use saved filters.
        let mut runtime = MockRuntime::new();
        runtime
            .expect_temp_dir()
            .returning(|| PathBuf::from("/tmp"));
        let github = MockSource::new();

        // --- Setup Paths ---
        #[cfg(not(windows))]
        let meta_path = PathBuf::from("/home/user/.ghri/o/r/meta.json");
        #[cfg(windows)]
        let meta_path = PathBuf::from("C:\\Users\\user\\.ghri\\o\\r\\meta.json");

        // --- 1. Load Existing Metadata with SAVED filters ---

        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            api_url: "api".into(),
            filters: vec!["*saved-filter*".to_string()],
            releases: vec![
                SourceRelease {
                    tag: "v1".into(),
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(move |_| Ok(serde_json::to_string(&meta).unwrap()));

        runtime.expect_is_dir().returning(|_| false);
        runtime.expect_exists().returning(|_| true);
        runtime.expect_is_symlink().returning(|_| true);
        runtime
            .expect_read_link()
            .returning(|_| Ok(PathBuf::from("v1")));
        runtime.expect_symlink().returning(|_, _| Ok(()));

        // --- Verify saved filters are used (current behavior) ---

        runtime
            .expect_write()
            .withf(|_, content| {
                let saved_meta: Meta = serde_json::from_slice(content).unwrap();
                // When user provides empty vec, saved filters are used (current behavior)
                saved_meta.filters == vec!["*saved-filter*"]
            })
            .returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---

        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let config = test_config();
        let downloader = MockDownloader::new();
        let installer = Installer::new(runtime, github, downloader, MockArchiveExtractor::new());

        // Empty filters vec -> uses saved filters (current behavior)
        installer
            .install_legacy(&config, &repo, None, &default_install_options())
            .await
            .unwrap();
    }
}
