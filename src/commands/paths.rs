use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::runtime::Runtime;

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
