// src/commands/ccs/install.rs

//! CCS package installation
//!
//! Commands for installing CCS packages with signature verification,
//! dependency checking, and hook execution.

mod capability_declaration;
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

pub(crate) use capability_declaration::validate_ccs_capability_declaration;
pub use command::cmd_ccs_install;
