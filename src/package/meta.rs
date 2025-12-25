use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize};
use std::path::{Path, PathBuf};

use crate::provider::{Release, ReleaseAsset, RepoId, RepoMetadata};
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
    /// List of link rules for creating external symlinks (updated on install/update)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkRule>,
    /// List of versioned links for historical version links (not updated on install/update)
    /// These are links created with explicit version specifiers (e.g., owner/repo@v1.0.0)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versioned_links: Vec<super::link_rule::VersionedLink>,
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
                let mut r: Vec<MetaRelease> = releases.into_iter().map(MetaRelease::from).collect();
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

    /// Get the latest release including pre-releases
    pub fn get_latest_release(&self) -> Option<&MetaRelease> {
        self.releases
            .iter()
            .max_by(|a, b| match (&a.published_at, &b.published_at) {
                (Some(at_a), Some(at_b)) => at_a.cmp(at_b),
                _ => a.version.cmp(&b.version),
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
                self.api_url.replace("/api/v3", "").replace("api.", "")
            };
            self.homepage = Some(format!("{}/{}/{}", web_url, owner, repo));
        }

        // Default current_version by reading the 'current' symlink
        if Self::is_empty_or_blank(&self.current_version)
            && let Some(parent) = meta_path.parent()
        {
            let current_link = parent.join("current");
            if let Ok(target) = runtime.read_link(&current_link)
                && let Some(version) = target.file_name().and_then(|s| s.to_str())
            {
                self.current_version = version.to_string();
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
            version: r.tag,
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
            tag: r.version,
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
            download_url: a.download_url,
        }
    }
}

impl From<MetaAsset> for ReleaseAsset {
    fn from(a: MetaAsset) -> Self {
        ReleaseAsset {
            name: a.name,
            size: a.size,
            download_url: a.download_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;
    use std::path::PathBuf;

    #[test]
    fn test_meta_serialization_with_api_urls() {
        // Test that Meta serializes and deserializes with custom API URL preserved

        // --- Setup ---
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let info = RepoMetadata {
            description: None,
            homepage: None,
            license: None,
            updated_at: Some("now".into()),
        };
        let api_url = "https://custom.api";

        // --- Execute ---

        // Create Meta with custom API URL
        let meta = Meta::from(repo, info, vec![], "v1", api_url);

        // Serialize to JSON and deserialize back
        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: Meta = serde_json::from_str(&json).unwrap();

        // --- Verify ---

        // Custom API URL should be preserved through serialization roundtrip
        assert_eq!(deserialized.api_url, api_url);
    }

    #[test]
    fn test_meta_releases_sorting() {
        // Test that releases are sorted by published_at date in descending order (newest first)

        // --- Setup ---
        let repo = RepoId {
            owner: "o".into(),
            repo: "r".into(),
        };
        let info = RepoMetadata {
            description: None,
            homepage: None,
            license: None,
            updated_at: Some("now".into()),
        };

        // Releases in random order with different published dates
        let releases = vec![
            Release {
                tag: "v1.0.0".into(),
                published_at: Some("2023-01-01T00:00:00Z".into()), // Oldest
                ..Default::default()
            },
            Release {
                tag: "v2.0.0".into(),
                published_at: Some("2023-02-01T00:00:00Z".into()), // Newest
                ..Default::default()
            },
            Release {
                tag: "v0.9.0".into(),
                published_at: Some("2022-12-01T00:00:00Z".into()), // Middle
                ..Default::default()
            },
        ];

        // --- Execute ---

        let meta = Meta::from(repo, info, releases, "v2.0.0", "https://api");

        // --- Verify ---

        // Releases should be sorted by published_at descending (newest first)
        assert_eq!(meta.releases[0].version, "v2.0.0"); // 2023-02-01 (newest)
        assert_eq!(meta.releases[1].version, "v1.0.0"); // 2023-01-01
        assert_eq!(meta.releases[2].version, "v0.9.0"); // 2022-12-01 (oldest)
    }

    #[test]
    fn test_meta_merge_sorting() {
        // Test that merge() adds new releases and re-sorts by published_at

        // --- Setup ---

        // Existing meta with v1 release (older)
        let mut meta = Meta {
            name: "o/r".into(),
            api_url: "api".into(),
            updated_at: "t1".into(),
            current_version: "v1".into(),
            releases: vec![
                Release {
                    tag: "v1".into(),
                    published_at: Some("2023-01-01".into()), // Older
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };

        // New meta with v2 release (newer)
        let other = Meta {
            name: "o/r".into(),
            api_url: "api".into(),
            updated_at: "t2".into(),
            current_version: "v1".into(),
            releases: vec![
                Release {
                    tag: "v2".into(),
                    published_at: Some("2023-02-01".into()), // Newer
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };

        // --- Execute ---

        meta.merge(other);

        // --- Verify ---

        // After merge, releases should be sorted (newest first)
        assert_eq!(meta.releases[0].version, "v2"); // 2023-02-01 (newest)
        assert_eq!(meta.releases[1].version, "v1"); // 2023-01-01 (oldest)
    }

    #[test]
    fn test_meta_sorting_fallback() {
        // Test sorting behavior when some releases have no published_at date
        // Releases with published_at should come first, then fallback to version string comparison

        // --- Setup ---

        let mut releases = vec![
            MetaRelease {
                version: "v1".into(),
                published_at: None, // No date - will be sorted by version
                title: None,
                is_prerelease: false,
                tarball_url: "".into(),
                assets: vec![],
            },
            MetaRelease {
                version: "v2".into(),
                published_at: None, // No date - will be sorted by version
                title: None,
                is_prerelease: false,
                tarball_url: "".into(),
                assets: vec![],
            },
            MetaRelease {
                version: "v1.5".into(),
                published_at: Some("2023".into()), // Has date - comes first
                title: None,
                is_prerelease: false,
                tarball_url: "".into(),
                assets: vec![],
            },
        ];

        // --- Execute ---

        Meta::sort_releases_internal(&mut releases);

        // --- Verify ---

        // v1.5 has published_at, so it comes first
        assert_eq!(releases[0].version, "v1.5");
        // v1 and v2 have no published_at, sorted by version descending
        assert_eq!(releases[1].version, "v2");
        assert_eq!(releases[2].version, "v1");
    }

    #[test]
    fn test_meta_get_latest_stable_release() {
        // Test that get_latest_stable_release() returns the latest non-prerelease version

        // --- Setup ---

        let mut meta = Meta {
            name: "n".into(),
            ..Default::default()
        };

        // Add stable release v1 (older)
        meta.releases.push(MetaRelease {
            version: "v1".into(),
            is_prerelease: false,
            published_at: Some("2023".into()),
            ..Default::default()
        });

        // Add prerelease v2-rc (newer, but prerelease)
        meta.releases.push(MetaRelease {
            version: "v2-rc".into(),
            is_prerelease: true, // This is a prerelease!
            published_at: Some("2024".into()),
            ..Default::default()
        });

        // --- Execute ---

        let latest = meta.get_latest_stable_release().unwrap();

        // --- Verify ---

        // Should return v1 (the only stable release), not v2-rc (prerelease)
        assert_eq!(latest.version, "v1");
    }

    #[test]
    fn test_meta_get_latest_stable_release_empty() {
        // Test that get_latest_stable_release() returns None when no releases exist

        // --- Setup ---

        let meta = Meta {
            name: "n".into(),
            releases: vec![], // No releases
            ..Default::default()
        };

        // --- Execute & Verify ---

        // Should return None when there are no releases
        assert!(meta.get_latest_stable_release().is_none());
    }

    #[test]
    fn test_meta_get_latest_stable_release_only_prerelease() {
        // Test that get_latest_stable_release() returns None when only prereleases exist

        // --- Setup ---

        let mut meta = Meta {
            name: "n".into(),
            ..Default::default()
        };

        // Add only a prerelease version
        meta.releases.push(MetaRelease {
            version: "v1-rc".into(),
            is_prerelease: true, // Only prerelease available
            ..Default::default()
        });

        // --- Execute & Verify ---

        // Should return None when all releases are prereleases
        assert!(meta.get_latest_stable_release().is_none());
    }

    #[test]
    fn test_meta_get_latest_release_includes_prerelease() {
        // Test that get_latest_release() returns the latest release including prereleases

        // --- Setup ---

        let mut meta = Meta {
            name: "n".into(),
            ..Default::default()
        };

        // Add stable release v1 (older)
        meta.releases.push(MetaRelease {
            version: "v1".into(),
            is_prerelease: false,
            published_at: Some("2023".into()),
            ..Default::default()
        });

        // Add prerelease v2-rc (newer)
        meta.releases.push(MetaRelease {
            version: "v2-rc".into(),
            is_prerelease: true,
            published_at: Some("2024".into()),
            ..Default::default()
        });

        // --- Execute ---

        let latest = meta.get_latest_release().unwrap();

        // --- Verify ---

        // Should return v2-rc (the latest including prereleases)
        assert_eq!(latest.version, "v2-rc");
    }

    #[test]
    fn test_meta_get_latest_release_only_prerelease() {
        // Test that get_latest_release() returns prerelease when only prereleases exist

        // --- Setup ---

        let mut meta = Meta {
            name: "n".into(),
            ..Default::default()
        };

        // Add only a prerelease version
        meta.releases.push(MetaRelease {
            version: "v1-rc".into(),
            is_prerelease: true,
            ..Default::default()
        });

        // --- Execute & Verify ---

        // Should return v1-rc (the only release)
        let latest = meta.get_latest_release().unwrap();
        assert_eq!(latest.version, "v1-rc");
    }

    #[test]
    fn test_meta_conversions() {
        // Test conversion between Meta types and Source types (MetaAsset <-> ReleaseAsset, MetaRelease <-> Release)

        // --- Test MetaAsset -> ReleaseAsset ---

        let meta_asset = MetaAsset {
            name: "app.tar.gz".into(),
            size: 1024,
            download_url: "https://example.com/app.tar.gz".into(),
        };

        let asset: ReleaseAsset = meta_asset.clone().into();

        // Verify conversion preserves all fields
        assert_eq!(asset.name, "app.tar.gz");
        assert_eq!(asset.size, 1024);
        assert_eq!(asset.download_url, "https://example.com/app.tar.gz");

        // --- Test MetaRelease -> Release ---

        let meta_release = MetaRelease {
            version: "v1.0.0".into(),
            title: Some("Release 1.0.0".into()),
            published_at: Some("2023-01-01".into()),
            is_prerelease: false,
            tarball_url: "https://example.com/tarball".into(),
            assets: vec![meta_asset],
        };

        let release: Release = meta_release.clone().into();

        // Verify conversion preserves all fields
        assert_eq!(release.tag, "v1.0.0");
        assert_eq!(release.name, Some("Release 1.0.0".into()));
        assert_eq!(release.published_at, Some("2023-01-01".into()));
        assert!(!release.prerelease);
        assert_eq!(release.tarball_url, "https://example.com/tarball");
        assert_eq!(release.assets.len(), 1);
    }

    #[test]
    fn test_update_timestamp_behavior() {
        // Test that merge() updates description when it changes

        // --- Setup ---

        let mut meta = Meta {
            name: "o/r".into(),
            description: Some("old description".into()),
            updated_at: "old".into(),
            ..Default::default()
        };

        let other = Meta {
            name: "o/r".into(),
            description: Some("new description".into()), // Changed description
            updated_at: "new".into(),
            ..Default::default()
        };

        // --- Execute ---

        let changed = meta.merge(other);

        // --- Verify ---

        // merge() should return true and update description
        assert!(changed);
        assert_eq!(meta.description, Some("new description".into()));
    }

    #[test]
    fn test_meta_load() {
        // Test loading a complete meta.json file with all fields populated

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/test/owner/repo/meta.json");

        // --- Read meta.json ---

        // Read /test/owner/repo/meta.json -> complete JSON with all fields
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
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

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify ---

        assert_eq!(meta.name, "o/r");
        assert_eq!(meta.current_version, "v1");
        assert_eq!(meta.api_url, "https://api.example.com");
        assert_eq!(meta.homepage, Some("https://example.com".into()));
    }

    #[test]
    fn test_meta_load_minimal_json_backward_compat() {
        // Test backward compatibility: loading minimal meta.json with only "name" field
        // Should apply default values for missing fields

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/test/owner/repo/meta.json");
        let current_link = PathBuf::from("/test/owner/repo/current");

        // --- Read meta.json ---

        // Read /test/owner/repo/meta.json -> minimal JSON with only "name"
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(|_| Ok(r#"{"name": "owner/repo"}"#.into()));

        // --- Try to Read Current Symlink (for current_version default) ---

        // Read symlink /test/owner/repo/current -> fails (not found)
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Err(anyhow::anyhow!("not found")));

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify Defaults Applied ---

        assert_eq!(meta.name, "owner/repo");
        // api_url should default to GitHub API
        assert_eq!(meta.api_url, "https://api.github.com");
        // URLs should be derived from name and api_url
        assert_eq!(
            meta.repo_info_url,
            "https://api.github.com/repos/owner/repo"
        );
        assert_eq!(
            meta.releases_url,
            "https://api.github.com/repos/owner/repo/releases"
        );
        // homepage should default to GitHub page
        assert_eq!(meta.homepage, Some("https://github.com/owner/repo".into()));
        // Optional fields stay empty
        assert_eq!(meta.description, None);
        assert_eq!(meta.license, None);
        assert_eq!(meta.updated_at, "");
        // current_version stays empty since symlink read failed
        assert_eq!(meta.current_version, "");
        assert!(meta.releases.is_empty());
    }

    #[test]
    fn test_meta_load_partial_fields_backward_compat() {
        // Test backward compatibility: loading meta.json with some fields missing
        // Missing fields should get default values

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/test/owner/repo/meta.json");

        // --- Read meta.json ---

        // Read /test/owner/repo/meta.json -> partial JSON (no api_url, repo_info_url, etc.)
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(|_| {
                Ok(r#"{
                    "name": "owner/repo",
                    "current_version": "v1.0.0",
                    "releases": [
                        {"version": "v1.0.0"}
                    ]
                }"#
                .into())
            });

        // No symlink read needed since current_version is provided

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify ---

        assert_eq!(meta.name, "owner/repo");
        assert_eq!(meta.current_version, "v1.0.0");
        // api_url should default to GitHub API
        assert_eq!(meta.api_url, "https://api.github.com");
        // Should have 1 release with minimal fields defaulted
        assert!(meta.releases.len() == 1);
        let release = &meta.releases[0];
        assert_eq!(release.version, "v1.0.0");
        assert_eq!(release.tarball_url, ""); // Defaulted to empty
        assert!(!release.is_prerelease); // Defaulted to false
        assert!(release.assets.is_empty()); // Defaulted to empty
    }

    #[test]
    fn test_meta_release_minimal_backward_compat() {
        // Test backward compatibility: deserializing a release with only version field
        // Missing fields should get default values

        // --- Setup ---

        let json = r#"{"version": "v2.0.0"}"#;

        // --- Execute ---

        let release: MetaRelease = serde_json::from_str(json).unwrap();

        // --- Verify Defaults Applied ---

        assert_eq!(release.version, "v2.0.0");
        assert_eq!(release.title, None); // Default: None
        assert_eq!(release.published_at, None); // Default: None
        assert!(!release.is_prerelease); // Default: false
        assert_eq!(release.tarball_url, ""); // Default: empty string
        assert!(release.assets.is_empty()); // Default: empty vec
    }

    #[test]
    fn test_meta_asset_minimal_backward_compat() {
        // Test backward compatibility: deserializing an asset with only name field
        // Missing fields should get default values

        // --- Setup ---

        let json = r#"{"name": "app-linux-x64.tar.gz"}"#;

        // --- Execute ---

        let asset: MetaAsset = serde_json::from_str(json).unwrap();

        // --- Verify Defaults Applied ---

        assert_eq!(asset.name, "app-linux-x64.tar.gz");
        assert_eq!(asset.size, 0); // Default: 0
        assert_eq!(asset.download_url, ""); // Default: empty string
    }

    #[test]
    fn test_meta_load_with_unknown_fields_forward_compat() {
        // Test forward compatibility: loading meta.json with unknown fields from future versions
        // Unknown fields should be ignored without error

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/test/owner/repo/meta.json");

        // --- Read meta.json ---

        // Read /test/owner/repo/meta.json -> JSON with unknown future fields
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(|_| {
                Ok(r#"{
                    "name": "owner/repo",
                    "current_version": "v1.0.0",
                    "some_future_field": "some_value",
                    "another_new_field": 12345,
                    "releases": []
                }"#
                .into())
            });

        // --- Execute ---

        // Should not fail even with unknown fields
        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify ---

        assert_eq!(meta.name, "owner/repo");
        assert_eq!(meta.current_version, "v1.0.0");
    }

    #[test]
    fn test_meta_load_current_version_from_symlink() {
        // Test that current_version is derived from symlink when missing in JSON

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/root/owner/repo/meta.json");
        let current_link = PathBuf::from("/root/owner/repo/current");

        // --- Read meta.json ---

        // Read /root/owner/repo/meta.json -> no current_version field
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(|_| Ok(r#"{"name": "owner/repo"}"#.into()));

        // --- Read Current Symlink ---

        // Read symlink /root/owner/repo/current -> "v2.0.0"
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v2.0.0")));

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify ---

        assert_eq!(meta.name, "owner/repo");
        // current_version should be derived from symlink target
        assert_eq!(meta.current_version, "v2.0.0");
    }

    #[test]
    fn test_meta_load_homepage_default_for_github() {
        // Test that homepage defaults to GitHub URL when missing

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/root/owner/repo/meta.json");

        // --- Read meta.json ---

        // Read meta.json -> no homepage field
        runtime.expect_read_to_string().returning(|_| {
            Ok(r#"{"name": "test-owner/test-repo", "current_version": "v1"}"#.into())
        });

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify ---

        // Homepage should default to GitHub URL based on name
        assert_eq!(
            meta.homepage,
            Some("https://github.com/test-owner/test-repo".into())
        );
    }

    #[test]
    fn test_meta_load_preserves_explicit_homepage() {
        // Test that explicit homepage value is preserved (not overwritten by default)

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/root/owner/repo/meta.json");

        // --- Read meta.json ---

        // Read meta.json -> has explicit homepage
        runtime.expect_read_to_string().returning(|_| {
            Ok(r#"{
                    "name": "owner/repo",
                    "homepage": "https://custom-homepage.com",
                    "current_version": "v1"
                }"#
            .into())
        });

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify ---

        // Explicit homepage should be preserved, not overwritten with GitHub default
        assert_eq!(meta.homepage, Some("https://custom-homepage.com".into()));
    }

    #[test]
    fn test_meta_parse_owner_repo() {
        // Test parsing valid "owner/repo" name format

        // --- Setup ---

        let meta = Meta {
            name: "owner/repo".into(),
            ..Default::default()
        };

        // --- Execute ---

        let (owner, repo) = meta.parse_owner_repo();

        // --- Verify ---

        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_meta_parse_owner_repo_invalid() {
        // Test parsing invalid name format (missing slash)

        // --- Setup ---

        let meta = Meta {
            name: "invalid-name".into(), // No "/" separator
            ..Default::default()
        };

        // --- Execute ---

        let (owner, repo) = meta.parse_owner_repo();

        // --- Verify ---

        // Should return empty strings for invalid format
        assert_eq!(owner, "");
        assert_eq!(repo, "");
    }

    #[test]
    fn test_meta_load_with_null_values() {
        // Test that null values in JSON are treated as missing and defaults applied

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/root/owner/repo/meta.json");

        // --- Read meta.json ---

        // Read meta.json -> fields with explicit null values
        runtime.expect_read_to_string().returning(|_| {
            Ok(r#"{
                    "name": "owner/repo",
                    "api_url": null,
                    "repo_info_url": null,
                    "releases_url": null,
                    "homepage": null,
                    "current_version": "v1"
                }"#
            .into())
        });

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify Defaults Applied ---

        assert_eq!(meta.name, "owner/repo");
        // Null should be treated as missing, defaults applied
        assert_eq!(meta.api_url, "https://api.github.com");
        assert_eq!(
            meta.repo_info_url,
            "https://api.github.com/repos/owner/repo"
        );
        assert_eq!(
            meta.releases_url,
            "https://api.github.com/repos/owner/repo/releases"
        );
        assert_eq!(meta.homepage, Some("https://github.com/owner/repo".into()));
    }

    #[test]
    fn test_meta_load_with_empty_strings() {
        // Test that empty strings are treated as missing and defaults applied

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/root/owner/repo/meta.json");

        // --- Read meta.json ---

        // Read meta.json -> fields with empty string values
        runtime.expect_read_to_string().returning(|_| {
            Ok(r#"{
                    "name": "owner/repo",
                    "api_url": "",
                    "repo_info_url": "",
                    "releases_url": "",
                    "homepage": "",
                    "current_version": "v1"
                }"#
            .into())
        });

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify Defaults Applied ---

        // Empty strings should be treated as missing, defaults applied
        assert_eq!(meta.api_url, "https://api.github.com");
        assert_eq!(
            meta.repo_info_url,
            "https://api.github.com/repos/owner/repo"
        );
        assert_eq!(
            meta.releases_url,
            "https://api.github.com/repos/owner/repo/releases"
        );
        assert_eq!(meta.homepage, Some("https://github.com/owner/repo".into()));
    }

    #[test]
    fn test_meta_load_with_whitespace_strings() {
        // Test that whitespace-only strings are treated as missing and defaults applied

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/root/owner/repo/meta.json");

        // --- Read meta.json ---

        // Read meta.json -> fields with whitespace-only values
        runtime.expect_read_to_string().returning(|_| {
            Ok(r#"{
                    "name": "owner/repo",
                    "api_url": "   ",
                    "repo_info_url": "  \t  ",
                    "releases_url": "\n",
                    "homepage": "   ",
                    "current_version": "v1"
                }"#
            .into())
        });

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify Defaults Applied ---

        // Whitespace-only strings should be treated as missing, defaults applied
        assert_eq!(meta.api_url, "https://api.github.com");
        assert_eq!(
            meta.repo_info_url,
            "https://api.github.com/repos/owner/repo"
        );
        assert_eq!(
            meta.releases_url,
            "https://api.github.com/repos/owner/repo/releases"
        );
        assert_eq!(meta.homepage, Some("https://github.com/owner/repo".into()));
    }

    #[test]
    fn test_meta_load_current_version_whitespace_reads_symlink() {
        // Test that whitespace-only current_version triggers symlink read

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/root/owner/repo/meta.json");
        let current_link = PathBuf::from("/root/owner/repo/current");

        // --- Read meta.json ---

        // Read /root/owner/repo/meta.json -> current_version is whitespace
        runtime.expect_read_to_string().returning(|_| {
            Ok(r#"{
                    "name": "owner/repo",
                    "current_version": "   "
                }"#
            .into())
        });

        // --- Read Current Symlink ---

        // Whitespace current_version triggers symlink read
        // Read symlink /root/owner/repo/current -> "v3.0.0"
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v3.0.0")));

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify ---

        // current_version should be derived from symlink (whitespace triggered read)
        assert_eq!(meta.current_version, "v3.0.0");
    }

    #[test]
    fn test_is_empty_or_blank() {
        // Test the is_empty_or_blank() helper function

        // --- Empty/Blank Cases (should return true) ---
        assert!(Meta::is_empty_or_blank("")); // Empty string
        assert!(Meta::is_empty_or_blank("   ")); // Spaces only
        assert!(Meta::is_empty_or_blank("\t\n")); // Tab and newline

        // --- Non-Empty Cases (should return false) ---
        assert!(!Meta::is_empty_or_blank("value")); // Normal value
        assert!(!Meta::is_empty_or_blank("  value  ")); // Value with surrounding whitespace
    }

    #[test]
    fn test_is_option_empty_or_blank() {
        // Test the is_option_empty_or_blank() helper function

        // --- Empty/Blank Cases (should return true) ---
        assert!(Meta::is_option_empty_or_blank(&None)); // None
        assert!(Meta::is_option_empty_or_blank(&Some("".into()))); // Some("")
        assert!(Meta::is_option_empty_or_blank(&Some("   ".into()))); // Some("   ")

        // --- Non-Empty Cases (should return false) ---
        assert!(!Meta::is_option_empty_or_blank(&Some("value".into()))); // Some("value")
    }

    #[test]
    fn test_meta_filters_serialization() {
        // Test that filters are correctly serialized and deserialized

        // --- Setup ---

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1.0.0".into(),
            filters: vec!["*aarch64*".into(), "*macos*".into()],
            ..Default::default()
        };

        // --- Execute ---

        // Serialize to JSON
        let json = serde_json::to_string(&meta).unwrap();

        // Deserialize back
        let deserialized: Meta = serde_json::from_str(&json).unwrap();

        // --- Verify ---

        // Filters should be preserved through serialization roundtrip
        assert_eq!(deserialized.filters, vec!["*aarch64*", "*macos*"]);
    }

    #[test]
    fn test_meta_filters_empty_not_serialized() {
        // Test that empty filters array is not included in JSON output (skip_serializing_if)

        // --- Setup ---

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1.0.0".into(),
            filters: vec![], // Empty filters
            ..Default::default()
        };

        // --- Execute ---

        let json = serde_json::to_string(&meta).unwrap();

        // --- Verify ---

        // Empty filters should not appear in JSON output
        assert!(!json.contains("filters"));
    }

    #[test]
    fn test_meta_load_without_filters_backward_compat() {
        // Test backward compatibility: loading meta.json without filters field
        // Should default to empty filters

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/test/owner/repo/meta.json");

        // --- Read meta.json ---

        // Read /test/owner/repo/meta.json -> no filters field (old format)
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(|_| {
                Ok(r#"{
                    "name": "owner/repo",
                    "current_version": "v1.0.0"
                }"#
                .into())
            });

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify ---

        // Filters should default to empty when not in JSON
        assert!(meta.filters.is_empty());
    }

    #[test]
    fn test_meta_load_with_filters() {
        // Test loading meta.json with filters field

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/test/owner/repo/meta.json");

        // --- Read meta.json ---

        // Read /test/owner/repo/meta.json -> has filters field
        runtime
            .expect_read_to_string()
            .with(eq(meta_path.clone()))
            .returning(|_| {
                Ok(r#"{
                    "name": "owner/repo",
                    "current_version": "v1.0.0",
                    "filters": ["*linux*", "*x86_64*"]
                }"#
                .into())
            });

        // --- Execute ---

        let meta = Meta::load(&runtime, &meta_path).unwrap();

        // --- Verify ---

        // Filters should be loaded from JSON
        assert_eq!(meta.filters, vec!["*linux*", "*x86_64*"]);
    }

    #[test]
    fn test_meta_from_does_not_include_filters() {
        // Test that Meta::from() creates meta with empty filters
        // Filters should only be set during install/update

        // --- Setup ---

        let repo = RepoId {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let info = RepoMetadata {
            description: None,
            homepage: None,
            license: None,
            updated_at: Some("now".into()),
        };

        // --- Execute ---

        let meta = Meta::from(repo, info, vec![], "v1", "https://api.github.com");

        // --- Verify ---

        // Meta::from() should create meta with empty filters
        assert!(meta.filters.is_empty());
    }
}
