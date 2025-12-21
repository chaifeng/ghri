use anyhow::{Context, Result};
use log::{info, warn};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{
    archive::Extractor,
    cleanup::CleanupContext,
    github::{GetReleases, GitHubRepo, Release, RepoSpec},
    package::{Meta, find_all_packages},
    runtime::Runtime,
};

use super::config::Config;
use super::paths::{default_install_root, get_target_dir};
use super::symlink::update_current_symlink;

mod download;
mod external_links;

use download::ensure_installed;
pub(crate) use external_links::update_external_links;

#[tracing::instrument(skip(runtime, install_root, api_url))]
pub async fn install<R: Runtime + 'static>(
    runtime: R,
    repo_str: &str,
    install_root: Option<PathBuf>,
    api_url: Option<String>,
) -> Result<()> {
    let config = Config::new(runtime, install_root, api_url)?;
    run(repo_str, config).await
}

#[tracing::instrument(skip(config))]
pub async fn run<R: Runtime + 'static, G: GetReleases, E: Extractor>(
    repo_str: &str,
    config: Config<R, G, E>,
) -> Result<()> {
    let spec = repo_str.parse::<RepoSpec>()?;
    let installer = Installer::new(
        config.runtime,
        config.github,
        config.client,
        config.extractor,
    );
    installer.install(&spec.repo, spec.version.as_deref(), config.install_root).await
}

pub struct Installer<R: Runtime, G: GetReleases, E: Extractor> {
    pub runtime: R,
    pub github: G,
    pub client: Client,
    pub extractor: E,
}

impl<R: Runtime + 'static, G: GetReleases, E: Extractor> Installer<R, G, E> {
    #[tracing::instrument(skip(runtime, github, client, extractor))]
    pub fn new(runtime: R, github: G, client: Client, extractor: E) -> Self {
        Self {
            runtime,
            github,
            client,
            extractor,
        }
    }

    #[tracing::instrument(skip(self, repo, version, install_root))]
    pub async fn install(&self, repo: &GitHubRepo, version: Option<&str>, install_root: Option<PathBuf>) -> Result<()> {
        println!("   resolving {}", repo);
        let (mut meta, meta_path) = self
            .get_or_fetch_meta(repo, install_root.as_deref())
            .await?;

        let meta_release = if let Some(ver) = version {
            // Find the specific version
            meta.releases
                .iter()
                .find(|r| r.version == ver || r.version == format!("v{}", ver) || r.version.trim_start_matches('v') == ver.trim_start_matches('v'))
                .ok_or_else(|| anyhow::anyhow!("Version '{}' not found for {}. Available versions: {}", 
                    ver, repo, 
                    meta.releases.iter().take(5).map(|r| r.version.as_str()).collect::<Vec<_>>().join(", ")))?
        } else {
            // Get latest stable release
            meta.get_latest_stable_release()
                .ok_or_else(|| anyhow::anyhow!("No stable release found for {}. If you want to install a pre-release, specify the version with @version.", repo))?
        };

        info!("Found version: {}", meta_release.version);
        let release: Release = meta_release.clone().into();

        let target_dir = get_target_dir(&self.runtime, repo, &release, install_root)?;

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
            &self.client,
            &self.extractor,
            Arc::clone(&cleanup_ctx),
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

        // Metadata handling
        meta.current_version = release.tag_name.clone();
        if let Err(e) = self.save_meta(&meta_path, &meta) {
            warn!("Failed to save package metadata: {}. Continuing.", e);
        }

        self.print_install_success(repo, &release.tag_name, &target_dir);

        Ok(())
    }

    #[tracing::instrument(skip(self, install_root))]
    pub async fn update_all(&self, install_root: Option<PathBuf>) -> Result<()> {
        let root = match install_root {
            Some(path) => path,
            None => default_install_root(&self.runtime)?,
        };

        let meta_files = find_all_packages(&self.runtime, &root)?;
        if meta_files.is_empty() {
            println!("No packages installed.");
            return Ok(());
        }

        for meta_path in meta_files {
            let meta = Meta::load(&self.runtime, &meta_path)?;
            let repo = meta.name.parse::<GitHubRepo>()?;

            println!("   updating {}", repo);
            if let Err(e) = self
                .save_metadata(&repo, &meta.current_version, &meta_path)
                .await
            {
                warn!("Failed to update metadata for {}: {}", repo, e);
            } else {
                // Check if update is available
                let updated_meta = Meta::load(&self.runtime, &meta_path)?;
                if let Some(latest) = updated_meta.get_latest_stable_release()
                    && meta.current_version != latest.version
                {
                    self.print_update_available(&repo, &meta.current_version, &latest.version);
                }
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip(self, repo, tag, target_dir))]
    fn print_install_success(&self, repo: &GitHubRepo, tag: &str, target_dir: &Path) {
        println!("   installed {} {} {}", repo, tag, target_dir.display());
    }

    #[tracing::instrument(skip(self, repo, current, latest))]
    fn print_update_available(&self, repo: &GitHubRepo, current: &str, latest: &str) {
        let current_display = if current.is_empty() {
            "(none)"
        } else {
            current
        };
        println!("  updatable {} {} -> {}", repo, current_display, latest);
    }

    #[tracing::instrument(skip(self, repo, install_root))]
    async fn get_or_fetch_meta(
        &self,
        repo: &GitHubRepo,
        install_root: Option<&Path>,
    ) -> Result<(Meta, PathBuf)> {
        let root = match install_root {
            Some(path) => path.to_path_buf(),
            None => default_install_root(&self.runtime)?,
        };
        let meta_path = root.join(&repo.owner).join(&repo.repo).join("meta.json");

        if self.runtime.exists(&meta_path) {
            match Meta::load(&self.runtime, &meta_path) {
                Ok(meta) => return Ok((meta, meta_path)),
                Err(e) => {
                    warn!(
                        "Failed to load existing meta.json at {:?}: {}. Re-fetching.",
                        meta_path, e
                    );
                }
            }
        }

        let meta = self.fetch_meta(repo, "", None).await?;

        if let Some(parent) = meta_path.parent() {
            self.runtime.create_dir_all(parent)?;
        }
        self.save_meta(&meta_path, &meta)?;

        Ok((meta, meta_path))
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

    #[tracing::instrument(skip(self, repo, current_version, target_dir))]
    async fn save_metadata(
        &self,
        repo: &GitHubRepo,
        current_version: &str,
        target_dir: &Path,
    ) -> Result<()> {
        let meta_path = if !self.runtime.is_dir(target_dir) {
            target_dir.to_path_buf()
        } else {
            let package_root = target_dir.parent().context("Failed to get package root")?;
            package_root.join("meta.json")
        };

        let existing_meta = if self.runtime.exists(&meta_path) {
            Meta::load(&self.runtime, &meta_path).ok()
        } else {
            None
        };

        let new_meta = self
            .fetch_meta(
                repo,
                current_version,
                existing_meta.as_ref().map(|m| m.api_url.as_str()),
            )
            .await?;

        let mut final_meta = if self.runtime.exists(&meta_path) {
            let mut existing = Meta::load(&self.runtime, &meta_path)?;
            if existing.merge(new_meta.clone()) {
                existing.updated_at = new_meta.updated_at;
            }
            existing
        } else {
            new_meta
        };

        // Ensure current_version is always correct (e.g. if we just installed a new version)
        final_meta.current_version = current_version.to_string();

        self.save_meta(&meta_path, &final_meta)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::MockExtractor;
    use crate::github::{MockGetReleases, RepoInfo};
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
    // Tests for get_target_dir and update_current_symlink are now in paths.rs and symlink.rs

    #[cfg(not(windows))]
    #[tokio::test]
    async fn test_install_happy_path() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // Meta check
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/home/user/.ghri/o/r/meta.json")))
            .returning(|_| false);

        // Save meta interactions
        runtime
            .expect_create_dir_all()
            .with(eq(PathBuf::from("/home/user/.ghri/o/r")))
            .returning(|_| Ok(()));
        runtime
            .expect_write()
            .with(
                eq(PathBuf::from("/home/user/.ghri/o/r/meta.json.tmp")),
                always(),
            )
            .returning(|_, _| Ok(()));
        runtime
            .expect_rename()
            .with(
                eq(PathBuf::from("/home/user/.ghri/o/r/meta.json.tmp")),
                eq(PathBuf::from("/home/user/.ghri/o/r/meta.json")),
            )
            .returning(|_, _| Ok(()));

        // Ensure installed interactions
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/home/user/.ghri/o/r/v1")))
            .returning(|_| false);
        runtime
            .expect_create_dir_all()
            .with(eq(PathBuf::from("/home/user/.ghri/o/r/v1")))
            .returning(|_| Ok(()));
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));
        runtime.expect_remove_file().returning(|_| Ok(()));

        // Symlink interactions
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/home/user/.ghri/o/r/current")))
            .returning(|_| false);
        runtime.expect_symlink().returning(|_, _| Ok(()));

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api.github.com".to_string());
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "now".into(),
            })
        });

        let download_url = format!("{}/tarball", url);
        github.expect_get_releases_at().return_once(move |_, _| {
            Ok(vec![Release {
                tag_name: "v1".into(),
                tarball_url: download_url,
                ..Default::default()
            }])
        });

        let _m = server
            .mock("GET", "/tarball")
            .with_status(200)
            .with_body("data")
            .create();

        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        let installer = Installer::new(runtime, github, Client::new(), extractor);
        installer.install(&repo, None, None).await.unwrap();
    }

    #[tokio::test]
    async fn test_get_or_fetch_meta_invalid_on_disk() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        #[cfg(not(windows))]
        let meta_path = PathBuf::from("/home/user/.ghri/o/r/meta.json");
        #[cfg(windows)]
        let meta_path = PathBuf::from("C:\\Users\\user\\.ghri\\o\\r\\meta.json");
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(|_| Ok("invalid json".into()));

        // Should fallback to fetch and then save
        runtime.expect_create_dir_all().returning(|_| Ok(()));
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api".to_string());
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "now".into(),
            })
        });
        github.expect_get_releases_at().returning(|_, _| Ok(vec![]));

        let installer = Installer::new(runtime, github, Client::new(), MockExtractor::new());
        let (meta, _) = installer.get_or_fetch_meta(&repo, None).await.unwrap();
        assert_eq!(meta.name, "o/r");
    }

    // test_find_all_packages is now in package/discovery.rs

    #[tokio::test]
    async fn test_update_all_happy_path() {
        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");
        configure_runtime_basics(&mut runtime);

        // Find one package
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|p| Ok(vec![p.join("o")]));
        runtime.expect_is_dir().returning(|_| true); // owner/repo are dirs
        runtime
            .expect_read_dir()
            .with(eq(root.join("o")))
            .returning(|p| Ok(vec![p.join("r")]));
        runtime
            .expect_exists()
            .with(eq(root.join("o/r/meta.json")))
            .returning(|_| true);

        // Load meta
        let meta = Meta {
            name: "o/r".into(),
            current_version: "v1".into(),
            updated_at: "old".into(),
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

        // Update check calls fetch_meta -> github
        let mut github = MockGetReleases::new();
        github.expect_api_url().return_const("api".to_string());
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                updated_at: "new".into(),
                ..RepoInfo {
                    description: None,
                    homepage: None,
                    license: None,
                    updated_at: "".into(),
                }
            })
        });
        // Return a new version v2
        github.expect_get_releases_at().returning(|_, _| {
            Ok(vec![
                Release {
                    tag_name: "v2".into(),
                    published_at: Some("2024".into()),
                    ..Default::default()
                },
                Release {
                    tag_name: "v1".into(),
                    published_at: Some("2023".into()),
                    ..Default::default()
                },
            ])
        });

        // save_meta called twice (one for update metadata)
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let installer = Installer::new(runtime, github, Client::new(), MockExtractor::new());
        installer.update_all(None).await.unwrap();
    }

    // Meta tests are now in package/meta.rs
    // find_all_packages tests are now in package/discovery.rs

    #[tokio::test]
    async fn test_update_all_no_packages() {
        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");
        configure_runtime_basics(&mut runtime);

        // update_all calls default_install_root (mocked by basics) -> /home/user/.ghri
        // then find_all_packages checks exists and read_dir
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|_| Ok(vec![]));

        let github = MockGetReleases::new();
        let extractor = MockExtractor::new();
        let installer = Installer::new(runtime, github, Client::new(), extractor);

        let result = installer.update_all(None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_install_no_stable_release() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // Meta fetch
        runtime.expect_exists().returning(|_| false);
        runtime.expect_create_dir_all().returning(|_| Ok(()));
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api.github.com".to_string());
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "now".into(),
            })
        });
        github.expect_get_releases_at().returning(|_, _| {
            Ok(vec![Release {
                tag_name: "v1-rc".into(),
                prerelease: true,
                ..Default::default()
            }])
        });

        let installer = Installer::new(runtime, github, Client::new(), MockExtractor::new());
        let result = installer.install(&repo, None, None).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No stable release found")
        );
    }

    // default_install_root test is now in paths.rs
    
    // Meta tests (test_meta_releases_sorting, test_meta_merge_sorting, test_meta_sorting_fallback,
    // test_meta_conversions, test_meta_get_latest_stable_release*) are now in package/meta.rs
    // Symlink tests are now in symlink.rs

    #[tokio::test]
    async fn test_get_or_fetch_meta_fetch_fail() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);
        runtime.expect_exists().returning(|_| false);

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api".to_string());
        github
            .expect_get_repo_info_at()
            .returning(|_, _| Err(anyhow::anyhow!("fail")));

        let installer = Installer::new(runtime, github, Client::new(), MockExtractor::new());
        let result = installer.get_or_fetch_meta(&repo, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_invalid_repo_str() {
        let config = Config {
            runtime: MockRuntime::new(),
            github: MockGetReleases::new(),
            client: Client::new(),
            extractor: MockExtractor::new(),
            install_root: None,
        };
        let result = run("invalid", config).await;
        assert!(result.is_err());
    }

    // More Meta tests that are now in package/meta.rs

    // Symlink tests are now in symlink.rs

    #[tokio::test]
    async fn test_save_metadata_failure_warning() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // Using Sequence for write
        let mut seq = mockall::Sequence::new();

        runtime.expect_exists().returning(|_| false);
        runtime.expect_create_dir_all().returning(|_| Ok(()));

        runtime
            .expect_write()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(())); // Fetch save
        runtime
            .expect_write()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Err(anyhow::anyhow!("fail"))); // Update save

        runtime.expect_rename().returning(|_, _| Ok(()));

        // Install steps
        runtime
            .expect_create_file()
            .returning(|_| Ok(Box::new(std::io::sink())));
        runtime.expect_remove_file().returning(|_| Ok(()));
        runtime.expect_symlink().returning(|_, _| Ok(()));

        let mut github = MockGetReleases::new();
        github
            .expect_api_url()
            .return_const("https://api".to_string());
        github.expect_get_repo_info_at().returning(|_, _| {
            Ok(RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "".into(),
            })
        });

        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let tar_url = format!("{}/tar", url);
        let _m = server.mock("GET", "/tar").with_status(200).create();

        github.expect_get_releases_at().return_once(move |_, _| {
            Ok(vec![Release {
                tag_name: "v1".into(),
                tarball_url: tar_url,
                ..Default::default()
            }])
        });

        let mut extractor = MockExtractor::new();
        extractor
            .expect_extract_with_cleanup()
            .returning(|_: &MockRuntime, _, _, _| Ok(()));

        let installer = Installer::new(runtime, github, Client::new(), extractor);
        let result = installer.install(&repo, None, None).await;
        assert!(result.is_ok());
    }

    // default_install_root_privileged_mock test is now in paths.rs
    
    #[tokio::test]
    async fn test_get_or_fetch_meta_exists_interaction() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);
        runtime.expect_exists().returning(|_| true);
        runtime.expect_read_to_string().returning(|_| Ok(r#"{"name":"o/r","api_url":"https://api.github.com","repo_info_url":"","releases_url":"","description":null,"homepage":null,"license":null,"updated_at":"","current_version":"v1","releases":[]}"#.into()));

        let installer = Installer::new(
            runtime,
            MockGetReleases::new(),
            Client::new(),
            MockExtractor::new(),
        );
        let (meta, _) = installer.get_or_fetch_meta(&repo, None).await.unwrap();
        assert_eq!(meta.name, "o/r");
    }

    #[tokio::test]
    async fn test_install_uses_existing_meta() {
        // Similar to exists check but inside install flow
        let mut runtime = MockRuntime::new();
        let github = MockGetReleases::new();
        configure_runtime_basics(&mut runtime);

        // Exists
        runtime.expect_exists().returning(|_| true);
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
        runtime.expect_is_dir().returning(|_| false); // meta.json is not dir

        // Logic should skip fetch
        // and proceed to ensure_installed (mocked by basics)
        runtime.expect_exists().returning(|_| true); // target dir exists
        runtime.expect_is_symlink().returning(|_| true);
        runtime
            .expect_read_link()
            .returning(|_| Ok(PathBuf::from("v1")));
        runtime.expect_symlink().returning(|_, _| Ok(()));
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let installer = Installer::new(runtime, github, Client::new(), MockExtractor::new());
        installer
            .install(
                &GitHubRepo {
                    owner: "o".into(),
                    repo: "r".into(),
                },
                None,
                None,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);
        runtime.expect_exists().returning(|_| true); // cached
        runtime.expect_read_to_string().returning(|_| Ok(r#"{"name":"o/r","api_url":"","repo_info_url":"","releases_url":"","description":null,"homepage":null,"license":null,"updated_at":"","current_version":"v1","releases":[{"version":"v1","is_prerelease":false,"tarball_url":"","assets":[]}]}"#.into()));
        runtime.expect_is_dir().returning(|_| false);
        runtime.expect_is_symlink().returning(|_| true);
        runtime
            .expect_read_link()
            .returning(|_| Ok(PathBuf::from("v1")));
        runtime.expect_symlink().returning(|_, _| Ok(()));
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        let config = Config {
            runtime,
            github: MockGetReleases::new(),
            client: Client::new(),
            extractor: MockExtractor::new(),
            install_root: None,
        };
        run("o/r", config).await.unwrap();
    }

    // test_update_current_symlink_no_op_if_already_correct is now in symlink.rs

    #[tokio::test]
    async fn test_update_atomic_safety() {
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // Verify write -> rename sequence for metadata
        let mut seq = mockall::Sequence::new();
        runtime
            .expect_write()
            .withf(|p, _| p.to_string_lossy().ends_with(".tmp"))
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        runtime
            .expect_rename()
            .withf(|f, t| {
                f.to_string_lossy().ends_with(".tmp") && t.to_string_lossy().ends_with(".json")
            })
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));

        let meta = Meta {
            name: "o/r".into(),
            ..Default::default()
        };
        Installer::new(
            runtime,
            MockGetReleases::new(),
            Client::new(),
            MockExtractor::new(),
        )
        .save_meta(&PathBuf::from("meta.json"), &meta)
        .unwrap();
    }

    // test_update_timestamp_behavior is now in package/meta.rs
}
