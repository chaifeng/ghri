use anyhow::Result;
use log::warn;

use crate::archive::ArchiveExtractor;
use crate::download::Downloader;
use crate::package::{Meta, PackageRepository, VersionResolver};
use crate::runtime::Runtime;
use crate::source::{RepoId, Source};

use super::config::{Config, ConfigOverrides};
use super::install::Installer;
use super::services::Services;

#[tracing::instrument(skip(runtime, overrides, repos))]
pub async fn update<R: Runtime + 'static>(
    runtime: R,
    overrides: ConfigOverrides,
    repos: Vec<String>,
) -> Result<()> {
    let config = Config::load(&runtime, overrides)?;
    let services = Services::from_config(&config)?;
    run_update(&config, runtime, services, repos).await
}

#[tracing::instrument(skip(config, runtime, services, repos))]
async fn run_update<R: Runtime + 'static, S: Source, E: ArchiveExtractor, D: Downloader>(
    config: &Config,
    runtime: R,
    services: Services<S, D, E>,
    repos: Vec<String>,
) -> Result<()> {
    let installer = Installer::new(
        runtime,
        services.source,
        services.downloader,
        services.extractor,
    );

    let pkg_repo = PackageRepository::new(&installer.runtime, config.install_root.clone());
    let packages = pkg_repo.find_all_with_meta()?;

    if packages.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    // Parse requested repos for filtering
    let filter_repos: Vec<RepoId> = repos
        .iter()
        .filter_map(|r| r.parse::<RepoId>().ok())
        .collect();

    for (_meta_path, meta) in packages {
        let repo = match meta.name.parse::<RepoId>() {
            Ok(r) => r,
            Err(e) => {
                warn!("Invalid repo name in meta: {}", e);
                continue;
            }
        };

        // Skip if not in filter list (when filter is specified)
        if !filter_repos.is_empty() && !filter_repos.contains(&repo) {
            continue;
        }

        println!("   updating {}", repo);
        if let Err(e) =
            save_metadata(config, &pkg_repo, &installer, &repo, &meta.current_version).await
        {
            warn!("Failed to update metadata for {}: {}", repo, e);
        } else {
            // Check if update is available using VersionResolver
            let updated_meta = pkg_repo.load_required(&repo.owner, &repo.repo)?;
            if let Some(latest) = VersionResolver::check_update(
                &updated_meta.releases,
                &meta.current_version,
                false, // don't include prereleases
            ) {
                print_update_available(&repo, &meta.current_version, &latest.version);
            }
        }
    }

    Ok(())
}

#[tracing::instrument(skip(repo, current, latest))]
fn print_update_available(repo: &RepoId, current: &str, latest: &str) {
    let current_display = if current.is_empty() {
        "(none)"
    } else {
        current
    };
    println!("  updatable {} {} -> {}", repo, current_display, latest);
}

#[tracing::instrument(skip(config, pkg_repo, installer, repo, current_version))]
async fn save_metadata<R: Runtime + 'static, S: Source, E: ArchiveExtractor, D: Downloader>(
    config: &Config,
    pkg_repo: &PackageRepository<'_, R>,
    installer: &Installer<R, S, E, D>,
    repo: &RepoId,
    current_version: &str,
) -> Result<()> {
    let existing_meta = pkg_repo.load(&repo.owner, &repo.repo)?;

    let new_meta = fetch_meta(
        config,
        installer,
        repo,
        current_version,
        existing_meta.as_ref().map(|m| m.api_url.as_str()),
    )
    .await?;

    let mut final_meta = if let Some(mut existing) = existing_meta {
        if existing.merge(new_meta.clone()) {
            existing.updated_at = new_meta.updated_at;
        }
        existing
    } else {
        new_meta
    };

    // Ensure current_version is always correct (e.g. if we just installed a new version)
    final_meta.current_version = current_version.to_string();

    pkg_repo.save(&repo.owner, &repo.repo, &final_meta)?;

    Ok(())
}

#[tracing::instrument(skip(config, installer, repo, current_version, api_url))]
async fn fetch_meta<R: Runtime + 'static, S: Source, E: ArchiveExtractor, D: Downloader>(
    config: &Config,
    installer: &Installer<R, S, E, D>,
    repo: &RepoId,
    current_version: &str,
    api_url: Option<&str>,
) -> Result<Meta> {
    let api_url = api_url.unwrap_or(&config.api_url);
    let repo_info = installer.source.get_repo_metadata_at(repo, api_url).await?;
    let releases = installer.source.get_releases_at(repo, api_url).await?;
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
    use crate::archive::MockArchiveExtractor;
    use crate::download::mock::MockDownloader;
    use crate::runtime::MockRuntime;
    use crate::source::{MockSource, RepoMetadata, SourceRelease};
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

    #[tokio::test]
    async fn test_update_function() {
        // Test that update() function works with empty install root

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup ---

        // Install root doesn't exist -> no packages to update
        runtime.expect_exists().returning(|_| false);

        // --- Execute ---

        update(runtime, ConfigOverrides::default(), vec![])
            .await
            .unwrap();
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

        // --- 3. Fetch New Metadata from Source ---

        let mut source = MockSource::new();
        source.expect_api_url().return_const("api".to_string());

        // Get repo info
        source.expect_get_repo_metadata_at().returning(|_, _| {
            Ok(RepoMetadata {
                updated_at: Some("new".into()),
                description: None,
                homepage: None,
                license: None,
            })
        });

        // Get releases -> v2 is available (newer than installed v1)
        source.expect_get_releases_at().returning(|_, _| {
            Ok(vec![
                SourceRelease {
                    tag: "v2".into(), // New version!
                    published_at: Some("2024".into()),
                    ..Default::default()
                },
                SourceRelease {
                    tag: "v1".into(), // Currently installed
                    published_at: Some("2023".into()),
                    ..Default::default()
                },
            ])
        });

        // --- 4. Save Updated Metadata ---

        // Check if package directory exists (for PackageRepository::save)
        runtime
            .expect_exists()
            .with(eq(root.join("o/r")))
            .returning(|_| true);

        // Write and rename meta.json
        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---

        let config = test_config();
        let services = Services {
            source,
            downloader: MockDownloader::new(),
            extractor: MockArchiveExtractor::new(),
        };
        run_update(&config, runtime, services, vec![])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_update_no_packages() {
        // Test that update shows "No packages installed" when directory is empty

        let mut runtime = MockRuntime::new();
        #[cfg(not(windows))]
        let root = PathBuf::from("/home/user/.ghri");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\Users\\user\\.ghri");

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

        let config = test_config();
        let services = Services {
            source: MockSource::new(),
            downloader: MockDownloader::new(),
            extractor: MockArchiveExtractor::new(),
        };
        let result = run_update(&config, runtime, services, vec![]).await;
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
                SourceRelease {
                    tag: "v1".into(),
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
                SourceRelease {
                    tag: "v1".into(),
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

        // --- 3. Fetch New Metadata from Source (only for owner1/repo1) ---

        let mut source = MockSource::new();

        // Only owner1/repo1 should be updated, so expect exactly one call
        source
            .expect_get_repo_metadata_at()
            .times(1)
            .returning(|_, _| {
                Ok(RepoMetadata {
                    updated_at: Some("new".into()),
                    description: None,
                    homepage: None,
                    license: None,
                })
            });

        source.expect_get_releases_at().times(1).returning(|_, _| {
            Ok(vec![SourceRelease {
                tag: "v1".into(),
                ..Default::default()
            }])
        });

        // --- 4. Save Updated Metadata ---

        // Check if package directory exists (for PackageRepository::save)
        runtime
            .expect_exists()
            .with(eq(root.join("owner1/repo1")))
            .returning(|_| true);

        runtime.expect_write().returning(|_, _| Ok(()));
        runtime.expect_rename().returning(|_, _| Ok(()));

        // --- Execute ---

        let config = test_config();
        let services = Services {
            source,
            downloader: MockDownloader::new(),
            extractor: MockArchiveExtractor::new(),
        };
        // Only update owner1/repo1, skip owner2/repo2
        let result = run_update(&config, runtime, services, vec!["owner1/repo1".to_string()]).await;
        assert!(result.is_ok());
    }
}
