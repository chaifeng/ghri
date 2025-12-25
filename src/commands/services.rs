//! Service factory for building application dependencies.
//!
//! This module separates the construction of service dependencies (Provider factory,
//! downloader, extractor) from the configuration. Services are built based on
//! configuration values but are not part of the configuration itself.

use anyhow::Result;
use log::debug;
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};

use crate::{
    archive::ArchiveExtractorImpl, download::HttpDownloader, http::HttpClient,
    provider::ProviderFactory,
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

/// Build a ProviderFactory from configuration
pub fn build_provider_factory(config: &Config) -> Result<ProviderFactory> {
    let http_client = build_http_client(config.token.as_deref())?;
    Ok(ProviderFactory::new(http_client, &config.api_url))
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

/// Container for application services.
pub struct Services {
    pub provider_factory: ProviderFactory,
    pub downloader: HttpDownloader,
    pub extractor: ArchiveExtractorImpl,
}

impl Services {
    /// Build services from configuration
    pub fn from_config(config: &Config) -> Result<Self> {
        Ok(Self {
            provider_factory: build_provider_factory(config)?,
            downloader: build_downloader(config)?,
            extractor: build_extractor(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderKind;
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
    fn test_build_provider_factory() {
        let config = Config {
            install_root: std::path::PathBuf::from("/test"),
            api_url: "https://api.github.com".to_string(),
            token: None,
        };

        let factory = build_provider_factory(&config).unwrap();
        let provider = factory.default_provider();
        assert_eq!(provider.kind(), ProviderKind::GitHub);
    }
}
