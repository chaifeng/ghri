pub mod application;
pub mod archive;
pub mod cleanup;
pub mod commands;
pub mod download;
pub mod http;
pub mod package;
pub mod platform;
pub mod provider;
pub mod runtime;

/// Test utilities for cross-platform path handling.
#[cfg(test)]
pub mod test_utils {
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;
    use std::path::PathBuf;

    /// Returns the test root directory path based on the platform.
    /// - Unix: `/home/user/.ghri`
    /// - Windows: `C:\Users\user\.ghri`
    pub fn test_root() -> PathBuf {
        #[cfg(not(windows))]
        {
            PathBuf::from("/home/user/.ghri")
        }
        #[cfg(windows)]
        {
            PathBuf::from(r"C:\Users\user\.ghri")
        }
    }

    /// Returns a test destination path for symlinks based on the platform.
    /// - Unix: `/usr/local/bin`
    /// - Windows: `C:\Program Files\bin`
    pub fn test_bin_dir() -> PathBuf {
        #[cfg(not(windows))]
        {
            PathBuf::from("/usr/local/bin")
        }
        #[cfg(windows)]
        {
            PathBuf::from(r"C:\Program Files\bin")
        }
    }

    /// Returns a test home directory path based on the platform.
    /// - Unix: `/home/user`
    /// - Windows: `C:\Users\user`
    pub fn test_home() -> PathBuf {
        #[cfg(not(windows))]
        {
            PathBuf::from("/home/user")
        }
        #[cfg(windows)]
        {
            PathBuf::from(r"C:\Users\user")
        }
    }

    /// Returns a test alternative destination path based on the platform.
    /// Used for testing external/other paths.
    /// - Unix: `/some/other/path`
    /// - Windows: `C:\some\other\path`
    pub fn test_other_path() -> PathBuf {
        #[cfg(not(windows))]
        {
            PathBuf::from("/some/other/path")
        }
        #[cfg(windows)]
        {
            PathBuf::from(r"C:\some\other\path")
        }
    }

    /// Returns a test opt directory path based on the platform.
    /// - Unix: `/opt/mytools/bin`
    /// - Windows: `C:\opt\mytools\bin`
    pub fn test_opt_bin() -> PathBuf {
        #[cfg(not(windows))]
        {
            PathBuf::from("/opt/mytools/bin")
        }
        #[cfg(windows)]
        {
            PathBuf::from(r"C:\opt\mytools\bin")
        }
    }

    /// Returns a test external package path based on the platform.
    /// Used for testing symlinks to external locations.
    /// - Unix: `/external/package`
    /// - Windows: `C:\external\package`
    pub fn test_external_path() -> PathBuf {
        #[cfg(not(windows))]
        {
            PathBuf::from("/external/package")
        }
        #[cfg(windows)]
        {
            PathBuf::from(r"C:\external\package")
        }
    }

    /// Configure a mock runtime with common defaults for tests.
    /// - home dir set to [`test_home`]
    /// - USER env set to "user"
    /// - GITHUB_TOKEN absent
    /// - not privileged
    /// - canonicalize is a no-op passthrough
    /// - current_dir set to [`test_home`]
    pub fn configure_mock_runtime_basics(runtime: &mut MockRuntime) {
        runtime.expect_home_dir().returning(|| Some(test_home()));

        runtime
            .expect_env_var()
            .with(eq("USER"))
            .returning(|_| Ok("user".to_string()));

        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        runtime.expect_is_privileged().returning(|| false);

        runtime
            .expect_canonicalize()
            .returning(|p| Ok(p.to_path_buf()));

        runtime.expect_current_dir().returning(|| Ok(test_home()));
    }
}
