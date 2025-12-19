use anyhow::Result;
use log::debug;
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};

use std::path::PathBuf;

use crate::{
    archive::{ArchiveExtractor, Extractor},
    github::{GetReleases, GitHub},
    runtime::Runtime,
};

pub struct Config<R: Runtime, G: GetReleases, E: Extractor> {
    pub runtime: R,
    pub github: G,
    pub client: Client,
    pub extractor: E,
    pub install_root: Option<PathBuf>,
}

impl<R: Runtime> Config<R, GitHub, ArchiveExtractor> {
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

        let extractor = ArchiveExtractor;

        Ok(Self {
            runtime,
            github,
            client,
            extractor,
            install_root,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockito::Server;

    // when GITHUB_TOKEN is set, Config::new should use it for authentication
    #[tokio::test]
    async fn test_config_new_with_github_token() {
        let token = "test_token";
        let mut runtime = MockRuntime::new();
        runtime
            .env_vars
            .insert("GITHUB_TOKEN".to_string(), token.to_string());

        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/")
            .match_header("Authorization", format!("Bearer {}", token).as_str())
            .create();

        let config = Config::new(runtime, None, None).unwrap();
        let client = &config.client;
        let _ = client.get(server.url()).send().await;

        mock.assert();
    }
}
