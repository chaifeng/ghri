//! Service factory for building application dependencies.
//!
//! This module separates the construction of service dependencies (Provider client,
//! downloader, extractor) from the configuration. Services are built based on
//! configuration values but are not part of the configuration itself.

use std::sync::Arc;

use anyhow::Result;
use log::debug;
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};

use crate::{
    archive::ArchiveExtractorImpl,
    download::HttpDownloader,
    http::HttpClient,
    provider::{GitHubProvider, Provider, ProviderKind, ProviderRegistry},
};

use super::config::Config;

/// Build an HTTP client with optional authentication token
pub fn build_http_client(token: Option<&str>) -> Result<HttpClient> {
    let mut headers = HeaderMap::new();

    if let Some(token) = token {
        let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", token))?;
        auth_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_value);
        debug!("HTTP client configured with authentication");
    }

    let client = Client::builder()
        .user_agent("ghri-cli")
        .default_headers(headers)
        .build()?;

    Ok(HttpClient::new(client))
}

/// Build a Provider (GitHub) client from configuration
pub fn build_provider(config: &Config) -> Result<GitHubProvider> {
    let http_client = build_http_client(config.token.as_deref())?;
    Ok(GitHubProvider::from_http_client(
        http_client,
        &config.api_url,
    ))
}

/// Build a ProviderRegistry with all available providers
pub fn build_provider_registry(config: &Config) -> Result<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();

    // Register GitHub provider (default)
    let github_provider = build_provider(config)?;
    registry.register(Arc::new(github_provider));

    // Future: Register GitLab, Gitee providers here
    // registry.register(Arc::new(build_gitlab_provider(config)?));
    // registry.register(Arc::new(build_gitee_provider(config)?));

    Ok(registry)
}

/// Build a downloader from configuration
pub fn build_downloader(config: &Config) -> Result<HttpDownloader> {
    let http_client = build_http_client(config.token.as_deref())?;
    Ok(HttpDownloader::new(http_client))
}

/// Build an archive extractor (stateless, no configuration needed)
pub fn build_extractor() -> ArchiveExtractorImpl {
    ArchiveExtractorImpl::new()
}

/// Container for services using ProviderRegistry for multi-platform support.
pub struct RegistryServices {
    pub registry: ProviderRegistry,
    pub downloader: HttpDownloader,
    pub extractor: ArchiveExtractorImpl,
}

impl RegistryServices {
    /// Build services with a provider registry from configuration
    pub fn from_config(config: &Config) -> Result<Self> {
        Ok(Self {
            registry: build_provider_registry(config)?,
            downloader: build_downloader(config)?,
            extractor: build_extractor(),
        })
    }

    /// Get a provider by kind from the registry
    pub fn get_provider(&self, kind: ProviderKind) -> Option<&Arc<dyn Provider>> {
        self.registry.get(kind)
    }

    /// Get the default provider (GitHub)
    pub fn default_provider(&self) -> Option<&Arc<dyn Provider>> {
        self.registry.get(self.registry.default_kind())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[tokio::test]
    async fn test_build_http_client_with_token() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/")
            .match_header(
                "Authorization",
                Matcher::Exact("Bearer test_token".to_string()),
            )
            .create();

        let http_client = build_http_client(Some("test_token")).unwrap();
        let _ = http_client.inner().get(server.url()).send().await;

        mock.assert();
    }

    #[tokio::test]
    async fn test_build_http_client_without_token() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/")
            .match_header("Authorization", Matcher::Missing)
            .create();

        let http_client = build_http_client(None).unwrap();
        let _ = http_client.inner().get(server.url()).send().await;

        mock.assert();
    }

    #[test]
    fn test_build_provider_registry() {
        let config = Config {
            install_root: std::path::PathBuf::from("/test"),
            api_url: "https://api.github.com".to_string(),
            token: None,
        };

        let registry = build_provider_registry(&config).unwrap();

        // Should have GitHub registered
        assert!(registry.has(ProviderKind::GitHub));
        assert_eq!(registry.default_kind(), ProviderKind::GitHub);
    }

    #[test]
    fn test_registry_services_from_config() {
        let config = Config {
            install_root: std::path::PathBuf::from("/test"),
            api_url: "https://api.github.com".to_string(),
            token: None,
        };

        let services = RegistryServices::from_config(&config).unwrap();

        // Should have default provider available
        assert!(services.default_provider().is_some());
        assert!(services.get_provider(ProviderKind::GitHub).is_some());
    }
}
