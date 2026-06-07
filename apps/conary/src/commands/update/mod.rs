// src/commands/update/mod.rs
//! Update command module routing.

mod adopted_authority;
mod collection;
mod delta_stats;
mod package;
mod pinning;
mod selection;
mod source_policy;

pub use collection::cmd_update_group;
pub use delta_stats::cmd_delta_stats;
pub use package::cmd_update;
pub use pinning::{cmd_list_pinned, cmd_pin, cmd_unpin};
