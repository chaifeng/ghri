//! Application layer - Actions that coordinate domain services.
//!
//! This layer contains the application-specific business rules and orchestrates
//! the flow of data between the CLI layer and domain services.

mod install;
mod link;
mod list;
mod prune;
mod remove;
mod show;
mod update;
mod upgrade;

pub use install::{InstallAction, InstallOperations};
pub use link::{LinkAction, LinkResult, UnlinkResult};
pub use list::{ListAction, PackageInfo};
pub use prune::{PruneAction, PruneInfo};
pub use remove::RemoveAction;
pub use show::{PackageDetails, ShowAction};
pub use update::{UpdateAction, UpdateResult};
pub use upgrade::{UpdateCheck, UpgradeAction, UpgradeCandidate, UpgradeCheckResult};

// Re-export options from commands layer
pub use crate::commands::{InstallOptions, UpgradeOptions};

#[cfg(test)]
pub use install::MockInstallOperations;
