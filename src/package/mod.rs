//! Package management module
//!
//! This module provides abstractions for managing installed packages,
//! including metadata storage, discovery, and version tracking.

mod discovery;
mod link_rule;
mod meta;

pub use discovery::find_all_packages;
pub use link_rule::LinkRule;
pub use meta::{Meta, MetaAsset, MetaRelease};
