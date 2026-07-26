// apps/conary/src/commands/remove.rs
//! Package removal commands

mod autoremove;
mod ccs_hook;
mod command;
mod directory_ownership;
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
pub(crate) use directory_ownership::PackagePayloadOwnership;
pub(crate) use transaction::{commit_remove_db, prepare_remove_for_state_restore, snapshot_trove};
#[allow(unused_imports)]
pub(crate) use types::RemoveInnerResult;
pub(crate) use types::RemoveLifecycleOptions;
