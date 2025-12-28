use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize};
use std::path::PathBuf;

use crate::domain::model::Release;
use crate::provider::{RepoId, RepoMetadata};

/// Deserialize a string that may be null as empty string
fn deserialize_nullable_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// Package metadata stored locally for installed packages
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Meta {
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub api_url: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub repo_info_url: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub releases_url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub updated_at: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub current_version: String,
    #[serde(default)]
    pub releases: Vec<Release>,
    /// List of link rules for creating external symlinks (updated on install/update)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<crate::domain::model::link::LinkRule>,
    /// List of versioned links for historical version links (not updated on install/update)
    /// These are links created with explicit version specifiers (e.g., owner/repo@v1.0.0)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versioned_links: Vec<crate::domain::model::link::VersionedLink>,
    /// Legacy: Path where the current version is linked to (external symlink)
    /// Deprecated: Use `links` instead. Kept for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_to: Option<PathBuf>,
    /// Legacy: Relative path within version directory to link
    /// Deprecated: Use `links` instead. Kept for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_path: Option<String>,
    /// Asset filter patterns used during install/update
    /// These patterns are saved and reused when updating the package
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<String>,
}

impl Meta {
    pub fn from(
        repo: RepoId,
        info: RepoMetadata,
        releases: Vec<Release>,
        current: &str,
        api_url: &str,
    ) -> Self {
        Meta {
            name: format!("{}/{}", repo.owner, repo.repo),
            api_url: api_url.to_string(),
            repo_info_url: format!("{}/repos/{}/{}", api_url, repo.owner, repo.repo),
            releases_url: format!("{}/repos/{}/{}/releases", api_url, repo.owner, repo.repo),
            description: info.description,
            homepage: info.homepage,
            license: info.license,
            updated_at: info.updated_at.unwrap_or_default(),
            current_version: current.to_string(),
            releases: {
                let mut r: Vec<Release> = releases;
                Meta::sort_releases_internal(&mut r);
                r
            },
            links: vec![],
            versioned_links: vec![],
            linked_to: None,
            linked_path: None,
            filters: vec![],
        }
    }

    pub(crate) fn sort_releases_internal(releases: &mut [Release]) {
        releases.sort_by(|a, b| {
            match (&a.published_at, &b.published_at) {
                (Some(at_a), Some(at_b)) => at_b.cmp(at_a),  // Descending
                (Some(_), None) => std::cmp::Ordering::Less, // Published comes before unpublished
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => b.tag.cmp(&a.tag), // Version descending fallback
            }
        });
    }

    pub fn sort_releases(&mut self) {
        Self::sort_releases_internal(&mut self.releases);
    }

    pub fn get_latest_stable_release(&self) -> Option<&Release> {
        self.releases
            .iter()
            .filter(|r| !r.prerelease)
            .max_by(|a, b| {
                // Simplified version comparison: tag_name might not be semver-compliant,
                // but published_at is a good proxy for "latest".
                // If published_at is missing, fall back to version string comparison.
                match (&a.published_at, &b.published_at) {
                    (Some(at_a), Some(at_b)) => at_a.cmp(at_b),
                    _ => a.tag.cmp(&b.tag),
                }
            })
    }

    /// Get the latest release including pre-releases
    pub fn get_latest_release(&self) -> Option<&Release> {
        self.releases
            .iter()
            .max_by(|a, b| match (&a.published_at, &b.published_at) {
                (Some(at_a), Some(at_b)) => at_a.cmp(at_b),
                _ => a.tag.cmp(&b.tag),
            })
    }

    /// Check if a string is effectively empty (None, empty, or whitespace-only)
    pub fn is_empty_or_blank(s: &str) -> bool {
        s.trim().is_empty()
    }

    /// Check if an Option<String> is effectively empty
    pub fn is_option_empty_or_blank(s: &Option<String>) -> bool {
        match s {
            None => true,
            Some(s) => Self::is_empty_or_blank(s),
        }
    }

    /// Parse owner and repo from the name field (format: "owner/repo")
    pub fn parse_owner_repo(&self) -> (String, String) {
        let parts: Vec<&str> = self.name.splitn(2, '/').collect();
        if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            (String::new(), String::new())
        }
    }

    pub fn merge(&mut self, other: Meta) -> bool {
        let mut changed = false;

        if self.description != other.description {
            self.description = other.description;
            changed = true;
        }
        if self.homepage != other.homepage {
            self.homepage = other.homepage;
            changed = true;
        }
        if self.license != other.license {
            self.license = other.license;
            changed = true;
        }

        for new_release in other.releases {
            if let Some(existing) = self.releases.iter_mut().find(|r| r.tag == new_release.tag) {
                if existing != &new_release {
                    *existing = new_release;
                    changed = true;
                }
            } else {
                self.releases.push(new_release);
                changed = true;
            }
        }

        if changed {
            self.sort_releases();
        }

        changed
    }
}
