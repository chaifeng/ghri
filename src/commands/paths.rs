use anyhow::{Context, Result};
use log::info;
use std::path::PathBuf;

use crate::github::{GitHubRepo, Release};
use crate::runtime::Runtime;

/// Get the target installation directory for a package version
#[tracing::instrument(skip(runtime, repo, release, install_root))]
pub fn get_target_dir<R: Runtime>(
    runtime: &R,
    repo: &GitHubRepo,
    release: &Release,
    install_root: Option<PathBuf>,
) -> Result<PathBuf> {
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(runtime)?,
    };

    info!("Using install root: {}", root.display());

    Ok(root
        .join(&repo.owner)
        .join(&repo.repo)
        .join(&release.tag_name))
}

/// Get the default installation root directory
#[tracing::instrument(skip(runtime))]
pub fn default_install_root<R: Runtime>(runtime: &R) -> Result<PathBuf> {
    if runtime.is_privileged() {
        Ok(system_install_root(runtime))
    } else {
        let home_dir = runtime
            .home_dir()
            .context("Could not find home directory")?;
        Ok(home_dir.join(".ghri"))
    }
}

#[cfg(target_os = "macos")]
#[tracing::instrument(skip(_runtime))]
fn system_install_root<R: Runtime>(_runtime: &R) -> PathBuf {
    PathBuf::from("/opt/ghri")
}

#[cfg(target_os = "windows")]
#[tracing::instrument(skip(_runtime))]
fn system_install_root<R: Runtime>(_runtime: &R) -> PathBuf {
    PathBuf::from(r"C:\ProgramData\ghri")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[tracing::instrument(skip(_runtime))]
fn system_install_root<R: Runtime>(_runtime: &R) -> PathBuf {
    PathBuf::from("/usr/local/ghri")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;

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

    #[test]
    fn test_get_target_dir() {
        // Test that get_target_dir returns correct path: {install_root}/{owner}/{repo}/{version}

        let mut runtime = MockRuntime::new();
        configure_runtime_basics(&mut runtime);

        // --- Setup ---
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag_name: "v1".into(),
            ..Default::default()
        };

        // --- Execute ---
        let target_dir = get_target_dir(&runtime, &repo, &release, None).unwrap();

        // --- Verify ---
        // With default install root (~/.ghri), path should be: ~/.ghri/o/r/v1
        #[cfg(not(windows))]
        assert_eq!(target_dir, PathBuf::from("/home/user/.ghri/o/r/v1"));
        #[cfg(windows)]
        assert_eq!(
            target_dir,
            PathBuf::from("C:\\Users\\user\\.ghri\\o\\r\\v1")
        );
    }

    #[test]
    fn test_get_target_dir_with_custom_root() {
        // Test that get_target_dir uses custom install root when provided

        // --- Setup ---
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let release = Release {
            tag_name: "v1".into(),
            ..Default::default()
        };
        let runtime = MockRuntime::new(); // No expectations needed - custom root bypasses defaults

        // --- Execute ---
        let target_dir =
            get_target_dir(&runtime, &repo, &release, Some(PathBuf::from("/custom"))).unwrap();

        // --- Verify ---
        // With custom install root /custom, path should be: /custom/o/r/v1
        assert_eq!(target_dir, PathBuf::from("/custom/o/r/v1"));
    }

    #[test]
    fn test_default_install_root_no_home() {
        // Test that default_install_root fails when home directory is not available

        let mut runtime = MockRuntime::new();

        // --- Setup ---

        // Not privileged user
        runtime.expect_is_privileged().returning(|| false);

        // Home directory not available -> None
        runtime.expect_home_dir().returning(|| None);

        // --- Execute & Verify ---

        // Should fail because home directory is required for non-privileged user
        let result = default_install_root(&runtime);
        assert!(result.is_err());
    }

    #[test]
    fn test_default_install_root_privileged() {
        // Test that privileged user gets system install root instead of home directory

        let mut runtime = MockRuntime::new();

        // --- Setup ---

        // Privileged user (e.g., root)
        runtime.expect_is_privileged().returning(|| true);

        // --- Execute ---

        let root = default_install_root(&runtime).unwrap();

        // --- Verify ---

        // Privileged users get system-wide install directory
        #[cfg(target_os = "macos")]
        assert_eq!(root, PathBuf::from("/opt/ghri"));
        #[cfg(all(unix, not(target_os = "macos")))]
        assert_eq!(root, PathBuf::from("/usr/local/ghri"));
        #[cfg(target_os = "windows")]
        assert_eq!(root, PathBuf::from("C:\\ProgramData\\ghri"));
    }
}
