//! Repository specification parsing for install command.

use anyhow::{Result, anyhow};
use std::str::FromStr;

use crate::provider::RepoId;

/// A repository specification that may include a version
/// Format: "owner/repo" or "owner/repo@version"
#[derive(Debug, PartialEq, Clone)]
pub struct RepoSpec {
    pub repo: RepoId,
    pub version: Option<String>,
}

impl std::fmt::Display for RepoSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.version {
            Some(v) => write!(f, "{}@{}", self.repo, v),
            None => write!(f, "{}", self.repo),
        }
    }
}

impl FromStr for RepoSpec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Split by @ to get optional version
        let (repo_part, version) = if let Some(at_pos) = s.rfind('@') {
            let (repo, ver) = s.split_at(at_pos);
            let ver = &ver[1..]; // Skip the @
            if ver.is_empty() {
                return Err(anyhow!(
                    "Invalid format: version after @ cannot be empty. Expected 'owner/repo@version'."
                ));
            }
            (repo, Some(ver.to_string()))
        } else {
            (s, None)
        };

        let repo = repo_part.parse::<RepoId>()?;
        Ok(RepoSpec { repo, version })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_repo_spec_without_version() {
        let spec = RepoSpec::from_str("owner/repo").unwrap();
        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.version, None);
    }

    #[test]
    fn test_parse_repo_spec_with_version() {
        let spec = RepoSpec::from_str("owner/repo@v1.0.0").unwrap();
        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.version, Some("v1.0.0".to_string()));
    }

    #[test]
    fn test_parse_repo_spec_with_version_no_v_prefix() {
        let spec = RepoSpec::from_str("bach-sh/bach@0.7.2").unwrap();
        assert_eq!(spec.repo.owner, "bach-sh");
        assert_eq!(spec.repo.repo, "bach");
        assert_eq!(spec.version, Some("0.7.2".to_string()));
    }

    #[test]
    fn test_parse_repo_spec_empty_version_fails() {
        let result = RepoSpec::from_str("owner/repo@");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_parse_repo_spec_invalid_repo_fails() {
        let result = RepoSpec::from_str("invalid@v1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_repo_spec_display_without_version() {
        let spec = RepoSpec {
            repo: RepoId {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
            },
            version: None,
        };
        assert_eq!(format!("{}", spec), "owner/repo");
    }

    #[test]
    fn test_repo_spec_display_with_version() {
        let spec = RepoSpec {
            repo: RepoId {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
            },
            version: Some("v1.0.0".to_string()),
        };
        assert_eq!(format!("{}", spec), "owner/repo@v1.0.0");
    }
}
