use anyhow::Result;
use log::warn;

use crate::package::{Meta, PackageRepository, VersionResolver};
use crate::runtime::Runtime;
use crate::source::RepoId;

use super::config::{Config, ConfigOverrides};
use super::services::RegistryServices;

#[tracing::instrument(skip(runtime, overrides, repos))]
pub async fn update<R: Runtime + 'static>(
    runtime: R,
    overrides: ConfigOverrides,
    repos: Vec<String>,
) -> Result<()> {
    let config = Config::load(&runtime, overrides)?;
    let services = RegistryServices::from_config(&config)?;
    run_update(&config, runtime, services, repos).await
}

#[tracing::instrument(skip(config, runtime, services, repos))]
async fn run_update<R: Runtime + 'static>(
    config: &Config,
    runtime: R,
    services: RegistryServices,
    repos: Vec<String>,
) -> Result<()> {
    let pkg_repo = PackageRepository::new(&runtime, config.install_root.clone());
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

        // Resolve source from package metadata (auto-detect GitHub/GitLab/Gitee)
        let source = match services.registry.resolve_from_meta(&meta) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to resolve source for {}: {}", repo, e);
                continue;
            }
        };

        if let Err(e) = save_metadata(&pkg_repo, source.as_ref(), &repo, &meta).await {
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

#[tracing::instrument(skip(pkg_repo, source, repo, existing_meta))]
async fn save_metadata<R: Runtime>(
    pkg_repo: &PackageRepository<'_, R>,
    source: &dyn crate::source::Source,
    repo: &RepoId,
    existing_meta: &Meta,
) -> Result<()> {
    let api_url = &existing_meta.api_url;
    let current_version = &existing_meta.current_version;

    // Fetch new metadata from source
    let repo_info = source.get_repo_metadata_at(repo, api_url).await?;
    let releases = source.get_releases_at(repo, api_url).await?;
    let new_meta = Meta::from(repo.clone(), repo_info, releases, current_version, api_url);

    // Merge with existing metadata
    let mut final_meta = existing_meta.clone();
    if final_meta.merge(new_meta.clone()) {
        final_meta.updated_at = new_meta.updated_at;
    }

    pkg_repo.save(&repo.owner, &repo.repo, &final_meta)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
