//! Provider factory for creating package providers.
//!
//! This module provides a factory that creates providers on demand
//! based on PackageSpec or Meta information.

use std::sync::Arc;

use super::{Provider, ProviderKind, RepoId, create_provider};
use crate::http::HttpClient;
use crate::package::Meta;

/// Package specification for identifying a package and its provider.
#[derive(Debug, Clone)]
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

    /// Create a provider from a package specification.
    ///
    /// Resolution:
    /// 1. Use `spec.provider_kind` if specified, otherwise use GitHub (default)
    /// 2. Use `spec.api_url` if specified, otherwise use default for the kind
    pub fn from_spec(&self, spec: &PackageSpec) -> Arc<dyn Provider> {
        let kind = spec.provider_kind.unwrap_or(ProviderKind::GitHub);
        let api_url = spec
            .api_url
            .as_deref()
            .unwrap_or_else(|| self.default_api_url(kind));
        self.create(kind, api_url)
    }

    /// Create a provider from installed package metadata.
    ///
    /// The provider kind is inferred from the stored API URL, and the
    /// exact API URL from meta is used (for GitHub Enterprise, etc.).
    pub fn from_meta(&self, meta: &Meta) -> Arc<dyn Provider> {
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
    fn test_from_spec_default() {
        let factory = make_test_factory();
        let spec = PackageSpec::new("owner/repo".parse().unwrap());

        let provider = factory.from_spec(&spec);
        assert_eq!(provider.kind(), ProviderKind::GitHub);
        assert_eq!(provider.api_url(), "https://api.github.com");
    }

    #[test]
    fn test_from_spec_with_custom_api_url() {
        let factory = make_test_factory();
        let spec = PackageSpec::new("owner/repo".parse().unwrap())
            .api_url("https://github.enterprise.com/api/v3");

        let provider = factory.from_spec(&spec);
        assert_eq!(provider.kind(), ProviderKind::GitHub);
        assert_eq!(provider.api_url(), "https://github.enterprise.com/api/v3");
    }

    #[test]
    fn test_from_meta() {
        let factory = make_test_factory();
        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://api.github.com".into(),
            ..Default::default()
        };

        let provider = factory.from_meta(&meta);
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

        let provider = factory.from_meta(&meta);
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
}
