//! Application layer - Actions that coordinate domain services.
//!
//! This layer contains the application-specific business rules and orchestrates
//! the flow of data between the CLI layer and domain services.

mod install;
mod upgrade;

pub use install::{InstallAction, InstallOperations};
pub use upgrade::{UpdateCheck, UpgradeAction};

// Re-export options from commands layer
pub use crate::commands::{InstallOptions, UpgradeOptions};

#[cfg(test)]
pub use install::MockInstallOperations;
