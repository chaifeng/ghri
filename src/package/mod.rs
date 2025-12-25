//! Package management module
//!
//! This module provides abstractions for managing installed packages,
//! including metadata storage, discovery, and version tracking.

mod discovery;
mod link;
mod link_rule;
mod meta;
mod repository;
mod version;

pub use discovery::find_all_packages;
pub use link::{CheckedLink, LinkManager, LinkStatus, LinkValidation, RemoveLinkResult};
pub use link_rule::{LinkRule, VersionedLink};
pub use meta::Meta;
pub use repository::PackageRepository;
pub use version::{VersionConstraint, VersionResolver};
