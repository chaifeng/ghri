//! Asset selection module
//!
//! This module provides abstractions for selecting which asset to download
//! from a GitHub release based on various criteria like platform, architecture,
//! or user preference.

mod picker;
mod platform;

pub use picker::{AssetPicker, DefaultAssetPicker};
pub use platform::{Platform, PlatformDetector};
