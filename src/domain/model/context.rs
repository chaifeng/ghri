//! Package context for operating on installed packages.

use std::path::PathBuf;

use super::{Meta, ResolvedVersion};

/// Context for operating on an installed package.
///
/// This struct contains all the normalized/resolved information needed
/// to work with an installed package. It is created by `PackageRepository::load_context()`
/// which handles version normalization and path resolution.
///
/// Commands should use this context for all package operations instead of
/// passing raw user input around.
#[derive(Debug)]
pub struct PackageContext {
    /// Owner name (e.g., "chaifeng")
    pub owner: String,
    /// Repository name (e.g., "zidr")
    pub repo: String,
    /// Display name for the package (e.g., "chaifeng/zidr")
    pub display_name: String,
    /// The resolved version (normalized against releases)
    /// None only when operating on the entire package (e.g., remove without version)
    pub version: Option<ResolvedVersion>,
    /// Whether a specific version was requested by the user (vs using current)
    pub version_specified: bool,
    /// The package directory (e.g., ~/.ghri/chaifeng/zidr)
    pub package_dir: PathBuf,
    /// The version directory (e.g., ~/.ghri/chaifeng/zidr/v0.2.0)
    /// None when version is None
    pub version_dir: Option<PathBuf>,
    /// The loaded metadata
    pub meta: Meta,
}

impl PackageContext {
    /// Get the version, panics if None (use when version is required)
    pub fn version(&self) -> &ResolvedVersion {
        self.version
            .as_ref()
            .expect("version is required for this operation")
    }

    /// Get the version directory, panics if None
    pub fn version_dir(&self) -> &PathBuf {
        self.version_dir
            .as_ref()
            .expect("version_dir is required for this operation")
    }
}
