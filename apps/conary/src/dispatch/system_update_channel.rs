// apps/conary/src/dispatch/system_update_channel.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) fn dispatch_system_update_channel_command(
    action: cli::UpdateChannelAction,
) -> Result<()> {
    match action {
        cli::UpdateChannelAction::Get { db } => commands::cmd_update_channel_get(&db.db_path),
        cli::UpdateChannelAction::Set { url, db } => {
            commands::cmd_update_channel_set(&db.db_path, &url)
        }
        cli::UpdateChannelAction::Reset { db } => commands::cmd_update_channel_reset(&db.db_path),
    }
}
