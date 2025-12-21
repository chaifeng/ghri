use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize};
use std::path::{Path, PathBuf};

use crate::github::{GitHubRepo, Release, ReleaseAsset, RepoInfo};
use crate::runtime::Runtime;

use super::LinkRule;

const DEFAULT_API_URL: &str = "https://api.github.com";

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
    pub releases: Vec<MetaRelease>,
    /// List of link rules for creating external symlinks
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkRule>,
    /// Legacy: Path where the current version is linked to (external symlink)
    /// Deprecated: Use `links` instead. Kept for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_to: Option<PathBuf>,
    /// Legacy: Relative path within version directory to link
    /// Deprecated: Use `links` instead. Kept for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_path: Option<String>,
}

impl Meta {
    pub fn from(
        repo: GitHubRepo,
        info: RepoInfo,
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
            license: info.license.map(|l| l.name),
            updated_at: info.updated_at,
            current_version: current.to_string(),
            releases: {
                let mut r: Vec<MetaRelease> = releases.into_iter().map(MetaRelease::from).collect();
                Meta::sort_releases_internal(&mut r);
                r
            },
            links: vec![],
            linked_to: None,
            linked_path: None,
        }
    }

    fn sort_releases_internal(releases: &mut [MetaRelease]) {
        releases.sort_by(|a, b| {
            match (&a.published_at, &b.published_at) {
                (Some(at_a), Some(at_b)) => at_b.cmp(at_a),  // Descending
                (Some(_), None) => std::cmp::Ordering::Less, // Published comes before unpublished
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => b.version.cmp(&a.version), // Version descending fallback
            }
        });
    }

    pub fn sort_releases(&mut self) {
        Self::sort_releases_internal(&mut self.releases);
    }

    pub fn get_latest_stable_release(&self) -> Option<&MetaRelease> {
        self.releases
            .iter()
            .filter(|r| !r.is_prerelease)
            .max_by(|a, b| {
                // Simplified version comparison: tag_name might not be semver-compliant,
                // but published_at is a good proxy for "latest".
                // If published_at is missing, fall back to version string comparison.
                match (&a.published_at, &b.published_at) {
                    (Some(at_a), Some(at_b)) => at_a.cmp(at_b),
                    _ => a.version.cmp(&b.version),
                }
            })
    }

    /// Load meta.json and apply default values for missing fields
    #[tracing::instrument(skip(runtime, path))]
    pub fn load<R: Runtime>(runtime: &R, path: &Path) -> Result<Self> {
        let content = runtime.read_to_string(path)?;
        let mut meta: Meta = serde_json::from_str(&content)?;
        
        // Apply semantic defaults for missing fields
        meta.apply_defaults(runtime, path);
        
        Ok(meta)
    }

    /// Check if a string is effectively empty (None, empty, or whitespace-only)
    fn is_empty_or_blank(s: &str) -> bool {
        s.trim().is_empty()
    }

    /// Check if an Option<String> is effectively empty
    fn is_option_empty_or_blank(s: &Option<String>) -> bool {
        match s {
            None => true,
            Some(s) => Self::is_empty_or_blank(s),
        }
    }

    /// Apply semantic default values for fields that are empty after deserialization
    fn apply_defaults<R: Runtime>(&mut self, runtime: &R, meta_path: &Path) {
        // Parse owner/repo from name
        let (owner, repo) = self.parse_owner_repo();

        // Default api_url to GitHub API
        if Self::is_empty_or_blank(&self.api_url) {
            self.api_url = DEFAULT_API_URL.to_string();
        }

        // Default repo_info_url based on api_url and name
        if Self::is_empty_or_blank(&self.repo_info_url) && !owner.is_empty() && !repo.is_empty() {
            self.repo_info_url = format!("{}/repos/{}/{}", self.api_url, owner, repo);
        }

        // Default releases_url based on api_url and name
        if Self::is_empty_or_blank(&self.releases_url) && !owner.is_empty() && !repo.is_empty() {
            self.releases_url = format!("{}/repos/{}/{}/releases", self.api_url, owner, repo);
        }

        // Default homepage to GitHub repo page (also handle empty string in Some)
        if Self::is_option_empty_or_blank(&self.homepage) && !owner.is_empty() && !repo.is_empty() {
            // Convert API URL to web URL
            let web_url = if self.api_url.contains("api.github.com") {
                "https://github.com".to_string()
            } else {
                // For GitHub Enterprise, try to derive web URL from API URL
                self.api_url
                    .replace("/api/v3", "")
                    .replace("api.", "")
            };
            self.homepage = Some(format!("{}/{}/{}", web_url, owner, repo));
        }

        // Default current_version by reading the 'current' symlink
        if Self::is_empty_or_blank(&self.current_version) {
            if let Some(parent) = meta_path.parent() {
                let current_link = parent.join("current");
                if let Ok(target) = runtime.read_link(&current_link) {
                    if let Some(version) = target.file_name().and_then(|s| s.to_str()) {
                        self.current_version = version.to_string();
                    }
                }
            }
        }

        // Migrate legacy linked_to/linked_path to links array
        if let Some(ref linked_to) = self.linked_to {
            // Check if this link already exists in links array
            let exists = self.links.iter().any(|l| l.dest == *linked_to);
            if !exists {
                self.links.push(LinkRule {
                    dest: linked_to.clone(),
                    path: self.linked_path.clone(),
                });
            }
            // Clear legacy fields after migration
            self.linked_to = None;
            self.linked_path = None;
        }
    }

    /// Parse owner and repo from the name field (format: "owner/repo")
    fn parse_owner_repo(&self) -> (String, String) {
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
            if let Some(existing) = self
                .releases
                .iter_mut()
                .find(|r| r.version == new_release.version)
            {
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

/// Release metadata
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct MetaRelease {
    pub version: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub is_prerelease: bool,
    #[serde(default)]
    pub tarball_url: String,
    #[serde(default)]
    pub assets: Vec<MetaAsset>,
}

impl From<Release> for MetaRelease {
    fn from(r: Release) -> Self {
        MetaRelease {
            version: r.tag_name,
            title: r.name,
            published_at: r.published_at,
            is_prerelease: r.prerelease,
            tarball_url: r.tarball_url,
            assets: r.assets.into_iter().map(MetaAsset::from).collect(),
        }
    }
}

impl From<MetaRelease> for Release {
    fn from(r: MetaRelease) -> Self {
        Release {
            tag_name: r.version,
            tarball_url: r.tarball_url,
            name: r.title,
            published_at: r.published_at,
            prerelease: r.is_prerelease,
            assets: r.assets.into_iter().map(ReleaseAsset::from).collect(),
        }
    }
}

/// Asset metadata
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MetaAsset {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub download_url: String,
}

impl From<ReleaseAsset> for MetaAsset {
    fn from(a: ReleaseAsset) -> Self {
        MetaAsset {
            name: a.name,
            size: a.size,
            download_url: a.browser_download_url,
        }
    }
}

impl From<MetaAsset> for ReleaseAsset {
    fn from(a: MetaAsset) -> Self {
        ReleaseAsset {
            name: a.name,
            size: a.size,
            browser_download_url: a.download_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::Release;
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;
    use std::path::PathBuf;

    #[test]
    fn test_meta_serialization_with_api_urls() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let info = RepoInfo {
            description: None,
            homepage: None,
            license: None,
            updated_at: "now".into(),
        };
        let api_url = "https://custom.api";
        let meta = Meta::from(repo, info, vec![], "v1", api_url);

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: Meta = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.api_url, api_url);
    }

    #[test]
    fn test_meta_releases_sorting() {
        let repo = GitHubRepo {
            owner: "o".into(),
            repo: "r".into(),
        };
        let info = RepoInfo {
            description: None,
            homepage: None,
            license: None,
            updated_at: "now".into(),
        };
        let releases = vec![
            Release {
                tag_name: "v1.0.0".into(),
                published_at: Some("2023-01-01T00:00:00Z".into()),
                ..Default::default()
            },
            Release {
                tag_name: "v2.0.0".into(),
                published_at: Some("2023-02-01T00:00:00Z".into()),
                ..Default::default()
            },
            Release {
                tag_name: "v0.9.0".into(),
                published_at: Some("2022-12-01T00:00:00Z".into()),
                ..Default::default()
            },
        ];
        let meta = Meta::from(repo, info, releases, "v2.0.0", "https://api");
        assert_eq!(meta.releases[0].version, "v2.0.0");
        assert_eq!(meta.releases[1].version, "v1.0.0");
        assert_eq!(meta.releases[2].version, "v0.9.0");
    }

    #[test]
    fn test_meta_merge_sorting() {
        let mut meta = Meta {
            name: "o/r".into(),
            api_url: "api".into(),
            updated_at: "t1".into(),
            current_version: "v1".into(),
            releases: vec![Release {
                tag_name: "v1".into(),
                published_at: Some("2023-01-01".into()),
                ..Default::default()
            }
            .into()],
            ..Default::default()
        };
        let other = Meta {
            name: "o/r".into(),
            api_url: "api".into(),
            updated_at: "t2".into(),
            current_version: "v1".into(),
            releases: vec![Release {
                tag_name: "v2".into(),
                published_at: Some("2023-02-01".into()),
                ..Default::default()
            }
            .into()],
            ..Default::default()
        };
        meta.merge(other);
        assert_eq!(meta.releases[0].version, "v2");
        assert_eq!(meta.releases[1].version, "v1");
    }

    #[test]
    fn test_meta_sorting_fallback() {
        let mut releases = vec![
            MetaRelease {
                version: "v1".into(),
                published_at: None,
                title: None,
                is_prerelease: false,
                tarball_url: "".into(),
                assets: vec![],
            },
            MetaRelease {
                version: "v2".into(),
                published_at: None,
                title: None,
                is_prerelease: false,
                tarball_url: "".into(),
                assets: vec![],
            },
            MetaRelease {
                version: "v1.5".into(),
                published_at: Some("2023".into()),
                title: None,
                is_prerelease: false,
                tarball_url: "".into(),
                assets: vec![],
            },
        ];
        Meta::sort_releases_internal(&mut releases);
        assert_eq!(releases[0].version, "v1.5");
        assert_eq!(releases[1].version, "v2");
        assert_eq!(releases[2].version, "v1");
    }

    #[test]
    fn test_meta_get_latest_stable_release() {
        let mut meta = Meta {
            name: "n".into(),
            ..Default::default()
        };
        meta.releases.push(MetaRelease {
            version: "v1".into(),
            is_prerelease: false,
            published_at: Some("2023".into()),
            ..Default::default()
        });
        meta.releases.push(MetaRelease {
            version: "v2-rc".into(),
            is_prerelease: true,
            published_at: Some("2024".into()),
            ..Default::default()
        });

        let latest = meta.get_latest_stable_release().unwrap();
        assert_eq!(latest.version, "v1");
    }

    #[test]
    fn test_meta_get_latest_stable_release_empty() {
        let meta = Meta {
            name: "n".into(),
            ..Default::default()
        };
        assert!(meta.get_latest_stable_release().is_none());
    }

    #[test]
    fn test_meta_get_latest_stable_release_only_prerelease() {
        let mut meta = Meta {
            name: "n".into(),
            ..Default::default()
        };
        meta.releases.push(MetaRelease {
            version: "v1-rc".into(),
            is_prerelease: true,
            ..Default::default()
        });
        assert!(meta.get_latest_stable_release().is_none());
    }

    #[test]
    fn test_meta_conversions() {
        let meta_asset = MetaAsset {
            name: "n".into(),
            size: 1,
            download_url: "u".into(),
        };
        let asset: ReleaseAsset = meta_asset.clone().into();
        assert_eq!(asset.name, meta_asset.name);

        let meta_release = MetaRelease {
            version: "v1".into(),
            title: Some("t".into()),
            published_at: Some("d".into()),
            is_prerelease: false,
            tarball_url: "u".into(),
            assets: vec![meta_asset],
        };
        let release: Release = meta_release.clone().into();
        assert_eq!(release.tag_name, meta_release.version);
    }

    #[test]
    fn test_update_timestamp_behavior() {
        let mut meta = Meta {
            name: "o/r".into(),
            description: Some("old".into()),
            updated_at: "old".into(),
            ..Default::default()
        };
        let other = Meta {
            name: "o/r".into(),
            description: Some("new".into()),
            updated_at: "new".into(),
            ..Default::default()
        };

        assert!(meta.merge(other));
        assert_eq!(meta.description, Some("new".into()));
    }

    #[test]
    fn test_meta_load() {
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/test/owner/repo/meta.json");

        runtime
            .expect_read_to_string()
            .with(eq(path.clone()))
            .returning(|_| {
                Ok(r#"{
                    "name": "o/r",
                    "api_url": "https://api.example.com",
                    "repo_info_url": "url",
                    "releases_url": "url",
                    "description": null,
                    "homepage": "https://example.com",
                    "license": null,
                    "updated_at": "now",
                    "current_version": "v1",
                    "releases": []
                }"#
                .into())
            });

        // No symlink read needed since current_version is provided
        let meta = Meta::load(&runtime, &path).unwrap();
        assert_eq!(meta.name, "o/r");
        assert_eq!(meta.current_version, "v1");
        assert_eq!(meta.api_url, "https://api.example.com");
        assert_eq!(meta.homepage, Some("https://example.com".into()));
    }

    #[test]
    fn test_meta_load_minimal_json_backward_compat() {
        // Test loading a minimal meta.json that might have been created by an older version
        // Only the required field "name" is present
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/test/owner/repo/meta.json");

        runtime
            .expect_read_to_string()
            .with(eq(path.clone()))
            .returning(|_| {
                Ok(r#"{"name": "owner/repo"}"#.into())
            });

        // Mock read_link for current symlink (will fail, so current_version stays empty)
        runtime
            .expect_read_link()
            .returning(|_| Err(anyhow::anyhow!("not found")));

        let meta = Meta::load(&runtime, &path).unwrap();
        assert_eq!(meta.name, "owner/repo");
        // api_url should default to GitHub API
        assert_eq!(meta.api_url, "https://api.github.com");
        // URLs should be derived from name and api_url
        assert_eq!(meta.repo_info_url, "https://api.github.com/repos/owner/repo");
        assert_eq!(meta.releases_url, "https://api.github.com/repos/owner/repo/releases");
        // homepage should default to GitHub page
        assert_eq!(meta.homepage, Some("https://github.com/owner/repo".into()));
        assert_eq!(meta.description, None);
        assert_eq!(meta.license, None);
        assert_eq!(meta.updated_at, "");
        // current_version stays empty since symlink read failed
        assert_eq!(meta.current_version, "");
        assert!(meta.releases.is_empty());
    }

    #[test]
    fn test_meta_load_partial_fields_backward_compat() {
        // Test loading meta.json with some fields missing (simulating older format)
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/test/owner/repo/meta.json");

        runtime
            .expect_read_to_string()
            .with(eq(path.clone()))
            .returning(|_| {
                Ok(r#"{
                    "name": "owner/repo",
                    "current_version": "v1.0.0",
                    "releases": [
                        {"version": "v1.0.0"}
                    ]
                }"#.into())
            });

        // No symlink read needed since current_version is provided

        let meta = Meta::load(&runtime, &path).unwrap();
        assert_eq!(meta.name, "owner/repo");
        assert_eq!(meta.current_version, "v1.0.0");
        // api_url should default to GitHub API
        assert_eq!(meta.api_url, "https://api.github.com");
        assert!(meta.releases.len() == 1);
        // Release with minimal fields
        let release = &meta.releases[0];
        assert_eq!(release.version, "v1.0.0");
        assert_eq!(release.tarball_url, "");
        assert!(!release.is_prerelease);
        assert!(release.assets.is_empty());
    }

    #[test]
    fn test_meta_release_minimal_backward_compat() {
        // Test deserializing a release with only version field
        let json = r#"{"version": "v2.0.0"}"#;
        let release: MetaRelease = serde_json::from_str(json).unwrap();
        
        assert_eq!(release.version, "v2.0.0");
        assert_eq!(release.title, None);
        assert_eq!(release.published_at, None);
        assert!(!release.is_prerelease);
        assert_eq!(release.tarball_url, "");
        assert!(release.assets.is_empty());
    }

    #[test]
    fn test_meta_asset_minimal_backward_compat() {
        // Test deserializing an asset with only name field
        let json = r#"{"name": "app-linux-x64.tar.gz"}"#;
        let asset: MetaAsset = serde_json::from_str(json).unwrap();
        
        assert_eq!(asset.name, "app-linux-x64.tar.gz");
        assert_eq!(asset.size, 0);
        assert_eq!(asset.download_url, "");
    }

    #[test]
    fn test_meta_load_with_unknown_fields_forward_compat() {
        // Test that unknown fields are ignored (forward compatibility)
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/test/owner/repo/meta.json");

        runtime
            .expect_read_to_string()
            .with(eq(path.clone()))
            .returning(|_| {
                Ok(r#"{
                    "name": "owner/repo",
                    "current_version": "v1.0.0",
                    "some_future_field": "some_value",
                    "another_new_field": 12345,
                    "releases": []
                }"#.into())
            });

        // Should not fail even with unknown fields
        let meta = Meta::load(&runtime, &path).unwrap();
        assert_eq!(meta.name, "owner/repo");
        assert_eq!(meta.current_version, "v1.0.0");
    }

    #[test]
    fn test_meta_load_current_version_from_symlink() {
        // Test that current_version is read from symlink when missing in JSON
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/root/owner/repo/meta.json");

        runtime
            .expect_read_to_string()
            .with(eq(path.clone()))
            .returning(|_| {
                Ok(r#"{"name": "owner/repo"}"#.into())
            });

        // Mock read_link to return the version from symlink
        runtime
            .expect_read_link()
            .with(eq(PathBuf::from("/root/owner/repo/current")))
            .returning(|_| Ok(PathBuf::from("v2.0.0")));

        let meta = Meta::load(&runtime, &path).unwrap();
        assert_eq!(meta.name, "owner/repo");
        // current_version should be read from symlink
        assert_eq!(meta.current_version, "v2.0.0");
    }

    #[test]
    fn test_meta_load_homepage_default_for_github() {
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/root/owner/repo/meta.json");

        runtime
            .expect_read_to_string()
            .returning(|_| {
                Ok(r#"{"name": "test-owner/test-repo", "current_version": "v1"}"#.into())
            });

        let meta = Meta::load(&runtime, &path).unwrap();
        // Homepage should default to GitHub URL
        assert_eq!(meta.homepage, Some("https://github.com/test-owner/test-repo".into()));
    }

    #[test]
    fn test_meta_load_preserves_explicit_homepage() {
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/root/owner/repo/meta.json");

        runtime
            .expect_read_to_string()
            .returning(|_| {
                Ok(r#"{
                    "name": "owner/repo",
                    "homepage": "https://custom-homepage.com",
                    "current_version": "v1"
                }"#.into())
            });

        let meta = Meta::load(&runtime, &path).unwrap();
        // Explicit homepage should be preserved
        assert_eq!(meta.homepage, Some("https://custom-homepage.com".into()));
    }

    #[test]
    fn test_meta_parse_owner_repo() {
        let meta = Meta {
            name: "owner/repo".into(),
            ..Default::default()
        };

        let (owner, repo) = meta.parse_owner_repo();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_meta_parse_owner_repo_invalid() {
        let meta = Meta {
            name: "invalid-name".into(),
            ..Default::default()
        };

        let (owner, repo) = meta.parse_owner_repo();
        assert_eq!(owner, "");
        assert_eq!(repo, "");
    }

    #[test]
    fn test_meta_load_with_null_values() {
        // Test that null values in JSON are handled properly
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/root/owner/repo/meta.json");

        runtime
            .expect_read_to_string()
            .returning(|_| {
                Ok(r#"{
                    "name": "owner/repo",
                    "api_url": null,
                    "repo_info_url": null,
                    "releases_url": null,
                    "homepage": null,
                    "current_version": "v1"
                }"#.into())
            });

        let meta = Meta::load(&runtime, &path).unwrap();
        assert_eq!(meta.name, "owner/repo");
        // Null should be treated as missing, defaults applied
        assert_eq!(meta.api_url, "https://api.github.com");
        assert_eq!(meta.repo_info_url, "https://api.github.com/repos/owner/repo");
        assert_eq!(meta.releases_url, "https://api.github.com/repos/owner/repo/releases");
        assert_eq!(meta.homepage, Some("https://github.com/owner/repo".into()));
    }

    #[test]
    fn test_meta_load_with_empty_strings() {
        // Test that empty strings are treated as missing
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/root/owner/repo/meta.json");

        runtime
            .expect_read_to_string()
            .returning(|_| {
                Ok(r#"{
                    "name": "owner/repo",
                    "api_url": "",
                    "repo_info_url": "",
                    "releases_url": "",
                    "homepage": "",
                    "current_version": "v1"
                }"#.into())
            });

        let meta = Meta::load(&runtime, &path).unwrap();
        // Empty strings should be treated as missing, defaults applied
        assert_eq!(meta.api_url, "https://api.github.com");
        assert_eq!(meta.repo_info_url, "https://api.github.com/repos/owner/repo");
        assert_eq!(meta.releases_url, "https://api.github.com/repos/owner/repo/releases");
        assert_eq!(meta.homepage, Some("https://github.com/owner/repo".into()));
    }

    #[test]
    fn test_meta_load_with_whitespace_strings() {
        // Test that whitespace-only strings are treated as missing
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/root/owner/repo/meta.json");

        runtime
            .expect_read_to_string()
            .returning(|_| {
                Ok(r#"{
                    "name": "owner/repo",
                    "api_url": "   ",
                    "repo_info_url": "  \t  ",
                    "releases_url": "\n",
                    "homepage": "   ",
                    "current_version": "v1"
                }"#.into())
            });

        let meta = Meta::load(&runtime, &path).unwrap();
        // Whitespace-only strings should be treated as missing, defaults applied
        assert_eq!(meta.api_url, "https://api.github.com");
        assert_eq!(meta.repo_info_url, "https://api.github.com/repos/owner/repo");
        assert_eq!(meta.releases_url, "https://api.github.com/repos/owner/repo/releases");
        assert_eq!(meta.homepage, Some("https://github.com/owner/repo".into()));
    }

    #[test]
    fn test_meta_load_current_version_whitespace_reads_symlink() {
        // Test that whitespace-only current_version triggers symlink read
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/root/owner/repo/meta.json");

        runtime
            .expect_read_to_string()
            .returning(|_| {
                Ok(r#"{
                    "name": "owner/repo",
                    "current_version": "   "
                }"#.into())
            });

        runtime
            .expect_read_link()
            .with(eq(PathBuf::from("/root/owner/repo/current")))
            .returning(|_| Ok(PathBuf::from("v3.0.0")));

        let meta = Meta::load(&runtime, &path).unwrap();
        // Whitespace current_version should trigger symlink read
        assert_eq!(meta.current_version, "v3.0.0");
    }

    #[test]
    fn test_is_empty_or_blank() {
        assert!(Meta::is_empty_or_blank(""));
        assert!(Meta::is_empty_or_blank("   "));
        assert!(Meta::is_empty_or_blank("\t\n"));
        assert!(!Meta::is_empty_or_blank("value"));
        assert!(!Meta::is_empty_or_blank("  value  "));
    }

    #[test]
    fn test_is_option_empty_or_blank() {
        assert!(Meta::is_option_empty_or_blank(&None));
        assert!(Meta::is_option_empty_or_blank(&Some("".into())));
        assert!(Meta::is_option_empty_or_blank(&Some("   ".into())));
        assert!(!Meta::is_option_empty_or_blank(&Some("value".into())));
    }
}
