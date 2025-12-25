//! GitHub provider implementation.

use anyhow::Result;
use async_trait::async_trait;
use log::debug;
#[cfg(test)]
use reqwest::Client;

use crate::http::HttpClient;

use super::{Provider, ProviderKind, Release, ReleaseAsset, RepoId, RepoMetadata};

/// GitHub API response types (internal).
mod api {
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    pub struct RepoInfo {
        pub description: Option<String>,
        pub homepage: Option<String>,
        pub license: Option<License>,
        pub updated_at: String,
    }

    #[derive(Deserialize, Debug)]
    pub struct License {
        pub name: String,
    }

    #[derive(Deserialize, Debug)]
    pub struct Release {
        pub tag_name: String,
        pub name: Option<String>,
        pub tarball_url: String,
        pub published_at: Option<String>,
        pub prerelease: bool,
        pub assets: Vec<Asset>,
    }

    #[derive(Deserialize, Debug)]
    pub struct Asset {
        pub name: String,
        pub size: u64,
        pub browser_download_url: String,
    }
}

/// GitHub provider implementation.
pub struct GitHubProvider {
    http_client: HttpClient,
    api_url: String,
}

impl GitHubProvider {
    /// Create a new GitHub provider with default API URL.
    /// Used primarily for testing.
    #[cfg(test)]
    pub fn new(client: Client) -> Self {
        Self::with_api_url(client, "https://api.github.com")
    }

    /// Create a new GitHub provider with custom API URL.
    /// Used primarily for testing.
    #[cfg(test)]
    pub fn with_api_url(client: Client, api_url: &str) -> Self {
        Self {
            http_client: HttpClient::new(client),
            api_url: api_url.to_string(),
        }
    }

    /// Create from an existing HttpClient.
    pub fn from_http_client(http_client: HttpClient, api_url: &str) -> Self {
        Self {
            http_client,
            api_url: api_url.to_string(),
        }
    }

    async fn fetch_repo_info(&self, repo: &RepoId, api_url: &str) -> Result<api::RepoInfo> {
        let url = format!("{}/repos/{}/{}", api_url, repo.owner, repo.repo);
        debug!("Fetching repo info from {}...", url);
        self.http_client.get_json(&url).await
    }

    async fn fetch_releases(&self, repo: &RepoId, api_url: &str) -> Result<Vec<api::Release>> {
        let mut releases = Vec::new();
        let mut page = 1;

        // Limit to 10 pages (1000 releases) to prevent infinite loop
        while page <= 10 {
            let url = format!("{}/repos/{}/{}/releases", api_url, repo.owner, repo.repo);
            debug!("Fetching releases page {} from {}...", page, url);

            let parsed: Vec<api::Release> = self
                .http_client
                .get_json_with_query(&url, &[("per_page", "100"), ("page", &page.to_string())])
                .await?;

            if parsed.is_empty() {
                break;
            }

            releases.extend(parsed);
            page += 1;
        }

        Ok(releases)
    }
}

#[async_trait]
impl Provider for GitHubProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GitHub
    }

    fn api_url(&self) -> &str {
        &self.api_url
    }

    async fn get_repo_metadata(&self, repo: &RepoId) -> Result<RepoMetadata> {
        self.get_repo_metadata_at(repo, &self.api_url.clone()).await
    }

    async fn get_releases(&self, repo: &RepoId) -> Result<Vec<Release>> {
        self.get_releases_at(repo, &self.api_url.clone()).await
    }

    async fn get_repo_metadata_at(&self, repo: &RepoId, api_url: &str) -> Result<RepoMetadata> {
        let info = self.fetch_repo_info(repo, api_url).await?;
        Ok(RepoMetadata {
            description: info.description,
            homepage: info.homepage,
            license: info.license.map(|l| l.name),
            updated_at: Some(info.updated_at),
        })
    }

    async fn get_releases_at(&self, repo: &RepoId, api_url: &str) -> Result<Vec<Release>> {
        let releases = self.fetch_releases(repo, api_url).await?;
        Ok(releases.into_iter().map(|r| r.into()).collect())
    }
}

impl From<api::Release> for Release {
    fn from(r: api::Release) -> Self {
        Release {
            tag: r.tag_name,
            name: r.name,
            published_at: r.published_at,
            prerelease: r.prerelease,
            tarball_url: r.tarball_url,
            assets: r.assets.into_iter().map(|a| a.into()).collect(),
        }
    }
}

impl From<api::Asset> for ReleaseAsset {
    fn from(a: api::Asset) -> Self {
        ReleaseAsset {
            name: a.name,
            size: a.size,
            download_url: a.browser_download_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_github_provider_kind() {
        let client = Client::new();
        let provider = GitHubProvider::new(client);
        assert_eq!(provider.kind(), ProviderKind::GitHub);
    }

    #[test]
    fn test_github_provider_api_url() {
        let client = Client::new();
        let provider = GitHubProvider::new(client);
        assert_eq!(provider.api_url(), "https://api.github.com");

        let custom = GitHubProvider::with_api_url(Client::new(), "https://custom.api");
        assert_eq!(custom.api_url(), "https://custom.api");
    }

    #[test]
    fn test_release_conversion() {
        let api_release = api::Release {
            tag_name: "v1.0.0".into(),
            name: Some("Release 1.0".into()),
            tarball_url: "https://example.com/tarball".into(),
            published_at: Some("2024-01-01T00:00:00Z".into()),
            prerelease: false,
            assets: vec![api::Asset {
                name: "tool-linux-amd64".into(),
                size: 1024,
                browser_download_url: "https://example.com/asset".into(),
            }],
        };

        let release: Release = api_release.into();
        assert_eq!(release.tag, "v1.0.0");
        assert_eq!(release.name, Some("Release 1.0".into()));
        assert!(!release.prerelease);
        assert_eq!(release.assets.len(), 1);
        assert_eq!(release.assets[0].name, "tool-linux-amd64");
    }
}
