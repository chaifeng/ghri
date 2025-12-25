//! Application layer - Actions that coordinate domain services.
//!
//! This layer contains the application-specific business rules and orchestrates
//! the flow of data between the CLI layer and domain services.

mod install;
mod upgrade;

pub use install::{InstallAction, InstallOperations, InstallOptions};
pub use upgrade::{UpdateCheck, UpgradeAction, UpgradeOptions};

#[cfg(test)]
pub use install::MockInstallOperations;
