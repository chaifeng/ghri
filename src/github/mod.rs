use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use log::debug;
use reqwest::Client;
use serde::Deserialize;
use std::str::FromStr;

const GITHUB_API_URL: &str = "https://api.github.com";

#[async_trait]
pub trait GetReleases {
    async fn get_latest_release(&self, repo: &GitHubRepo) -> Result<Release>;
}

pub struct GitHub {
    pub client: Client,
}

#[async_trait]
impl GetReleases for GitHub {
    async fn get_latest_release(&self, repo: &GitHubRepo) -> Result<Release> {
        GitHub::get_latest_release(repo, &self.client, GITHUB_API_URL).await
    }
}

impl GitHub {
    /// Fetches the latest release information from GitHub.
    pub async fn get_latest_release(
        repo: &GitHubRepo,
        client: &Client,
        api_url: &str,
    ) -> Result<Release> {
        let url = format!(
            "{}/repos/{}/{}/releases/latest",
            api_url, repo.owner, repo.repo
        );

        debug!("Fetching latest release from {}...", url);

        let response = client
            .get(&url)
            .send()
            .await
            .context("Failed to send request to GitHub API")?;

        let release = response
            .error_for_status()?
            .json::<Release>()
            .await
            .context("Failed to parse JSON response from GitHub API")?;

        Ok(release)
    }
}

#[derive(Debug, PartialEq)]
pub struct GitHubRepo {
    pub owner: String,
    pub repo: String,
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

/// Represents a GitHub release asset
#[derive(Deserialize, Debug, PartialEq, Clone)]
pub struct Release {
    pub tag_name: String,
    pub tarball_url: String,
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

    #[test]
    fn test_parse_github_repo_invalid() {
        assert!(GitHubRepo::from_str("owner").is_err());
        assert!(GitHubRepo::from_str("owner/").is_err());
        assert!(GitHubRepo::from_str("/repo").is_err());
        assert!(GitHubRepo::from_str("owner/repo/extra").is_err());
    }

    #[test]
    fn test_get_latest_release() {
        let mut server = mockito::Server::new();
        let url = server.url();

        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
        };

        let mock = server
            .mock("GET", "/repos/owner/repo/releases/latest")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"tag_name": "v1.0.0", "tarball_url": "https://example.com/v1.0.0.tar.gz"}"#,
            )
            .create();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let release = rt
            .block_on(async {
                let client = Client::new();
                GitHub::get_latest_release(&repo, &client, &url).await
            })
            .unwrap();

        mock.assert();
        assert_eq!(
            release,
            Release {
                tag_name: "v1.0.0".to_string(),
                tarball_url: "https://example.com/v1.0.0.tar.gz".to_string()
            }
        );
    }
}
