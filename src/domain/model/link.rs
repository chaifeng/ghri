use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Rule for creating a symbolic link
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LinkRule {
    /// Destination path for the symlink (where the link will be created)
    /// Can be absolute or relative to the package directory
    pub dest: PathBuf,
    /// Source path within the version directory (what the link points to)
    /// If None, defaults to the version directory itself or the single executable in it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A versioned link that was created for a specific version
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VersionedLink {
    /// Destination path for the symlink
    pub dest: PathBuf,
    /// The version this link points to
    pub version: String,
    /// Source path within the version directory
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}
