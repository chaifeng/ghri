use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use log::debug;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[async_trait]
pub trait GetReleases: Send + Sync {
    async fn get_repo_info(&self, repo: &GitHubRepo) -> Result<RepoInfo>;
    async fn get_releases(&self, repo: &GitHubRepo) -> Result<Vec<Release>>;
    async fn get_repo_info_at(&self, repo: &GitHubRepo, api_url: &str) -> Result<RepoInfo>;
    async fn get_releases_at(&self, repo: &GitHubRepo, api_url: &str) -> Result<Vec<Release>>;
    fn api_url(&self) -> &str;
}

pub struct GitHub {
    pub client: Client,
    pub api_url: String,
}

impl GitHub {
    #[tracing::instrument(skip(client, api_url))]
    pub fn new(client: Client, api_url: Option<String>) -> Self {
        let api_url = api_url.unwrap_or_else(|| "https://api.github.com".to_string());
        Self { client, api_url }
    }
}

#[async_trait]
impl GetReleases for GitHub {
    #[tracing::instrument(skip(self, repo))]
    async fn get_repo_info(&self, repo: &GitHubRepo) -> Result<RepoInfo> {
        self.get_repo_info_at(repo, &self.api_url).await
    }

    #[tracing::instrument(skip(self, repo))]
    async fn get_releases(&self, repo: &GitHubRepo) -> Result<Vec<Release>> {
        self.get_releases_at(repo, &self.api_url).await
    }

    #[tracing::instrument(skip(self, repo, api_url))]
    async fn get_repo_info_at(&self, repo: &GitHubRepo, api_url: &str) -> Result<RepoInfo> {
        GitHub::get_repo_info(repo, &self.client, api_url).await
    }

    #[tracing::instrument(skip(self, repo, api_url))]
    async fn get_releases_at(&self, repo: &GitHubRepo, api_url: &str) -> Result<Vec<Release>> {
        GitHub::get_releases(repo, &self.client, api_url).await
    }

    #[tracing::instrument(skip(self))]
    fn api_url(&self) -> &str {
        &self.api_url
    }
}

impl GitHub {
    #[tracing::instrument(skip(client, api_url))]
    pub async fn get_repo_info(
        repo: &GitHubRepo,
        client: &Client,
        api_url: &str,
    ) -> Result<RepoInfo> {
        let url = format!("{}/repos/{}/{}", api_url, repo.owner, repo.repo);

        debug!("Fetching repo info from {}...", url);

        let response = client
            .get(&url)
            .send()
            .await
            .context("Failed to send request to GitHub API")?;

        let info = response
            .error_for_status()?
            .json::<RepoInfo>()
            .await
            .context("Failed to parse JSON response from GitHub API")?;

        Ok(info)
    }

    #[tracing::instrument(skip(client, api_url))]
    pub async fn get_releases(
        repo: &GitHubRepo,
        client: &Client,
        api_url: &str,
    ) -> Result<Vec<Release>> {
        let mut releases = Vec::new();
        let mut page = 1;

        // Limit to 10 pages (1000 releases) to be safe for now/prevent infinite loop
        while page <= 10 {
            let url = format!("{}/repos/{}/{}/releases", api_url, repo.owner, repo.repo);

            let request = client
                .get(&url)
                .query(&[("per_page", "100"), ("page", &page.to_string())]);

            debug!("Fetching releases page {} from {}...", page, url);

            let response = request
                .send()
                .await
                .context("Failed to send request to GitHub API")?;

            let parsed: Vec<Release> = response
                .error_for_status()?
                .json()
                .await
                .context("Failed to parse JSON response from GitHub API")?;

            if parsed.is_empty() {
                break;
            }

            let len = parsed.len();
            releases.extend(parsed);

            if len < 100 {
                break;
            }

            page += 1;
        }

        Ok(releases)
    }

    pub async fn get_repo_info_at(&self, repo: &GitHubRepo, api_url: &str) -> Result<RepoInfo> {
        GitHub::get_repo_info(repo, &self.client, api_url).await
    }

    pub async fn get_releases_at(&self, repo: &GitHubRepo, api_url: &str) -> Result<Vec<Release>> {
        GitHub::get_releases(repo, &self.client, api_url).await
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct GitHubRepo {
    pub owner: String,
    pub repo: String,
}

impl std::fmt::Display for GitHubRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

impl FromStr for GitHubRepo {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            Err(anyhow!("Invalid repository format. Expected 'owner/repo'."))
        } else {
            Ok(GitHubRepo {
                owner: parts[0].to_string(),
                repo: parts[1].to_string(),
            })
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct RepoInfo {
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<License>,
    pub updated_at: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct License {
    pub key: String,
    pub name: String,
}

/// Represents a GitHub release asset
#[derive(Deserialize, Serialize, Debug, PartialEq, Clone)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
}

/// Represents a GitHub release
#[derive(Deserialize, Serialize, Debug, PartialEq, Clone, Default)]
pub struct Release {
    pub tag_name: String,
    pub tarball_url: String,
    pub name: Option<String>,
    pub published_at: Option<String>,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_repo_valid() {
        let repo_str = "owner/repo";
        let repo = GitHubRepo::from_str(repo_str).unwrap();
        assert_eq!(
            repo,
            GitHubRepo {
                owner: "owner".to_string(),
                repo: "repo".to_string()
            }
        );
    }

    #[tokio::test]
    async fn test_get_repo_info_not_found() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let repo = GitHubRepo {
            owner: "test-owner".to_string(),
            repo: "test-repo".to_string(),
        };

        let mock = server
            .mock("GET", "/repos/test-owner/test-repo")
            .with_status(404)
            .create_async()
            .await;

        let client = Client::new();
        let result = GitHub::get_repo_info(&repo, &client, &url).await;

        mock.assert_async().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_repo_info() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let repo = GitHubRepo {
            owner: "test-owner".to_string(),
            repo: "test-repo".to_string(),
        };

        let mock = server
            .mock("GET", "/repos/test-owner/test-repo")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "description": "A test repo",
                    "homepage": "https://example.com",
                    "license": { "key": "mit", "name": "MIT License" },
                    "updated_at": "2023-01-01T00:00:00Z"
                }"#,
            )
            .create_async()
            .await;

        let client = Client::new();
        let repo_info = GitHub::get_repo_info(&repo, &client, &url).await.unwrap();

        mock.assert_async().await;
        assert_eq!(
            repo_info,
            RepoInfo {
                description: Some("A test repo".to_string()),
                homepage: Some("https://example.com".to_string()),
                license: Some(License {
                    key: "mit".to_string(),
                    name: "MIT License".to_string()
                }),
                updated_at: "2023-01-01T00:00:00Z".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn test_get_repo_info_minimal() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let repo = GitHubRepo {
            owner: "test-owner".to_string(),
            repo: "test-repo".to_string(),
        };

        let mock = server
            .mock("GET", "/repos/test-owner/test-repo")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "description": null,
                    "homepage": null,
                    "license": null,
                    "updated_at": "2023-01-01T00:00:00Z"
                }"#,
            )
            .create_async()
            .await;

        let client = Client::new();
        let repo_info = GitHub::get_repo_info(&repo, &client, &url).await.unwrap();

        mock.assert_async().await;
        assert_eq!(
            repo_info,
            RepoInfo {
                description: None,
                homepage: None,
                license: None,
                updated_at: "2023-01-01T00:00:00Z".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn test_get_releases_single_page() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let repo = GitHubRepo {
            owner: "test-owner".to_string(),
            repo: "test-repo".to_string(),
        };

        let mock = server
            .mock(
                "GET",
                "/repos/test-owner/test-repo/releases?per_page=100&page=1",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"[
                    {
                        "tag_name": "v1.0.0",
                        "tarball_url": "url1",
                        "prerelease": false,
                        "assets": []
                    },
                    {
                        "tag_name": "v0.9.0",
                        "tarball_url": "url2",
                        "prerelease": true,
                        "assets": []
                    }
                ]"#,
            )
            .create_async()
            .await;

        let client = Client::new();
        let releases = GitHub::get_releases(&repo, &client, &url).await.unwrap();

        mock.assert_async().await;
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].tag_name, "v1.0.0");
        assert_eq!(releases[1].tag_name, "v0.9.0");
        assert!(releases[1].prerelease);
    }

    #[tokio::test]
    async fn test_get_releases_multiple_pages() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let repo = GitHubRepo {
            owner: "test-owner".to_string(),
            repo: "test-repo".to_string(),
        };

        // Create 100 dummy releases for the first page
        let mut page1_body = String::from("[");
        for i in 0..100 {
            if i > 0 {
                page1_body.push(',');
            }
            page1_body.push_str(&format!(
                r#"{{"tag_name": "v1.0.{}", "tarball_url": "url", "prerelease": false, "assets": []}}"#,
                i
            ));
        }
        page1_body.push(']');

        let mock_p1 = server
            .mock(
                "GET",
                "/repos/test-owner/test-repo/releases?per_page=100&page=1",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(&page1_body)
            .create_async()
            .await;

        let mock_p2 = server
            .mock(
                "GET",
                "/repos/test-owner/test-repo/releases?per_page=100&page=2",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"[
                {"tag_name": "v0.0.1", "tarball_url": "url", "prerelease": false, "assets": []}
            ]"#,
            )
            .create_async()
            .await;

        let client = Client::new();
        let releases = GitHub::get_releases(&repo, &client, &url).await.unwrap();

        mock_p1.assert_async().await;
        mock_p2.assert_async().await;
        assert_eq!(releases.len(), 101);
        assert_eq!(releases[100].tag_name, "v0.0.1");
    }

    #[tokio::test]
    async fn test_get_releases_not_found() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let repo = GitHubRepo {
            owner: "test-owner".to_string(),
            repo: "test-repo".to_string(),
        };

        let mock = server
            .mock(
                "GET",
                "/repos/test-owner/test-repo/releases?per_page=100&page=1",
            )
            .with_status(404)
            .create_async()
            .await;

        let client = Client::new();
        let result = GitHub::get_releases(&repo, &client, &url).await;

        mock.assert_async().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_repo_info_at() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
        };

        let mock = server
            .mock("GET", "/repos/owner/repo")
            .with_status(200)
            .with_body(r#"{"updated_at": "2023-01-01T00:00:00Z"}"#)
            .create_async()
            .await;

        let github = GitHub::new(Client::new(), None);
        let info = github.get_repo_info_at(&repo, &url).await.unwrap();

        mock.assert_async().await;
        assert_eq!(info.updated_at, "2023-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn test_get_releases_at() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
        };

        let mock = server
            .mock("GET", "/repos/owner/repo/releases?per_page=100&page=1")
            .with_status(200)
            .with_body(r#"[{"tag_name": "v1.0.0", "tarball_url": "url", "prerelease": false, "assets": []}]"#)
            .create_async()
            .await;

        let github = GitHub::new(Client::new(), None);
        let releases = github.get_releases_at(&repo, &url).await.unwrap();

        mock.assert_async().await;
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].tag_name, "v1.0.0");
    }
}
