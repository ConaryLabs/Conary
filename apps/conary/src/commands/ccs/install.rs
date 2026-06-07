// src/commands/ccs/install.rs

//! CCS package installation
//!
//! Commands for installing CCS packages with signature verification,
//! dependency checking, and hook execution.

mod capability_policy;
mod command;
mod component_selection;
mod dependency;

#[cfg(test)]
mod command_capability_tests;
#[cfg(test)]
mod command_component_tests;
#[cfg(test)]
mod command_hook_tests;
#[cfg(test)]
mod command_metadata_tests;
#[cfg(test)]
mod command_payload_tests;
#[cfg(test)]
mod command_reinstall_tests;
#[cfg(test)]
mod test_support;

pub(crate) use capability_policy::enforce_ccs_capability_policy;
pub use command::{cmd_ccs_install, cmd_ccs_install_with_replay_options};
