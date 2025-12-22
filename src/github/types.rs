use serde::{Deserialize, Serialize};

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
