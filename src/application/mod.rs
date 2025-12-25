//! Application layer - Actions that coordinate domain services.
//!
//! This layer contains the application-specific business rules and orchestrates
//! the flow of data between the CLI layer and domain services.

mod install;
mod list;
mod show;
mod upgrade;

pub use install::{InstallAction, InstallOperations};
pub use list::{ListAction, PackageInfo};
pub use show::{PackageDetails, ShowAction};
pub use upgrade::{UpdateCheck, UpgradeAction};

// Re-export options from commands layer
pub use crate::commands::{InstallOptions, UpgradeOptions};

#[cfg(test)]
pub use install::MockInstallOperations;
