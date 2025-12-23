//! Platform detection and asset selection module
//!
//! This module provides abstractions for detecting the current platform
//! (OS and architecture) and selecting which release asset to download
//! based on platform, architecture, or user preference.

mod detection;
mod picker;

pub use detection::{Platform, PlatformDetector};
pub use picker::{AssetPicker, DefaultAssetPicker};
