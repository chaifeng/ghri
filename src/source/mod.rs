//! Source abstraction for package registries.
//!
//! This module provides a unified interface for different package sources
//! (GitHub, GitLab, Gitee, etc.), enabling multi-platform support.

mod github;
mod registry;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub use github::GitHubSource;
pub use registry::{PackageSpec, SourceRegistry};

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

/// A release from the source.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceRelease {
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

/// Source kind identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    #[default]
    GitHub,
    GitLab,
    Gitee,
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceKind::GitHub => write!(f, "github"),
            SourceKind::GitLab => write!(f, "gitlab"),
            SourceKind::Gitee => write!(f, "gitee"),
        }
    }
}

impl FromStr for SourceKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "github" => Ok(SourceKind::GitHub),
            "gitlab" => Ok(SourceKind::GitLab),
            "gitee" => Ok(SourceKind::Gitee),
            _ => anyhow::bail!(
                "Unknown source kind: {}. Expected github, gitlab, or gitee.",
                s
            ),
        }
    }
}

/// Trait for package sources (GitHub, GitLab, etc.).
///
/// This trait abstracts the operations needed to fetch package information
/// from different code hosting platforms.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait Source: Send + Sync {
    /// Get the source kind.
    fn kind(&self) -> SourceKind;

    /// Get the API base URL.
    fn api_url(&self) -> &str;

    /// Fetch repository metadata.
    async fn get_repo_metadata(&self, repo: &RepoId) -> Result<RepoMetadata>;

    /// Fetch all releases for a repository.
    async fn get_releases(&self, repo: &RepoId) -> Result<Vec<SourceRelease>>;

    /// Fetch repository metadata from a specific API URL.
    async fn get_repo_metadata_at(&self, repo: &RepoId, api_url: &str) -> Result<RepoMetadata>;

    /// Fetch all releases from a specific API URL.
    async fn get_releases_at(&self, repo: &RepoId, api_url: &str) -> Result<Vec<SourceRelease>>;
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
    fn test_source_kind_parse() {
        assert_eq!("github".parse::<SourceKind>().unwrap(), SourceKind::GitHub);
        assert_eq!("GitHub".parse::<SourceKind>().unwrap(), SourceKind::GitHub);
        assert_eq!("gitlab".parse::<SourceKind>().unwrap(), SourceKind::GitLab);
        assert_eq!("gitee".parse::<SourceKind>().unwrap(), SourceKind::Gitee);
        assert!("unknown".parse::<SourceKind>().is_err());
    }

    #[test]
    fn test_source_kind_display() {
        assert_eq!(SourceKind::GitHub.to_string(), "github");
        assert_eq!(SourceKind::GitLab.to_string(), "gitlab");
        assert_eq!(SourceKind::Gitee.to_string(), "gitee");
    }
}
