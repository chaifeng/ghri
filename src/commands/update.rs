use anyhow::{Context, Result};
use log::warn;
use std::path::{Path, PathBuf};

use crate::archive::Extractor;
use crate::download::Downloader;
use crate::github::{GetReleases, GitHubRepo};
use crate::package::{Meta, find_all_packages};
use crate::runtime::Runtime;

use super::config::Config;
use super::install::Installer;
use super::paths::default_install_root;

#[tracing::instrument(skip(runtime, install_root, api_url, repos))]
pub async fn update<R: Runtime + 'static>(
    runtime: R,
    install_root: Option<PathBuf>,
    api_url: Option<String>,
    repos: Vec<String>,
) -> Result<()> {
    let config = Config::new(runtime, install_root, api_url)?;
    run_update(config, repos).await
}

#[tracing::instrument(skip(config, repos))]
async fn run_update<R: Runtime + 'static, G: GetReleases, E: Extractor, D: Downloader>(
    config: Config<R, G, E, D>,
    repos: Vec<String>,
) -> Result<()> {
    let root = match config.install_root {
        Some(path) => path,
        None => default_install_root(&config.runtime)?,
    };

    let meta_files = find_all_packages(&config.runtime, &root)?;
    if meta_files.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    // Parse requested repos for filtering
    let filter_repos: Vec<GitHubRepo> = repos
        .iter()
        .filter_map(|r| r.parse::<GitHubRepo>().ok())
        .collect();

    let installer = Installer::new(
        config.runtime,
        config.github,
        config.downloader,
        config.extractor,
    );

    for meta_path in meta_files {
        let meta = Meta::load(&installer.runtime, &meta_path)?;
        let repo = meta.name.parse::<GitHubRepo>()?;

        // Skip if not in filter list (when filter is specified)
        if !filter_repos.is_empty() && !filter_repos.contains(&repo) {
            continue;
        }

        println!("   updating {}", repo);
        if let Err(e) = save_metadata(&installer, &repo, &meta.current_version, &meta_path).await {
            warn!("Failed to update metadata for {}: {}", repo, e);
        } else {
            // Check if update is available
            let updated_meta = Meta::load(&installer.runtime, &meta_path)?;
            if let Some(latest) = updated_meta.get_latest_stable_release()
                && meta.current_version != latest.version
            {
                print_update_available(&repo, &meta.current_version, &latest.version);
            }
        }
    }

    Ok(())
}

#[tracing::instrument(skip(repo, current, latest))]
fn print_update_available(repo: &GitHubRepo, current: &str, latest: &str) {
    let current_display = if current.is_empty() {
        "(none)"
    } else {
        current
    };
    println!("  updatable {} {} -> {}", repo, current_display, latest);
}

#[tracing::instrument(skip(installer, repo, current_version, target_dir))]
async fn save_metadata<R: Runtime + 'static, G: GetReleases, E: Extractor, D: Downloader>(
    installer: &Installer<R, G, E, D>,
    repo: &GitHubRepo,
    current_version: &str,
    target_dir: &Path,
) -> Result<()> {
    let meta_path = if !installer.runtime.is_dir(target_dir) {
        target_dir.to_path_buf()
    } else {
        let package_root = target_dir.parent().context("Failed to get package root")?;
        package_root.join("meta.json")
    };

    let existing_meta = if installer.runtime.exists(&meta_path) {
        Meta::load(&installer.runtime, &meta_path).ok()
    } else {
        None
    };

    let new_meta = fetch_meta(
        installer,
        repo,
        current_version,
        existing_meta.as_ref().map(|m| m.api_url.as_str()),
    )
    .await?;

    let mut final_meta = if installer.runtime.exists(&meta_path) {
        let mut existing = Meta::load(&installer.runtime, &meta_path)?;
        if existing.merge(new_meta.clone()) {
            existing.updated_at = new_meta.updated_at;
        }
        existing
    } else {
        new_meta
    };

    // Ensure current_version is always correct (e.g. if we just installed a new version)
    final_meta.current_version = current_version.to_string();

    installer.save_meta(&meta_path, &final_meta)?;

    Ok(())
}

#[tracing::instrument(skip(installer, repo, current_version, api_url))]
async fn fetch_meta<R: Runtime + 'static, G: GetReleases, E: Extractor, D: Downloader>(
    installer: &Installer<R, G, E, D>,
    repo: &GitHubRepo,
    current_version: &str,
    api_url: Option<&str>,
) -> Result<Meta> {
    let api_url = api_url.unwrap_or(installer.github.api_url());
    let repo_info = installer.github.get_repo_info_at(repo, api_url).await?;
    let releases = installer.github.get_releases_at(repo, api_url).await?;
    Ok(Meta::from(
        repo.clone(),
        repo_info,
        releases,
        current_version,
        api_url,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::MockExtractor;
    use crate::download::HttpDownloader;
    use crate::github::{MockGetReleases, Release, RepoInfo};
    use crate::http::HttpClient;
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

    #[tokio::test]
    async fn test_update_function() {
        // Test that update() function works with empty install root

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup ---

        // Install root doesn't exist -> no packages to update
        runtime.expect_exists().returning(|_| false);

        // --- Execute ---

        update(runtime, None, None, vec![]).await.unwrap();
    }

    #[tokio::test]
    async fn test_update_happy_path() {
        // Test updating packages: fetches new metadata and detects available updates

        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");
        configure_runtime_basics(&mut runtime);

        // --- 1. Find Installed Packages ---

        // Directory exists: ~/.ghri -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Read dir ~/.ghri -> [o] (one owner)
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|p| Ok(vec![p.join("o")]));

        // Is dir checks for owner/repo traversal
        runtime.expect_is_dir().returning(|_| true);

        // Read dir ~/.ghri/o -> [r] (one repo)
        runtime
            .expect_read_dir()
            .with(eq(root.join("o")))
            .returning(|p| Ok(vec![p.join("r")]));

        // File exists: ~/.ghri/o/r/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(root.join("o/r/meta.json")))
            .returning(|_| true);

        // --- 2. Load Current Metadata ---

        // Read meta.json -> current version v1
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

        // --- 3. Fetch New Metadata from GitHub ---

        let mut github = MockGetReleases::new();
        github.expect_api_url().return_const("api".to_string());

        // Get repo info
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

        // Get releases -> v2 is available (newer than installed v1)
        github.expect_get_releases_at().returning(|_, _| {
            Ok(vec![
                Release {
                    tag_name: "v2".into(), // New version!
                    published_at: Some("2024".into()),
                    ..Default::default()
                },
                Release {
                    tag_name: "v1".into(), // Currently installed
                    published_at: Some("2023".into()),
                    ..Default::default()
                },
            ])
        });

        // --- 4. Save Updated Metadata ---

        // Write and rename meta.json
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---

        let config = Config {
            runtime,
            github,
            downloader: HttpDownloader::new(HttpClient::new(Client::new())),
            extractor: MockExtractor::new(),
            install_root: None,
        };
        run_update(config, vec![]).await.unwrap();
    }

    #[tokio::test]
    async fn test_update_no_packages() {
        // Test that update shows "No packages installed" when directory is empty

        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");
        configure_runtime_basics(&mut runtime);

        // --- Setup ---

        // Directory exists: ~/.ghri -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Read dir ~/.ghri -> empty (no packages)
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|_| Ok(vec![]));

        // --- Execute ---

        let config = Config {
            runtime,
            github: MockGetReleases::new(),
            downloader: HttpDownloader::new(HttpClient::new(Client::new())),
            extractor: MockExtractor::new(),
            install_root: None,
        };
        let result = run_update(config, vec![]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_update_filter_specific_packages() {
        // Test that update only updates specified packages when filter is provided

        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");
        configure_runtime_basics(&mut runtime);

        // --- 1. Find Installed Packages ---

        // Directory exists: ~/.ghri -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Read dir ~/.ghri -> [owner1, owner2] (two owners)
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|p| Ok(vec![p.join("owner1"), p.join("owner2")]));

        // Is dir checks for owner/repo traversal
        runtime.expect_is_dir().returning(|_| true);

        // Read dir ~/.ghri/owner1 -> [repo1]
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner1")))
            .returning(|p| Ok(vec![p.join("repo1")]));

        // Read dir ~/.ghri/owner2 -> [repo2]
        runtime
            .expect_read_dir()
            .with(eq(root.join("owner2")))
            .returning(|p| Ok(vec![p.join("repo2")]));

        // File exists: meta.json for both packages
        runtime
            .expect_exists()
            .with(eq(root.join("owner1/repo1/meta.json")))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(root.join("owner2/repo2/meta.json")))
            .returning(|_| true);

        // --- 2. Load Current Metadata ---

        // Read meta.json -> return different meta for each package
        let meta1 = Meta {
            name: "owner1/repo1".into(),
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
        let meta2 = Meta {
            name: "owner2/repo2".into(),
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
        let meta1_json = serde_json::to_string(&meta1).unwrap();
        let meta2_json = serde_json::to_string(&meta2).unwrap();

        runtime
            .expect_read_to_string()
            .with(eq(root.join("owner1/repo1/meta.json")))
            .returning(move |_| Ok(meta1_json.clone()));
        runtime
            .expect_read_to_string()
            .with(eq(root.join("owner2/repo2/meta.json")))
            .returning(move |_| Ok(meta2_json.clone()));

        // --- 3. Fetch New Metadata from GitHub (only for owner1/repo1) ---

        let mut github = MockGetReleases::new();
        github.expect_api_url().return_const("api".to_string());

        // Only owner1/repo1 should be updated, so expect exactly one call
        github.expect_get_repo_info_at().times(1).returning(|_, _| {
            Ok(RepoInfo {
                updated_at: "new".into(),
                description: None,
                homepage: None,
                license: None,
            })
        });

        github.expect_get_releases_at().times(1).returning(|_, _| {
            Ok(vec![Release {
                tag_name: "v1".into(),
                ..Default::default()
            }])
        });

        // --- 4. Save Updated Metadata ---

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---

        let config = Config {
            runtime,
            github,
            downloader: HttpDownloader::new(HttpClient::new(Client::new())),
            extractor: MockExtractor::new(),
            install_root: None,
        };
        // Only update owner1/repo1, skip owner2/repo2
        let result = run_update(config, vec!["owner1/repo1".to_string()]).await;
        assert!(result.is_ok());
    }
}
