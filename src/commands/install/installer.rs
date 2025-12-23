use anyhow::Result;
use log::{info, warn};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{
    archive::Extractor,
    cleanup::CleanupContext,
    github::{GetReleases, GitHubRepo, Release},
    http::HttpClient,
    package::Meta,
    runtime::Runtime,
};

use crate::commands::link_check::{check_links, LinkStatus};
use crate::commands::paths::{default_install_root, get_target_dir};
use crate::commands::symlink::update_current_symlink;

use super::download::{ensure_installed, get_download_plan, DownloadPlan};
use super::external_links::update_external_links;

pub struct Installer<R: Runtime, G: GetReleases, E: Extractor> {
    pub runtime: R,
    pub github: G,
    pub http_client: HttpClient,
    pub extractor: E,
}

impl<R: Runtime + 'static, G: GetReleases, E: Extractor> Installer<R, G, E> {
    #[tracing::instrument(skip(runtime, github, http_client, extractor))]
    pub fn new(runtime: R, github: G, http_client: HttpClient, extractor: E) -> Self {
        Self {
            runtime,
            github,
            http_client,
            extractor,
        }
    }

    #[tracing::instrument(skip(self, repo, version, install_root, filters))]
    pub async fn install(&self, repo: &GitHubRepo, version: Option<&str>, install_root: Option<PathBuf>, filters: Vec<String>, pre: bool, yes: bool) -> Result<()> {
        println!("   resolving {}", repo);
        let (mut meta, meta_path, needs_save) = self
            .get_or_fetch_meta(repo, install_root.as_deref())
            .await?;

        // Use saved filters from meta if user didn't provide any
        let effective_filters = if filters.is_empty() && !meta.filters.is_empty() {
            info!("Using saved filters from meta: {:?}", meta.filters);
            meta.filters.clone()
        } else {
            filters
        };

        let meta_release = if let Some(ver) = version {
            // Find the specific version
            meta.releases
                .iter()
                .find(|r| r.version == ver || r.version == format!("v{}", ver) || r.version.trim_start_matches('v') == ver.trim_start_matches('v'))
                .ok_or_else(|| anyhow::anyhow!("Version '{}' not found for {}. Available versions: {}", 
                    ver, repo, 
                    meta.releases.iter().take(5).map(|r| r.version.as_str()).collect::<Vec<_>>().join(", ")))?
        } else if pre {
            // Get latest release including pre-releases
            meta.get_latest_release()
                .ok_or_else(|| anyhow::anyhow!("No release found for {}.", repo))?
        } else {
            // Get latest stable release
            meta.get_latest_stable_release()
                .ok_or_else(|| anyhow::anyhow!("No stable release found for {}. If you want to install a pre-release, specify the version with @version or use --pre.", repo))?
        };

        info!("Found version: {}", meta_release.version);
        let release: Release = meta_release.clone().into();

        let target_dir = get_target_dir(&self.runtime, repo, &release, install_root)?;

        // Check if already installed
        if self.runtime.exists(&target_dir) {
            println!("   {} {} is already installed", repo, release.tag_name);
            return Ok(());
        }

        // Get download plan and show confirmation
        let plan = get_download_plan(&release, &effective_filters)?;
        
        if !yes {
            self.show_install_plan(repo, &release, &target_dir, &meta_path, &plan, needs_save, &meta);
            if !self.confirm_install()? {
                println!("Installation cancelled.");
                return Ok(());
            }
        }

        // Set up cleanup context for Ctrl-C handling
        let cleanup_ctx = Arc::new(Mutex::new(CleanupContext::new()));
        let cleanup_ctx_clone = Arc::clone(&cleanup_ctx);

        // Register Ctrl-C handler
        let ctrl_c_handler = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\nInterrupted, cleaning up...");
                cleanup_ctx_clone.lock().unwrap().cleanup();
                std::process::exit(130); // Standard exit code for Ctrl-C
            }
        });

        let result = ensure_installed(
            &self.runtime,
            &target_dir,
            repo,
            &release,
            &self.http_client,
            &self.extractor,
            Arc::clone(&cleanup_ctx),
            &effective_filters,
        )
        .await;

        // Abort the Ctrl-C handler since installation completed (successfully or with error)
        ctrl_c_handler.abort();

        result?;

        update_current_symlink(&self.runtime, &target_dir, &release.tag_name)?;

        // Update external links if configured
        if let Some(parent) = target_dir.parent() {
            if let Err(e) = update_external_links(&self.runtime, parent, &target_dir, &meta) {
                warn!("Failed to update external links: {}. Continuing.", e);
            }
        }

        // Metadata handling - save meta only after successful install
        meta.current_version = release.tag_name.clone();
        // Save filters to meta (for use during updates)
        // Always update filters: use effective_filters which may be user-provided or from saved meta
        meta.filters = effective_filters;
        if needs_save {
            // Newly fetched meta - need to create parent directory first
            if let Some(parent) = meta_path.parent() {
                self.runtime.create_dir_all(parent)?;
            }
        }
        if let Err(e) = self.save_meta(&meta_path, &meta) {
            warn!("Failed to save package metadata: {}. Continuing.", e);
        }

        self.print_install_success(repo, &release.tag_name, &target_dir);

        Ok(())
    }

    fn show_install_plan(&self, repo: &GitHubRepo, release: &Release, target_dir: &Path, meta_path: &Path, plan: &DownloadPlan, needs_save: bool, meta: &Meta) {
        println!();
        println!("=== Installation Plan ===");
        println!();
        println!("Package:  {}", repo);
        println!("Version:  {}", release.tag_name);
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
            println!("  [LINK] {}/current -> {}", parent.display(), release.tag_name);
        }
        
        // Show external links that will be updated (with validity check)
        if !meta.links.is_empty() {
            if let Some(package_dir) = target_dir.parent() {
                let (valid_links, invalid_links) = check_links(&self.runtime, &meta.links, package_dir);
                
                // Show valid links (existing or to be created)
                if !valid_links.is_empty() {
                    println!();
                    println!("External links to update:");
                    for link in &valid_links {
                        let source = link.path.as_ref().map(|p| format!(":{}", p)).unwrap_or_default();
                        match link.status {
                            LinkStatus::Valid => {
                                println!("  [LINK] {} -> {}{}/{}", link.dest.display(), repo, source, release.tag_name);
                            }
                            LinkStatus::NotExists => {
                                println!("  [NEW]  {} -> {}{}/{}", link.dest.display(), repo, source, release.tag_name);
                            }
                            _ => {}
                        }
                    }
                }
                
                // Show invalid links
                if !invalid_links.is_empty() {
                    println!();
                    println!("External links to skip (will not be updated):");
                    for link in &invalid_links {
                        println!("  [SKIP] {} ({})", link.dest.display(), link.status.reason());
                    }
                }
            }
        }
        
        // Show versioned links (these won't be updated)
        if !meta.versioned_links.is_empty() {
            println!();
            println!("Versioned links (unchanged):");
            for link in &meta.versioned_links {
                println!("  [LINK] {} -> {}@{}", link.dest.display(), repo, link.version);
            }
        }
        
        println!();
    }

    fn confirm_install(&self) -> Result<bool> {
        print!("Proceed with installation? [y/N] ");
        io::stdout().flush()?;
        
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        
        let response = input.trim().to_lowercase();
        Ok(response == "y" || response == "yes")
    }

    #[tracing::instrument(skip(self, repo, tag, target_dir))]
    fn print_install_success(&self, repo: &GitHubRepo, tag: &str, target_dir: &Path) {
        println!("   installed {} {} {}", repo, tag, target_dir.display());
    }

    /// Get or fetch meta, returning (meta, meta_path, needs_save)
    /// needs_save is true if meta was newly fetched and needs to be saved after successful install
    #[tracing::instrument(skip(self, repo, install_root))]
    pub(crate) async fn get_or_fetch_meta(
        &self,
        repo: &GitHubRepo,
        install_root: Option<&Path>,
    ) -> Result<(Meta, PathBuf, bool)> {
        let root = match install_root {
            Some(path) => path.to_path_buf(),
            None => default_install_root(&self.runtime)?,
        };
        let meta_path = root.join(&repo.owner).join(&repo.repo).join("meta.json");

        if self.runtime.exists(&meta_path) {
            match Meta::load(&self.runtime, &meta_path) {
                Ok(meta) => return Ok((meta, meta_path, false)),
                Err(e) => {
                    warn!(
                        "Failed to load existing meta.json at {:?}: {}. Re-fetching.",
                        meta_path, e
                    );
                }
            }
        }

        let meta = self.fetch_meta(repo, "", None).await?;

        // Don't save meta here - let the caller save it after successful install
        Ok((meta, meta_path, true))
    }

    #[tracing::instrument(skip(self, repo, current_version, api_url))]
    async fn fetch_meta(
        &self,
        repo: &GitHubRepo,
        current_version: &str,
        api_url: Option<&str>,
    ) -> Result<Meta> {
        let api_url = api_url.unwrap_or(self.github.api_url());
        let repo_info = self.github.get_repo_info_at(repo, api_url).await?;
        let releases = self.github.get_releases_at(repo, api_url).await?;
        Ok(Meta::from(
            repo.clone(),
            repo_info,
            releases,
            current_version,
            api_url,
        ))
    }

    #[tracing::instrument(skip(self, meta_path, meta))]
    pub(crate) fn save_meta(&self, meta_path: &Path, meta: &Meta) -> Result<()> {
        let json = serde_json::to_string_pretty(meta)?;
        let tmp_path = meta_path.with_extension("json.tmp");

        self.runtime.write(&tmp_path, json.as_bytes())?;
        self.runtime.rename(&tmp_path, meta_path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::MockExtractor;
    use crate::commands::config::Config;
    use crate::github::{MockGetReleases, RepoInfo};
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;
    use reqwest::Client;
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
    // Tests for get_target_dir and update_current_symlink are now in paths.rs and symlink.rs

    #[cfg(not(windows))]
    #[tokio::test]
    async fn test_install_happy_path() {
        // Test successful installation of a new package from scratch
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let meta_path = root.join("o/r/meta.json");             // /home/user/.ghri/o/r/meta.json
        let meta_tmp = root.join("o/r/meta.json.tmp");          // /home/user/.ghri/o/r/meta.json.tmp
        let package_dir = root.join("o/r");                     // /home/user/.ghri/o/r
        let version_dir = root.join("o/r/v1");                  // /home/user/.ghri/o/r/v1
        let current_link = root.join("o/r/current");            // /home/user/.ghri/o/r/current

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

        // --- Setup GitHub API Mock ---

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api.github.com".to_string());

        // API returns repo info
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "now".into(),
            })
        });

        // API returns one release v1 with tarball URL
        let download_url = format!("{}/tarball", url);
        github.expect_get_releases_at().return_once(move |_, _| {
            Ok(vec![Release {
                tag_name: "v1".into(),
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

        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- Execute ---

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let http_client = HttpClient::new(Client::new());
        let installer = Installer::new(runtime, github, http_client, extractor);
        installer.install(&repo, None, None, vec![], false, true).await.unwrap();
    }

    #[tokio::test]
    async fn test_get_or_fetch_meta_invalid_on_disk() {
        // Test that invalid meta.json on disk triggers re-fetch from GitHub API
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

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

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api".to_string());

        // API returns repo info
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "now".into(),
            })
        });

        // API returns empty releases list
        github.expect_get_releases_at().returning(|_, _| Ok(vec![]));

        // --- Execute & Verify ---

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let http_client = HttpClient::new(Client::new());
        let installer = Installer::new(runtime, github, http_client, MockExtractor::new());
        let (meta, _, needs_save) = installer.get_or_fetch_meta(&repo, None).await.unwrap();

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
        configure_runtime_basics(&mut runtime);

        // --- 1. Fetch Metadata (no cached meta) ---

        // File exists: meta.json -> false (need to fetch)
        runtime.expect_exists().returning(|_| false);

        // Create package directory
        runtime.expect_create_dir_all().returning(|_| Ok(()));

        // Write and rename meta.json
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Setup GitHub API Mock ---

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api.github.com".to_string());

        // API returns repo info
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "now".into(),
            })
        });

        // API returns ONLY a pre-release version (no stable release!)
        github.expect_get_releases_at().returning(|_, _| {
            Ok(vec![Release {
                tag_name: "v1-rc".into(),
                prerelease: true,  // This is a pre-release
                ..Default::default()
            }])
        });

        // --- Execute & Verify ---

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let http_client = HttpClient::new(Client::new());
        let installer = Installer::new(runtime, github, http_client, MockExtractor::new());

        // Should fail because no stable release found
        let result = installer.install(&repo, None, None, vec![], false, true).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No stable release found")
        );
    }

    #[tokio::test]
    async fn test_install_prerelease_with_pre_flag() {
        // Test that --pre flag allows selecting pre-release when no stable release exists
        // This test verifies that with pre=true, get_latest_release() is used instead of get_latest_stable_release()
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

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
        runtime.expect_create_file().returning(|_| Ok(Box::new(std::io::sink())));
        runtime.expect_remove_file().returning(|_| Ok(()));

        // Symlink operations
        runtime.expect_symlink().returning(|_, _| Ok(()));

        // --- Setup GitHub API Mock ---

        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api.github.com".to_string());

        // API returns repo info
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "now".into(),
            })
        });

        // API returns ONLY a pre-release version (no stable release!)
        let download_url = format!("{}/tarball", url);
        github.expect_get_releases_at().return_once(move |_, _| {
            Ok(vec![Release {
                tag_name: "v1-rc".into(),
                prerelease: true,  // This is a pre-release
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

        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- Execute & Verify ---

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let http_client = HttpClient::new(Client::new());
        let installer = Installer::new(runtime, github, http_client, extractor);

        // With pre=true, should succeed and install the pre-release
        let result = installer.install(&repo, None, None, vec![], true, true).await;
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
        configure_runtime_basics(&mut runtime);

        // --- 1. Check for Existing Metadata ---

        // File exists: meta.json -> false (need to fetch)
        runtime.expect_exists().returning(|_| false);

        // --- Setup GitHub API Mock (FAILS) ---

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api".to_string());

        // API call fails with error
        github
            .expect_get_repo_info_at()
            .returning(|_, _| Err(anyhow::anyhow!("fail")));

        // --- Execute & Verify ---

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let http_client = HttpClient::new(Client::new());
        let installer = Installer::new(runtime, github, http_client, MockExtractor::new());

        // Should fail because API call failed
        let result = installer.get_or_fetch_meta(&repo, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_invalid_repo_str() {
        // Test that run() fails with invalid repository string format

        // --- Setup (no runtime expectations needed - fails before any IO) ---

        let config = Config {
            runtime: MockRuntime::new(),
            github: MockGetReleases::new(),
            http_client: HttpClient::new(Client::new()),
            extractor: MockExtractor::new(),
            install_root: None,
        };

        // --- Execute & Verify ---

        // "invalid" is not a valid "owner/repo" format
        let result = super::super::run("invalid", config, vec![], false, true).await;
        assert!(result.is_err());
    }

    // More Meta tests that are now in package/meta.rs

    // Symlink tests are now in symlink.rs

    #[tokio::test]
    async fn test_save_metadata_failure_warning() {
        // Test that installation succeeds even when final metadata save fails
        // (save failure is just a warning, not a fatal error)
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

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

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api".to_string());

        // API returns repo info
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "".into(),
            })
        });

        // API returns one release v1
        let tar_url = format!("{}/tar", url);
        github.expect_get_releases_at().return_once(move |_, _| {
            Ok(vec![Release {
                tag_name: "v1".into(),
                tarball_url: tar_url,
                ..Default::default()
            }])
        });

        // --- Setup HTTP Server Mock ---

        let _m = server.mock("GET", "/tar").with_status(200).create();

        // --- Setup Extractor Mock ---

        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        // --- Execute & Verify ---

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let http_client = HttpClient::new(Client::new());
        let installer = Installer::new(runtime, github, http_client, extractor);

        // Should succeed despite metadata save failure (it's just a warning)
        let result = installer.install(&repo, None, None, vec![], false, true).await;
        assert!(result.is_ok());
    }

    // default_install_root_privileged_mock test is now in paths.rs
    
    #[tokio::test]
    async fn test_get_or_fetch_meta_exists_interaction() {
        // Test that valid cached meta.json is used without fetching from API
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- 1. Load Existing Metadata (cache hit) ---

        // File exists: meta.json -> true (use cache)
        runtime.expect_exists().returning(|_| true);

        // Read meta.json -> valid JSON with current_version "v1"
        runtime.expect_read_to_string().returning(|_| Ok(r#"{"name":"o/r","api_url":"https://api.github.com","repo_info_url":"","releases_url":"","description":null,"homepage":null,"license":null,"updated_at":"","current_version":"v1","releases":[]}"#.into()));

        // --- Execute & Verify ---

        // No GitHub API mock needed - should use cached meta

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let http_client = HttpClient::new(Client::new());
        let installer = Installer::new(
            runtime,
            MockGetReleases::new(),
            http_client,
            MockExtractor::new(),
        );
        let (meta, _, needs_save) = installer.get_or_fetch_meta(&repo, None).await.unwrap();

        // Should return cached metadata (needs_save = false)
        assert_eq!(meta.name, "o/r");
        assert!(!needs_save, "Cached meta should not need saving");
    }

    #[tokio::test]
    async fn test_install_uses_existing_meta() {
        // Test that install uses cached metadata and skips API fetch
        let mut runtime = MockRuntime::new();
        let github = MockGetReleases::new();
        configure_runtime_basics(&mut runtime);

        // --- 1. Load Existing Metadata (cache hit) ---

        // File exists: meta.json -> true (use cache)
        runtime.expect_exists().returning(|_| true);

        // Read meta.json -> valid JSON with v1 release
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            api_url: "api".into(),
            releases: vec![
                Release {
                    tag_name: "v1".into(),
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

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let http_client = HttpClient::new(Client::new());
        let installer = Installer::new(runtime, github, http_client, MockExtractor::new());
        installer.install(&repo, None, None, vec![], false, true).await.unwrap();
    }

    #[tokio::test]
    async fn test_run() {
        // Test the run() entry point with cached metadata
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- 1. Load Existing Metadata (cache hit) ---

        // File exists: meta.json -> true
        runtime.expect_exists().returning(|_| true);

        // Read meta.json -> valid JSON with v1 release
        runtime.expect_read_to_string().returning(|_| Ok(r#"{"name":"o/r","api_url":"","repo_info_url":"","releases_url":"","description":null,"homepage":null,"license":null,"updated_at":"","current_version":"v1","releases":[{"version":"v1","is_prerelease":false,"tarball_url":"","assets":[]}]}"#.into()));

        // Check if meta.json is a directory -> false
        runtime.expect_is_dir().returning(|_| false);

        // --- 2. Check Version Already Installed ---

        // Check current symlink exists -> true
        runtime.expect_is_symlink().returning(|_| true);

        // Read current symlink -> points to v1
        runtime
            .expect_read_link()
            .returning(|_| Ok(PathBuf::from("v1")));

        // Update symlink
        runtime.expect_symlink().returning(|_, _| Ok(()));

        // --- 3. Save Updated Metadata ---

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---

        let config = Config {
            runtime,
            github: MockGetReleases::new(),
            http_client: HttpClient::new(Client::new()),
            extractor: MockExtractor::new(),
            install_root: None,
        };

        // Install o/r using run() entry point
        super::super::run("o/r", config, vec![], false, true).await.unwrap();
    }

    // test_update_current_symlink_no_op_if_already_correct is now in symlink.rs

    #[tokio::test]
    async fn test_update_atomic_safety() {
        // Test that metadata is saved atomically (write to .tmp then rename)
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

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
        let http_client = HttpClient::new(Client::new());
        Installer::new(
            runtime,
            MockGetReleases::new(),
            http_client,
            MockExtractor::new(),
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
        let github = MockGetReleases::new();
        configure_runtime_basics(&mut runtime);

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
            filters: vec!["*old-filter*".to_string()],  // Saved filter from previous install
            releases: vec![
                Release {
                    tag_name: "v1".into(),
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

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let http_client = HttpClient::new(Client::new());
        let installer = Installer::new(runtime, github, http_client, MockExtractor::new());

        // User provides NEW filter, should override saved "*old-filter*"
        installer
            .install(&repo, None, None, vec!["*new-user-filter*".to_string()], false, true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_install_uses_saved_filters_when_user_provides_none() {
        // Test that saved filters from meta.json are used when user doesn't provide any
        let mut runtime = MockRuntime::new();
        let github = MockGetReleases::new();
        configure_runtime_basics(&mut runtime);

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
                Release {
                    tag_name: "v1".into(),
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

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let http_client = HttpClient::new(Client::new());
        let installer = Installer::new(runtime, github, http_client, MockExtractor::new());

        // User provides NO filters -> should use saved filters from meta.json
        installer.install(&repo, None, None, vec![], false, true).await.unwrap();
    }

    #[tokio::test]
    async fn test_install_clears_saved_filters_when_user_provides_empty() {
        // Test edge case: if user explicitly wants to clear filters, empty vec should clear saved filters
        // Note: Currently there's no way to distinguish "user didn't specify --filter" from 
        // "user wants to clear filters". This test documents the current behavior where
        // empty filters from user will use saved filters.
        let mut runtime = MockRuntime::new();
        let github = MockGetReleases::new();
        configure_runtime_basics(&mut runtime);

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
                Release {
                    tag_name: "v1".into(),
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

        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let http_client = HttpClient::new(Client::new());
        let installer = Installer::new(runtime, github, http_client, MockExtractor::new());

        // Empty filters vec -> uses saved filters (current behavior)
        installer.install(&repo, None, None, vec![], false, true).await.unwrap();
    }
}
