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

/// Status of a symlink check.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkStatus {
    /// Link exists and points to the expected location
    Valid,
    /// Link doesn't exist yet (will be created)
    NotExists,
    /// Link exists but points to a different location
    WrongTarget,
    /// Path exists but is not a symlink
    NotSymlink,
    /// Cannot resolve the symlink target
    Unresolvable,
}

impl LinkStatus {
    /// Get a human-readable reason for this status.
    pub fn reason(&self) -> &'static str {
        match self {
            LinkStatus::Valid => "valid",
            LinkStatus::NotExists => "does not exist",
            LinkStatus::WrongTarget => "points to different location",
            LinkStatus::NotSymlink => "not a symlink",
            LinkStatus::Unresolvable => "cannot resolve target",
        }
    }

    /// Check if this status indicates a valid link.
    pub fn is_valid(&self) -> bool {
        matches!(self, LinkStatus::Valid)
    }

    /// Check if this status indicates the link will be created.
    pub fn is_creatable(&self) -> bool {
        matches!(self, LinkStatus::NotExists)
    }

    /// Check if this status indicates a problem.
    pub fn is_problematic(&self) -> bool {
        matches!(
            self,
            LinkStatus::WrongTarget | LinkStatus::NotSymlink | LinkStatus::Unresolvable
        )
    }
}

/// Result of a safe link removal operation.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoveLinkResult {
    /// Link was successfully removed
    Removed,
    /// Link doesn't exist (nothing to remove)
    NotExists,
    /// Path exists but is not a symlink
    NotSymlink,
    /// Link points to external path (not under expected prefix)
    ExternalTarget,
    /// Cannot resolve the symlink target
    Unresolvable,
}

/// A checked link with its status.
#[derive(Debug, Clone)]
pub struct CheckedLink {
    /// The destination path (symlink location)
    pub dest: PathBuf,
    /// The status of this link
    pub status: LinkStatus,
    /// Optional path within version directory
    pub path: Option<String>,
}

/// Result of validating a link for creation/update.
#[derive(Debug)]
pub enum LinkValidation {
    /// Link is valid and ready to be created/updated
    Valid {
        /// The link target (source file in version directory)
        target: PathBuf,
        /// The destination path (symlink location)
        dest: PathBuf,
        /// Whether the destination already exists and needs to be removed first
        needs_removal: bool,
    },
    /// Link should be skipped
    Skip { dest: PathBuf, reason: String },
    /// Link validation failed with an error
    Error { dest: PathBuf, error: String },
}
