use anyhow::Result;
use log::warn;

use crate::application::InstallAction;
use crate::package::VersionResolver;
use crate::provider::RepoId;
use crate::runtime::Runtime;

use super::config::{Config, ConfigOverrides};
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
async fn run_update<R: Runtime + 'static>(
    config: &Config,
    runtime: R,
    services: Services,
    repos: Vec<String>,
) -> Result<()> {
    // Create InstallAction for metadata operations
    let action = InstallAction::new(
        &runtime,
        &services.provider_factory,
        config.install_root.clone(),
    );
    let pkg_repo = action.package_repo();

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

        // Resolve source from package metadata
        let source = action.resolve_source_from_meta(&meta);

        // Fetch new metadata using InstallAction with saved API URL
        let new_meta = match action
            .fetch_meta_at(&repo, source.as_ref(), &meta.api_url, &meta.current_version)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to fetch metadata for {}: {}", repo, e);
                continue;
            }
        };

        // Merge with existing metadata
        let mut final_meta = meta.clone();
        if final_meta.merge(new_meta.clone()) {
            final_meta.updated_at = new_meta.updated_at.clone();
        }

        // Save updated metadata
        if let Err(e) = action.save_meta(&repo, &final_meta) {
            warn!("Failed to save metadata for {}: {}", repo, e);
            continue;
        }

        // Check if update is available
        if let Some(latest) = VersionResolver::check_update(
            &final_meta.releases,
            &meta.current_version,
            false, // don't include prereleases
        ) {
            print_update_available(&repo, &meta.current_version, &latest.version);
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
