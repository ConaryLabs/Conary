// src/commands/adopt/mod.rs

//! Commands for adopting existing system packages into Conary tracking
//!
//! This module provides the ability to import packages already installed
//! by the system package manager (RPM, dpkg, pacman) into Conary's tracking database.

mod conflicts;
mod packages;
mod status;
mod system;

// Re-export all public commands
pub use conflicts::cmd_conflicts;
pub use packages::cmd_adopt;
pub use status::cmd_adopt_status;
pub use system::cmd_adopt_system;
