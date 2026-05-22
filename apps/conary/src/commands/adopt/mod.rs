// src/commands/adopt/mod.rs

//! Commands for adopting existing system packages into Conary tracking
//!
//! This module provides the ability to import packages already installed
//! by the system package manager (RPM, dpkg, pacman) into Conary's tracking database.

pub(crate) mod cas_capture;
mod conflicts;
mod convert;
mod hooks;
mod native_handoff;
mod packages;
mod refresh;
mod status;
mod system;
mod unadopt;

// Re-export all public commands
pub use conflicts::cmd_conflicts;
pub use convert::cmd_adopt_convert;
pub use hooks::cmd_sync_hook_install;
pub use native_handoff::{
    NativeHandoffOptions, NativeHandoffOutcome, NativeHandoffSummary, cmd_native_handoff,
};
pub use packages::cmd_adopt;
pub use refresh::cmd_adopt_refresh;
pub use status::cmd_adopt_status;
pub use system::FileInfoTuple;
pub use system::cmd_adopt_system;
pub use unadopt::{UnadoptOptions, cmd_unadopt};
