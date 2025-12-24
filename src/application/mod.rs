//! Application layer - Use cases that coordinate domain services.
//!
//! This layer contains the application-specific business rules and orchestrates
//! the flow of data between the CLI layer and domain services.

mod install;
mod upgrade;

pub use install::{InstallOperations, InstallOptions, InstallUseCase};
pub use upgrade::{UpdateCheck, UpgradeOptions, UpgradeUseCase};

#[cfg(test)]
pub use install::MockInstallOperations;
