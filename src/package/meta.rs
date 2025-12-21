use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::github::{GitHubRepo, Release, ReleaseAsset, RepoInfo};
use crate::runtime::Runtime;

/// Package metadata stored locally for installed packages
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Meta {
    pub name: String,
    pub api_url: String,
    pub repo_info_url: String,
    pub releases_url: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub updated_at: String,
    pub current_version: String,
    pub releases: Vec<MetaRelease>,
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

    #[tracing::instrument(skip(runtime, path))]
    pub fn load<R: Runtime>(runtime: &R, path: &Path) -> Result<Self> {
        let content = runtime.read_to_string(path)?;
        let meta: Meta = serde_json::from_str(&content)?;
        Ok(meta)
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
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MetaRelease {
    pub version: String,
    pub title: Option<String>,
    pub published_at: Option<String>,
    pub is_prerelease: bool,
    pub tarball_url: String,
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
    pub size: u64,
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
            repo_info_url: "url".into(),
            releases_url: "url".into(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "t1".into(),
            current_version: "v1".into(),
            releases: vec![Release {
                tag_name: "v1".into(),
                published_at: Some("2023-01-01".into()),
                ..Default::default()
            }
            .into()],
        };
        let other = Meta {
            name: "o/r".into(),
            api_url: "api".into(),
            repo_info_url: "url".into(),
            releases_url: "url".into(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "t2".into(),
            current_version: "v1".into(),
            releases: vec![Release {
                tag_name: "v2".into(),
                published_at: Some("2023-02-01".into()),
                ..Default::default()
            }
            .into()],
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
            api_url: "".into(),
            repo_info_url: "".into(),
            releases_url: "".into(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "".into(),
            current_version: "".into(),
            releases: vec![],
        };
        meta.releases.push(MetaRelease {
            version: "v1".into(),
            is_prerelease: false,
            published_at: Some("2023".into()),
            title: None,
            tarball_url: "".into(),
            assets: vec![],
        });
        meta.releases.push(MetaRelease {
            version: "v2-rc".into(),
            is_prerelease: true,
            published_at: Some("2024".into()),
            title: None,
            tarball_url: "".into(),
            assets: vec![],
        });

        let latest = meta.get_latest_stable_release().unwrap();
        assert_eq!(latest.version, "v1");
    }

    #[test]
    fn test_meta_get_latest_stable_release_empty() {
        let meta = Meta {
            name: "n".into(),
            api_url: "".into(),
            repo_info_url: "".into(),
            releases_url: "".into(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "".into(),
            current_version: "".into(),
            releases: vec![],
        };
        assert!(meta.get_latest_stable_release().is_none());
    }

    #[test]
    fn test_meta_get_latest_stable_release_only_prerelease() {
        let mut meta = Meta {
            name: "n".into(),
            api_url: "".into(),
            repo_info_url: "".into(),
            releases_url: "".into(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "".into(),
            current_version: "".into(),
            releases: vec![],
        };
        meta.releases.push(MetaRelease {
            version: "v1-rc".into(),
            is_prerelease: true,
            published_at: None,
            title: None,
            tarball_url: "".into(),
            assets: vec![],
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
            api_url: "".into(),
            repo_info_url: "".into(),
            releases_url: "".into(),
            description: Some("old".into()),
            homepage: None,
            license: None,
            updated_at: "old".into(),
            current_version: "".into(),
            releases: vec![],
        };
        let other = Meta {
            name: "o/r".into(),
            api_url: "".into(),
            repo_info_url: "".into(),
            releases_url: "".into(),
            description: Some("new".into()),
            homepage: None,
            license: None,
            updated_at: "new".into(),
            current_version: "".into(),
            releases: vec![],
        };

        assert!(meta.merge(other));
        assert_eq!(meta.description, Some("new".into()));
    }

    #[test]
    fn test_meta_load() {
        let mut runtime = MockRuntime::new();
        let path = PathBuf::from("/test/meta.json");

        runtime
            .expect_read_to_string()
            .with(eq(path.clone()))
            .returning(|_| {
                Ok(r#"{
                    "name": "o/r",
                    "api_url": "api",
                    "repo_info_url": "url",
                    "releases_url": "url",
                    "description": null,
                    "homepage": null,
                    "license": null,
                    "updated_at": "now",
                    "current_version": "v1",
                    "releases": []
                }"#
                .into())
            });

        let meta = Meta::load(&runtime, &path).unwrap();
        assert_eq!(meta.name, "o/r");
        assert_eq!(meta.current_version, "v1");
    }
}
