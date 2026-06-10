// src/commands/remove.rs
//! Package removal commands

mod autoremove;
mod command;
mod execution_path;
mod legacy_replay;
mod scriptlets;
#[cfg(test)]
pub(super) mod test_support;
mod transaction;
mod types;

pub use autoremove::cmd_autoremove;
pub use command::cmd_remove;
pub(crate) use transaction::remove_inner;
#[allow(unused_imports)]
pub(crate) use types::RemoveInnerResult;
pub(crate) use types::RemoveScriptletOptions;
