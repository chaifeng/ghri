use anyhow::{Context, Result};
use log::debug;
use std::path::PathBuf;

use crate::runtime::Runtime;

/// Application configuration loaded from environment and CLI overrides.
/// This struct contains only configuration values, not service dependencies.
#[derive(Debug, Clone)]
pub struct Config {
    /// Installation root directory (e.g., ~/.ghri or /opt/ghri)
    pub install_root: PathBuf,
    /// GitHub API URL (e.g., https://api.github.com)
    pub api_url: String,
    /// GitHub authentication token (optional)
    pub token: Option<String>,
}

/// Overrides that can be provided via CLI arguments
#[derive(Debug, Default)]
pub struct ConfigOverrides {
    /// Override the default install root
    pub install_root: Option<PathBuf>,
    /// Override the default GitHub API URL
    pub api_url: Option<String>,
}

impl Config {
    /// Default GitHub API URL
    pub const DEFAULT_API_URL: &'static str = "https://api.github.com";

    /// Load configuration from runtime environment with optional CLI overrides
    pub fn load<R: Runtime>(runtime: &R, overrides: ConfigOverrides) -> Result<Self> {
        // Determine install root: CLI override > default based on privilege
        let install_root = match overrides.install_root {
            Some(path) => path,
            None => Self::default_install_root(runtime)?,
        };

        // Determine API URL: CLI override > default
        let api_url = overrides
            .api_url
            .unwrap_or_else(|| Self::DEFAULT_API_URL.to_string());

        // Load token from environment
        let token = runtime.env_var("GITHUB_TOKEN").ok();

        if let Some(ref t) = token {
            if t.len() >= 12 {
                debug!(
                    "Using GITHUB_TOKEN for authentication: {}*********{}",
                    &t[..8],
                    &t[t.len() - 4..]
                );
            } else {
                debug!("Using GITHUB_TOKEN for authentication");
            }
        }

        Ok(Self {
            install_root,
            api_url,
            token,
        })
    }

    /// Get the default installation root directory based on user privilege
    fn default_install_root<R: Runtime>(runtime: &R) -> Result<PathBuf> {
        if runtime.is_privileged() {
            Ok(Self::system_install_root())
        } else {
            let home_dir = runtime
                .home_dir()
                .context("Could not find home directory")?;
            Ok(home_dir.join(".ghri"))
        }
    }

    /// Get the system-wide installation root (for privileged users)
    #[cfg(target_os = "macos")]
    fn system_install_root() -> PathBuf {
        PathBuf::from("/opt/ghri")
    }

    #[cfg(target_os = "windows")]
    fn system_install_root() -> PathBuf {
        PathBuf::from(r"C:\ProgramData\ghri")
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn system_install_root() -> PathBuf {
        PathBuf::from("/usr/local/ghri")
    }

    /// Get the package directory for a given repo
    pub fn package_dir(&self, owner: &str, repo: &str) -> PathBuf {
        self.install_root.join(owner).join(repo)
    }

    /// Get the version directory for a given repo and version
    pub fn version_dir(&self, owner: &str, repo: &str, version: &str) -> PathBuf {
        self.package_dir(owner, repo).join(version)
    }

    /// Get the meta.json path for a given repo
    pub fn meta_path(&self, owner: &str, repo: &str) -> PathBuf {
        self.package_dir(owner, repo).join("meta.json")
    }
}

/// Options for the install command (behavior parameters)
#[derive(Debug, Default, Clone)]
pub struct InstallOptions {
    /// Asset name filters (e.g., ["*linux*", "*x86_64*"])
    pub filters: Vec<String>,
    /// Allow installing pre-release versions
    pub pre: bool,
    /// Skip confirmation prompts
    pub yes: bool,
    /// Prune old versions after installation
    pub prune: bool,
}

/// Options for the upgrade command (behavior parameters)
#[derive(Debug, Default, Clone)]
pub struct UpgradeOptions {
    /// Allow upgrading to pre-release versions
    pub pre: bool,
    /// Skip confirmation prompts
    pub yes: bool,
    /// Prune old versions after upgrade
    pub prune: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;

    #[test]
    fn test_config_load_defaults() {
        // Test loading config with default values (no overrides)
        let mut runtime = MockRuntime::new();

        runtime.expect_is_privileged().returning(|| false);
        runtime
            .expect_home_dir()
            .returning(|| Some(PathBuf::from("/home/user")));
        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        let config = Config::load(&runtime, ConfigOverrides::default()).unwrap();

        assert_eq!(config.install_root, PathBuf::from("/home/user/.ghri"));
        assert_eq!(config.api_url, Config::DEFAULT_API_URL);
        assert!(config.token.is_none());
    }

    #[test]
    fn test_config_load_with_overrides() {
        // Test loading config with CLI overrides
        let mut runtime = MockRuntime::new();

        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Ok("test_token".to_string()));

        let overrides = ConfigOverrides {
            install_root: Some(PathBuf::from("/custom/root")),
            api_url: Some("https://github.example.com/api/v3".to_string()),
        };

        let config = Config::load(&runtime, overrides).unwrap();

        assert_eq!(config.install_root, PathBuf::from("/custom/root"));
        assert_eq!(config.api_url, "https://github.example.com/api/v3");
        assert_eq!(config.token, Some("test_token".to_string()));
    }

    #[test]
    fn test_config_load_privileged_user() {
        // Test that privileged users get system install root
        let mut runtime = MockRuntime::new();

        runtime.expect_is_privileged().returning(|| true);
        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        let config = Config::load(&runtime, ConfigOverrides::default()).unwrap();

        #[cfg(target_os = "macos")]
        assert_eq!(config.install_root, PathBuf::from("/opt/ghri"));
        #[cfg(all(unix, not(target_os = "macos")))]
        assert_eq!(config.install_root, PathBuf::from("/usr/local/ghri"));
        #[cfg(target_os = "windows")]
        assert_eq!(config.install_root, PathBuf::from("C:\\ProgramData\\ghri"));
    }

    #[test]
    fn test_config_load_no_home_dir() {
        // Test that config fails to load when home dir is unavailable for non-privileged user
        let mut runtime = MockRuntime::new();

        runtime.expect_is_privileged().returning(|| false);
        runtime.expect_home_dir().returning(|| None);

        let result = Config::load(&runtime, ConfigOverrides::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_config_path_helpers() {
        let config = Config {
            install_root: PathBuf::from("/root"),
            api_url: Config::DEFAULT_API_URL.to_string(),
            token: None,
        };

        assert_eq!(
            config.package_dir("owner", "repo"),
            PathBuf::from("/root/owner/repo")
        );
        assert_eq!(
            config.version_dir("owner", "repo", "v1.0.0"),
            PathBuf::from("/root/owner/repo/v1.0.0")
        );
        assert_eq!(
            config.meta_path("owner", "repo"),
            PathBuf::from("/root/owner/repo/meta.json")
        );
    }
}
