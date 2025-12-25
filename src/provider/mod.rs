//! Provider abstraction for package registries.
//!
//! This module provides a unified interface for different package providers
//! (GitHub, GitLab, Gitee, etc.), enabling multi-platform support.

mod github;
mod registry;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub use github::GitHubProvider;
pub use registry::{PackageSpec, ProviderRegistry};

/// Repository identifier (owner/repo format).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoId {
    pub owner: String,
    pub repo: String,
}

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

impl FromStr for RepoId {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            anyhow::bail!("Invalid repository format. Expected 'owner/repo'.")
        } else {
            Ok(RepoId {
                owner: parts[0].to_string(),
                repo: parts[1].to_string(),
            })
        }
    }
}

/// Repository metadata from the source.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoMetadata {
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub updated_at: Option<String>,
}

/// A downloadable asset from a release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    pub download_url: String,
}

/// A release from the provider.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Release {
    /// Version tag (e.g., "v1.0.0")
    pub tag: String,
    /// Release name/title
    pub name: Option<String>,
    /// Publication date (ISO 8601)
    pub published_at: Option<String>,
    /// Whether this is a pre-release
    pub prerelease: bool,
    /// URL to download the source tarball
    pub tarball_url: String,
    /// Downloadable assets
    pub assets: Vec<ReleaseAsset>,
}

/// Provider kind identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    #[default]
    GitHub,
    GitLab,
    Gitee,
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderKind::GitHub => write!(f, "github"),
            ProviderKind::GitLab => write!(f, "gitlab"),
            ProviderKind::Gitee => write!(f, "gitee"),
        }
    }
}

impl FromStr for ProviderKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "github" => Ok(ProviderKind::GitHub),
            "gitlab" => Ok(ProviderKind::GitLab),
            "gitee" => Ok(ProviderKind::Gitee),
            _ => anyhow::bail!(
                "Unknown provider kind: {}. Expected github, gitlab, or gitee.",
                s
            ),
        }
    }
}

/// Trait for package providers (GitHub, GitLab, etc.).
///
/// This trait abstracts the operations needed to fetch package information
/// from different code hosting platforms.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait Provider: Send + Sync {
    /// Get the provider kind.
    fn kind(&self) -> ProviderKind;

    /// Get the API base URL.
    fn api_url(&self) -> &str;

    /// Fetch repository metadata.
    async fn get_repo_metadata(&self, repo: &RepoId) -> Result<RepoMetadata>;

    /// Fetch all releases for a repository.
    async fn get_releases(&self, repo: &RepoId) -> Result<Vec<Release>>;

    /// Fetch repository metadata from a specific API URL.
    async fn get_repo_metadata_at(&self, repo: &RepoId, api_url: &str) -> Result<RepoMetadata>;

    /// Fetch all releases from a specific API URL.
    async fn get_releases_at(&self, repo: &RepoId, api_url: &str) -> Result<Vec<Release>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_id_parse() {
        let repo: RepoId = "owner/repo".parse().unwrap();
        assert_eq!(repo.owner, "owner");
        assert_eq!(repo.repo, "repo");
    }

    #[test]
    fn test_repo_id_display() {
        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        assert_eq!(repo.to_string(), "owner/repo");
    }

    #[test]
    fn test_repo_id_invalid() {
        assert!("invalid".parse::<RepoId>().is_err());
        assert!("".parse::<RepoId>().is_err());
        assert!("/repo".parse::<RepoId>().is_err());
        assert!("owner/".parse::<RepoId>().is_err());
    }

    #[test]
    fn test_provider_kind_parse() {
        assert_eq!(
            "github".parse::<ProviderKind>().unwrap(),
            ProviderKind::GitHub
        );
        assert_eq!(
            "GitHub".parse::<ProviderKind>().unwrap(),
            ProviderKind::GitHub
        );
        assert_eq!(
            "gitlab".parse::<ProviderKind>().unwrap(),
            ProviderKind::GitLab
        );
        assert_eq!(
            "gitee".parse::<ProviderKind>().unwrap(),
            ProviderKind::Gitee
        );
        assert!("unknown".parse::<ProviderKind>().is_err());
    }

    #[test]
    fn test_provider_kind_display() {
        assert_eq!(ProviderKind::GitHub.to_string(), "github");
        assert_eq!(ProviderKind::GitLab.to_string(), "gitlab");
        assert_eq!(ProviderKind::Gitee.to_string(), "gitee");
    }
}
