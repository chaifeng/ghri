use anyhow::Result;

use crate::application::UpdateAction;
use crate::runtime::Runtime;

use super::config::Config;
use super::services::Services;

#[tracing::instrument(skip(runtime, config, repos))]
pub async fn update<R: Runtime + 'static>(
    runtime: R,
    config: Config,
    repos: Vec<String>,
) -> Result<()> {
    let services = Services::from_config(&config)?;

    let action = UpdateAction::new(
        &runtime,
        &services.provider_factory,
        config.install_root.clone(),
    );

    let results = action.update_all(&repos).await?;

    if results.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    for result in &results {
        println!("   updating {}", result.repo);
        if result.has_update && result.latest_version.is_some() {
            let latest = result.latest_version.as_ref().unwrap();
            print_update_available(&result.repo.to_string(), &result.current_version, latest);
        }
    }

    Ok(())
}

#[tracing::instrument(skip(repo, current, latest))]
fn print_update_available(repo: &str, current: &str, latest: &str) {
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
    use crate::test_utils::{configure_mock_runtime_basics, test_root};

    // Helper to configure simple home dir and user
    fn configure_runtime_basics(runtime: &mut MockRuntime) {
        configure_mock_runtime_basics(runtime);
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

        update(runtime, Config::for_test(test_root()), vec![])
            .await
            .unwrap();
    }
}
