use anyhow::Result;
use log::debug;
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};

use std::path::PathBuf;

use crate::{
    archive::{ArchiveExtractor, Extractor},
    download::{Downloader, HttpDownloader},
    github::{GetReleases, GitHub},
    http::HttpClient,
    runtime::Runtime,
};

pub struct Config<R: Runtime, G: GetReleases, E: Extractor, D: Downloader> {
    pub runtime: R,
    pub github: G,
    pub downloader: D,
    pub extractor: E,
    pub install_root: Option<PathBuf>,
}

impl<R: Runtime> Config<R, GitHub, ArchiveExtractor, HttpDownloader> {
    pub fn new(runtime: R, install_root: Option<PathBuf>, api_url: Option<String>) -> Result<Self> {
        let mut headers = HeaderMap::new();
        if let Ok(token) = runtime.env_var("GITHUB_TOKEN") {
            let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", token))?;
            auth_value.set_sensitive(true);
            headers.insert(AUTHORIZATION, auth_value);
            debug!(
                "Using GITHUB_TOKEN for authentication: {}*********{}",
                &token[..8],
                &token[token.len() - 4..]
            );
        }

        let client = Client::builder()
            .user_agent("ghri-cli")
            .default_headers(headers)
            .build()?;

        let github = GitHub::new(client.clone(), api_url);
        let http_client = HttpClient::new(client);
        let extractor = ArchiveExtractor;
        let downloader = HttpDownloader::new(http_client);

        Ok(Self {
            runtime,
            github,
            downloader,
            extractor,
            install_root,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockito::{Matcher, Server};

    /// Helper function to verify Authorization header behavior
    /// - `token`: Some(token) to test with GITHUB_TOKEN set, None to test without
    async fn verify_authorization_header(token: Option<&str>) {
        // --- Setup MockRuntime ---

        let mut runtime = MockRuntime::new();
        let token_clone = token.map(|t| t.to_string());

        runtime
            .expect_env_var()
            .with(mockall::predicate::eq("GITHUB_TOKEN"))
            .returning(move |_| token_clone.clone().ok_or(std::env::VarError::NotPresent));

        // --- Create Mock Server ---

        let mut server = Server::new_async().await;

        let expected_header = match token {
            Some(t) => Matcher::Exact(format!("Bearer {}", t)),
            None => Matcher::Missing,
        };

        let mock = server
            .mock("GET", "/")
            .match_header("Authorization", expected_header)
            .create();

        // --- Execute ---

        let config = Config::new(runtime, None, None).unwrap();
        let client = config.downloader.http_client().inner();
        let _ = client.get(server.url()).send().await;

        // --- Verify ---

        mock.assert();
    }

    #[tokio::test]
    async fn test_config_new_with_github_token() {
        // Test that GITHUB_TOKEN is used for authentication when set
        verify_authorization_header(Some("test_token")).await;
    }

    #[tokio::test]
    async fn test_config_new_without_github_token() {
        // Test that no Authorization header is sent when GITHUB_TOKEN is not set
        verify_authorization_header(None).await;
    }
}
