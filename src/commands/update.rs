use anyhow::{Context, Result};
use log::warn;
use std::path::{Path, PathBuf};

use crate::archive::Extractor;
use crate::github::{GetReleases, GitHubRepo};
use crate::package::{Meta, find_all_packages};
use crate::runtime::Runtime;

use super::config::Config;
use super::install::Installer;
use super::paths::default_install_root;

#[tracing::instrument(skip(runtime, install_root, api_url))]
pub async fn update<R: Runtime + 'static>(
    runtime: R,
    install_root: Option<PathBuf>,
    api_url: Option<String>,
) -> Result<()> {
    let config = Config::new(runtime, install_root, api_url)?;
    run_update(config).await
}

#[tracing::instrument(skip(config))]
async fn run_update<R: Runtime + 'static, G: GetReleases, E: Extractor>(
    config: Config<R, G, E>,
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

    let installer = Installer::new(
        config.runtime,
        config.github,
        config.client,
        config.extractor,
    );

    for meta_path in meta_files {
        let meta = Meta::load(&installer.runtime, &meta_path)?;
        let repo = meta.name.parse::<GitHubRepo>()?;

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
async fn save_metadata<R: Runtime + 'static, G: GetReleases, E: Extractor>(
    installer: &Installer<R, G, E>,
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
async fn fetch_meta<R: Runtime + 'static, G: GetReleases, E: Extractor>(
    installer: &Installer<R, G, E>,
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
    use crate::github::{MockGetReleases, Release, RepoInfo};
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
        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);
        runtime.expect_exists().returning(|_| false); // root empty

        update(runtime, None, None).await.unwrap();
    }

    #[tokio::test]
    async fn test_update_happy_path() {
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

        let config = Config {
            runtime,
            github,
            client: Client::new(),
            extractor: MockExtractor::new(),
            install_root: None,
        };
        run_update(config).await.unwrap();
    }

    #[tokio::test]
    async fn test_update_no_packages() {
        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");
        configure_runtime_basics(&mut runtime);

        // update calls default_install_root (mocked by basics) -> /home/user/.ghri
        // then find_all_packages checks exists and read_dir
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(|_| Ok(vec![]));

        let config = Config {
            runtime,
            github: MockGetReleases::new(),
            client: Client::new(),
            extractor: MockExtractor::new(),
            install_root: None,
        };
        let result = run_update(config).await;
        assert!(result.is_ok());
    }
}
