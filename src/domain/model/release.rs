use serde::{Deserialize, Serialize};

/// A downloadable asset from a release.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ReleaseAsset {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub download_url: String,
}

/// A release from the provider.
///
/// This type is used both for API responses and for local metadata storage.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Release {
    /// Version tag (e.g., "v1.0.0")
    pub tag: String,
    /// Release name/title
    #[serde(default)]
    pub name: Option<String>,
    /// Publication date (ISO 8601)
    #[serde(default)]
    pub published_at: Option<String>,
    /// Whether this is a pre-release
    #[serde(default)]
    pub prerelease: bool,
    /// URL to download the source tarball
    #[serde(default)]
    pub tarball_url: String,
    /// Downloadable assets
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}
