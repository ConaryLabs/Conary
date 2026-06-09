// apps/conary/src/dispatch/config.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_config_command(config_cmd: cli::ConfigCommands) -> Result<()> {
    match config_cmd {
        cli::ConfigCommands::List { package, db, all } => {
            commands::cmd_config_list(&db.db_path, package.as_deref(), all).await
        }

        cli::ConfigCommands::Diff { path, common } => {
            commands::cmd_config_diff(&common.db.db_path, &path, &common.root).await
        }

        cli::ConfigCommands::Backup { path, common } => {
            commands::cmd_config_backup(&common.db.db_path, &path, &common.root).await
        }

        cli::ConfigCommands::Restore {
            path,
            common,
            backup_id,
        } => commands::cmd_config_restore(&common.db.db_path, &path, &common.root, backup_id).await,

        cli::ConfigCommands::Check { package, common } => {
            commands::cmd_config_check(&common.db.db_path, &common.root, package.as_deref()).await
        }

        cli::ConfigCommands::Backups { path, db } => {
            commands::cmd_config_backups(&db.db_path, &path).await
        }
    }
}
