use anyhow::{Result, anyhow};
use std::str::FromStr;

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

/// A repository specification that may include a version
/// Format: "owner/repo" or "owner/repo@version"
#[derive(Debug, PartialEq, Clone)]
pub struct RepoSpec {
    pub repo: GitHubRepo,
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

        let repo = repo_part.parse::<GitHubRepo>()?;
        Ok(RepoSpec { repo, version })
    }
}

/// A link specification that may include version and path
/// Format: "owner/repo", "owner/repo@version", "owner/repo:path", or "owner/repo@version:path"
#[derive(Debug, PartialEq, Clone)]
pub struct LinkSpec {
    pub repo: GitHubRepo,
    pub version: Option<String>,
    pub path: Option<String>,
}

impl std::fmt::Display for LinkSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.repo)?;
        if let Some(ref v) = self.version {
            write!(f, "@{}", v)?;
        }
        if let Some(ref p) = self.path {
            write!(f, ":{}", p)?;
        }
        Ok(())
    }
}

impl FromStr for LinkSpec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First split by : to get optional path (take the last : to allow paths with colons)
        let (repo_version_part, path) = if let Some(colon_pos) = s.rfind(':') {
            // Check if this colon is part of the repo/version or is the path separator
            // The path separator comes after @ or after repo name
            let before_colon = &s[..colon_pos];
            // If there's a / in before_colon, it's likely the path separator
            if before_colon.contains('/') {
                let path = &s[colon_pos + 1..];
                if path.is_empty() {
                    return Err(anyhow!("Invalid format: path after : cannot be empty."));
                }
                (before_colon, Some(path.to_string()))
            } else {
                (s, None)
            }
        } else {
            (s, None)
        };

        // Now parse the repo@version part
        let (repo_part, version) = if let Some(at_pos) = repo_version_part.rfind('@') {
            let (repo, ver) = repo_version_part.split_at(at_pos);
            let ver = &ver[1..]; // Skip the @
            if ver.is_empty() {
                return Err(anyhow!("Invalid format: version after @ cannot be empty."));
            }
            (repo, Some(ver.to_string()))
        } else {
            (repo_version_part, None)
        };

        let repo = repo_part.parse::<GitHubRepo>()?;
        Ok(LinkSpec {
            repo,
            version,
            path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_repo_valid() {
        // Test parsing valid "owner/repo" format

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
    fn test_parse_repo_spec_without_version() {
        // Test parsing RepoSpec without version: "owner/repo"

        let spec = RepoSpec::from_str("owner/repo").unwrap();

        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.version, None);
    }

    #[test]
    fn test_parse_repo_spec_with_version() {
        // Test parsing RepoSpec with version: "owner/repo@v1.0.0"

        let spec = RepoSpec::from_str("owner/repo@v1.0.0").unwrap();

        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.version, Some("v1.0.0".to_string()));
    }

    #[test]
    fn test_parse_repo_spec_with_version_no_v_prefix() {
        // Test parsing RepoSpec with version without 'v' prefix: "bach-sh/bach@0.7.2"

        let spec = RepoSpec::from_str("bach-sh/bach@0.7.2").unwrap();

        assert_eq!(spec.repo.owner, "bach-sh");
        assert_eq!(spec.repo.repo, "bach");
        assert_eq!(spec.version, Some("0.7.2".to_string()));
    }

    #[test]
    fn test_parse_repo_spec_empty_version_fails() {
        // Test that empty version after @ fails: "owner/repo@"

        let result = RepoSpec::from_str("owner/repo@");

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_parse_repo_spec_invalid_repo_fails() {
        // Test that invalid repo format fails: "invalid@v1.0.0" (missing slash)

        let result = RepoSpec::from_str("invalid@v1.0.0");

        assert!(result.is_err());
    }

    #[test]
    fn test_repo_spec_display_without_version() {
        // Test Display trait for RepoSpec without version

        let spec = RepoSpec {
            repo: GitHubRepo {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
            },
            version: None,
        };

        assert_eq!(format!("{}", spec), "owner/repo");
    }

    #[test]
    fn test_repo_spec_display_with_version() {
        // Test Display trait for RepoSpec with version

        let spec = RepoSpec {
            repo: GitHubRepo {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
            },
            version: Some("v1.0.0".to_string()),
        };

        assert_eq!(format!("{}", spec), "owner/repo@v1.0.0");
    }

    #[test]
    fn test_parse_link_spec_repo_only() {
        // Test parsing LinkSpec with repo only: "owner/repo"

        let spec = LinkSpec::from_str("owner/repo").unwrap();

        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.version, None);
        assert_eq!(spec.path, None);
    }

    #[test]
    fn test_parse_link_spec_with_version() {
        // Test parsing LinkSpec with version: "owner/repo@v1.0.0"

        let spec = LinkSpec::from_str("owner/repo@v1.0.0").unwrap();

        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.version, Some("v1.0.0".to_string()));
        assert_eq!(spec.path, None);
    }

    #[test]
    fn test_parse_link_spec_with_path() {
        // Test parsing LinkSpec with path: "owner/repo:bin/tool"

        let spec = LinkSpec::from_str("owner/repo:bin/tool").unwrap();

        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.version, None);
        assert_eq!(spec.path, Some("bin/tool".to_string()));
    }

    #[test]
    fn test_parse_link_spec_with_version_and_path() {
        // Test parsing LinkSpec with both version and path: "owner/repo@v1.0.0:bin/tool"

        let spec = LinkSpec::from_str("owner/repo@v1.0.0:bin/tool").unwrap();

        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.version, Some("v1.0.0".to_string()));
        assert_eq!(spec.path, Some("bin/tool".to_string()));
    }

    #[test]
    fn test_parse_link_spec_bach_example() {
        // Test parsing real-world example: "bach-sh/bach:bach.sh"

        let spec = LinkSpec::from_str("bach-sh/bach:bach.sh").unwrap();

        assert_eq!(spec.repo.owner, "bach-sh");
        assert_eq!(spec.repo.repo, "bach");
        assert_eq!(spec.version, None);
        assert_eq!(spec.path, Some("bach.sh".to_string()));
    }

    #[test]
    fn test_parse_link_spec_bach_with_version() {
        // Test parsing real-world example with version: "bach-sh/bach@0.7.0:bach.sh"

        let spec = LinkSpec::from_str("bach-sh/bach@0.7.0:bach.sh").unwrap();

        assert_eq!(spec.repo.owner, "bach-sh");
        assert_eq!(spec.repo.repo, "bach");
        assert_eq!(spec.version, Some("0.7.0".to_string()));
        assert_eq!(spec.path, Some("bach.sh".to_string()));
    }

    #[test]
    fn test_parse_link_spec_empty_path_fails() {
        // Test that empty path after : fails: "owner/repo:"

        let result = LinkSpec::from_str("owner/repo:");

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_link_spec_empty_version_fails() {
        // Test that empty version after @ fails: "owner/repo@:bin/tool"
        let result = LinkSpec::from_str("owner/repo@:path");

        assert!(result.is_err());
    }

    #[test]
    fn test_link_spec_display() {
        // Test Display trait for LinkSpec with all fields

        let spec = LinkSpec {
            repo: GitHubRepo {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
            },
            version: Some("v1.0.0".to_string()),
            path: Some("bin/tool".to_string()),
        };

        assert_eq!(format!("{}", spec), "owner/repo@v1.0.0:bin/tool");
    }

    #[test]
    fn test_link_spec_display_no_version() {
        // Test Display trait for LinkSpec without version

        let spec = LinkSpec {
            repo: GitHubRepo {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
            },
            version: None,
            path: Some("tool".to_string()),
        };

        assert_eq!(format!("{}", spec), "owner/repo:tool");
    }
}
