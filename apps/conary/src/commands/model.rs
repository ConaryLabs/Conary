// src/commands/model.rs

//! System Model Commands
//!
//! Commands for declarative system state management using model files.

mod apply;
mod check;
mod context;
mod diff;
mod lock;
mod presentation;
mod publish;
mod remote_diff;
mod snapshot;
#[cfg(test)]
mod test_support;

pub use apply::{ApplyOptions, cmd_model_apply};
pub use check::cmd_model_check;
pub use diff::cmd_model_diff;
pub use lock::{cmd_model_lock, cmd_model_update};
pub use publish::cmd_model_publish;
pub use remote_diff::cmd_model_remote_diff;
pub use snapshot::cmd_model_snapshot;
