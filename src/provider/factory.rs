//! Provider factory for creating package providers.
//!
//! This module provides a factory that creates providers on demand
//! based on PackageSpec or Meta information.

use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Result, anyhow};

use super::{Provider, ProviderKind, RepoId, create_provider};
use crate::http::HttpClient;
use crate::package::Meta;

/// Package specification for identifying a package and its provider.
/// Format: "owner/repo" or "owner/repo@version"
#[derive(Debug, Clone, PartialEq)]
pub struct PackageSpec {
    /// Repository identifier (owner/repo format)
    pub repo: RepoId,
    /// Version constraint (None = latest stable)
    pub version: Option<String>,
    /// Provider kind (None = use default or infer)
    pub provider_kind: Option<ProviderKind>,
    /// Custom API URL (overrides default for the provider kind)
    pub api_url: Option<String>,
}

impl std::fmt::Display for PackageSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.version {
            Some(v) => write!(f, "{}@{}", self.repo, v),
            None => write!(f, "{}", self.repo),
        }
    }
}

impl FromStr for PackageSpec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Split by @ to get optional version
        let (repo_part, version) = if let Some(at_pos) = s.rfind('@') {
            let (repo, ver) = s.split_at(at_pos);
            let ver = &ver[1..]; // Skip the @
            if ver.is_empty() {
                return Err(anyhow!(
                    "Invalid format: version after @ cannot be empty. Expected 'owner/repo@version'."
                ));
            }
            (repo, Some(ver.to_string()))
        } else {
            (s, None)
        };

        let repo = repo_part.parse::<RepoId>()?;
        Ok(PackageSpec {
            repo,
            version,
            provider_kind: None,
            api_url: None,
        })
    }
}

impl PackageSpec {
    /// Create a new package spec with just a repo ID.
    pub fn new(repo: RepoId) -> Self {
        Self {
            repo,
            version: None,
            provider_kind: None,
            api_url: None,
        }
    }

    /// Create a package spec with a specific version.
    pub fn with_version(repo: RepoId, version: impl Into<String>) -> Self {
        Self {
            repo,
            version: Some(version.into()),
            provider_kind: None,
            api_url: None,
        }
    }

    /// Set the provider kind.
    pub fn provider(mut self, kind: ProviderKind) -> Self {
        self.provider_kind = Some(kind);
        self
    }

    /// Set a custom API URL.
    pub fn api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = Some(url.into());
        self
    }
}

/// Factory for creating package providers on demand.
///
/// The factory creates providers based on:
/// - PackageSpec (for new installations)
/// - Meta (for updates/upgrades of existing packages)
pub struct ProviderFactory {
    http_client: HttpClient,
    /// Default API URL for GitHub (can be overridden for GitHub Enterprise)
    default_github_api_url: String,
}

impl ProviderFactory {
    /// Create a new factory with the given HTTP client and default GitHub API URL.
    pub fn new(http_client: HttpClient, default_github_api_url: &str) -> Self {
        Self {
            http_client,
            default_github_api_url: default_github_api_url.to_string(),
        }
    }

    /// Get the default API URL for a provider kind.
    fn default_api_url(&self, kind: ProviderKind) -> &str {
        match kind {
            ProviderKind::GitHub => &self.default_github_api_url,
            ProviderKind::GitLab => "https://gitlab.com/api/v4",
            ProviderKind::Gitee => "https://gitee.com/api/v5",
        }
    }

    /// Create a provider for the given kind and API URL.
    pub fn create(&self, kind: ProviderKind, api_url: &str) -> Arc<dyn Provider> {
        create_provider(self.http_client.clone(), kind, api_url)
    }

    /// Create a provider from installed package metadata.
    ///
    /// The provider kind is inferred from the stored API URL, and the
    /// exact API URL from meta is used (for GitHub Enterprise, etc.).
    pub fn provider_for_meta(&self, meta: &Meta) -> Arc<dyn Provider> {
        let kind = Self::infer_provider_kind(&meta.api_url);
        self.create(kind, &meta.api_url)
    }

    /// Create the default provider (GitHub with configured API URL).
    pub fn default_provider(&self) -> Arc<dyn Provider> {
        self.create(
            ProviderKind::GitHub,
            self.default_api_url(ProviderKind::GitHub),
        )
    }

    /// Infer provider kind from an API URL.
    ///
    /// - URLs containing "gitlab" -> GitLab
    /// - URLs containing "gitee" -> Gitee
    /// - Otherwise -> GitHub (default, includes github.com and GitHub Enterprise)
    pub fn infer_provider_kind(api_url: &str) -> ProviderKind {
        let url_lower = api_url.to_lowercase();
        if url_lower.contains("gitlab") {
            ProviderKind::GitLab
        } else if url_lower.contains("gitee") {
            ProviderKind::Gitee
        } else {
            ProviderKind::GitHub
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_factory() -> ProviderFactory {
        let http_client = HttpClient::new(reqwest::Client::new());
        ProviderFactory::new(http_client, "https://api.github.com")
    }

    #[test]
    fn test_default_provider() {
        let factory = make_test_factory();
        let provider = factory.default_provider();
        assert_eq!(provider.kind(), ProviderKind::GitHub);
        assert_eq!(provider.api_url(), "https://api.github.com");
    }

    #[test]
    fn test_from_meta() {
        let factory = make_test_factory();
        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://api.github.com".into(),
            ..Default::default()
        };

        let provider = factory.provider_for_meta(&meta);
        assert_eq!(provider.kind(), ProviderKind::GitHub);
        assert_eq!(provider.api_url(), "https://api.github.com");
    }

    #[test]
    fn test_from_meta_github_enterprise() {
        let factory = make_test_factory();
        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://github.mycompany.com/api/v3".into(),
            ..Default::default()
        };

        let provider = factory.provider_for_meta(&meta);
        assert_eq!(provider.kind(), ProviderKind::GitHub);
        assert_eq!(provider.api_url(), "https://github.mycompany.com/api/v3");
    }

    #[test]
    fn test_infer_provider_kind() {
        assert_eq!(
            ProviderFactory::infer_provider_kind("https://api.github.com"),
            ProviderKind::GitHub
        );
        assert_eq!(
            ProviderFactory::infer_provider_kind("https://gitlab.com/api/v4"),
            ProviderKind::GitLab
        );
        assert_eq!(
            ProviderFactory::infer_provider_kind("https://gitee.com/api/v5"),
            ProviderKind::Gitee
        );
        assert_eq!(
            ProviderFactory::infer_provider_kind("https://unknown.com/api"),
            ProviderKind::GitHub
        );
    }

    #[test]
    fn test_package_spec_builder() {
        let spec = PackageSpec::new("owner/repo".parse().unwrap())
            .provider(ProviderKind::GitLab)
            .api_url("https://custom.gitlab.com/api/v4");

        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.provider_kind, Some(ProviderKind::GitLab));
        assert_eq!(
            spec.api_url,
            Some("https://custom.gitlab.com/api/v4".into())
        );
    }

    #[test]
    fn test_package_spec_with_version() {
        let spec = PackageSpec::with_version("owner/repo".parse().unwrap(), "v1.0.0");
        assert_eq!(spec.version, Some("v1.0.0".into()));
    }

    // Tests migrated from repo_spec.rs

    #[test]
    fn test_parse_package_spec_without_version() {
        let spec = PackageSpec::from_str("owner/repo").unwrap();
        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.version, None);
    }

    #[test]
    fn test_parse_package_spec_with_version() {
        let spec = PackageSpec::from_str("owner/repo@v1.0.0").unwrap();
        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.version, Some("v1.0.0".to_string()));
    }

    #[test]
    fn test_parse_package_spec_with_version_no_v_prefix() {
        let spec = PackageSpec::from_str("bach-sh/bach@0.7.2").unwrap();
        assert_eq!(spec.repo.owner, "bach-sh");
        assert_eq!(spec.repo.repo, "bach");
        assert_eq!(spec.version, Some("0.7.2".to_string()));
    }

    #[test]
    fn test_parse_package_spec_empty_version_fails() {
        let result = PackageSpec::from_str("owner/repo@");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_parse_package_spec_invalid_repo_fails() {
        let result = PackageSpec::from_str("invalid@v1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_package_spec_display_without_version() {
        let spec = PackageSpec {
            repo: RepoId {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
            },
            version: None,
            provider_kind: None,
            api_url: None,
        };
        assert_eq!(format!("{}", spec), "owner/repo");
    }

    #[test]
    fn test_package_spec_display_with_version() {
        let spec = PackageSpec {
            repo: RepoId {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
            },
            version: Some("v1.0.0".to_string()),
            provider_kind: None,
            api_url: None,
        };
        assert_eq!(format!("{}", spec), "owner/repo@v1.0.0");
    }
}
