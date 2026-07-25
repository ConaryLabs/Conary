// src/commands/remove.rs
//! Package removal commands

mod autoremove;
mod ccs_hook;
mod command;
mod native_graph;
#[cfg(test)]
pub(super) mod test_support;
mod transaction;
mod types;

pub use autoremove::cmd_autoremove;
pub(crate) use ccs_hook::{
    execute_preflighted_ccs_remove_hook, load_ccs_remove_hook, preflight_ccs_remove_hook,
    preflight_loaded_ccs_remove_hook,
};
pub use command::cmd_remove;
pub(crate) use native_graph::execute_installed_trove_remove_graph;
#[allow(unused_imports)]
pub(crate) use types::RemoveInnerResult;
pub(crate) use types::RemoveLifecycleOptions;
