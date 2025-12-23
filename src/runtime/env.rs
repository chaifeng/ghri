//! Environment and system information operations.

use std::env;
use std::path::PathBuf;

use super::RealRuntime;

impl RealRuntime {
    #[tracing::instrument(skip(self))]
    pub(crate) fn env_var_impl(&self, key: &str) -> Result<String, env::VarError> {
        env::var(key)
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn home_dir_impl(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn config_dir_impl(&self) -> Option<PathBuf> {
        dirs::config_dir()
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn temp_dir_impl(&self) -> PathBuf {
        env::temp_dir()
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn is_privileged_impl(&self) -> bool {
        #[cfg(unix)]
        return nix::unistd::geteuid().as_raw() == 0;

        #[cfg(windows)]
        return is_elevated::is_elevated();
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::{RealRuntime, Runtime};

    #[test]
    fn test_real_runtime_env_and_dirs() {
        let runtime = RealRuntime;

        // Test env_var - PATH should exist on all systems
        assert!(runtime.env_var("PATH").is_ok());

        // Test home_dir - should exist for most systems
        let home = runtime.home_dir();
        assert!(home.is_some() || cfg!(target_os = "linux")); // CI might not have home

        // Test temp_dir - should always return a valid path
        let temp = runtime.temp_dir();
        assert!(temp.is_absolute() || cfg!(windows));

        // Test is_privileged - should work without panic
        let _ = runtime.is_privileged();
    }
}
