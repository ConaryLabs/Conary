// apps/conary/src/dispatch/system_trigger.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_system_trigger_command(
    trigger_cmd: cli::TriggerCommands,
) -> Result<()> {
    match trigger_cmd {
        cli::TriggerCommands::List { db, all, builtin } => {
            commands::cmd_trigger_list(&db.db_path, all, builtin).await
        }

        cli::TriggerCommands::Show { name, db } => {
            commands::cmd_trigger_show(&name, &db.db_path).await
        }

        cli::TriggerCommands::Enable { name, db } => {
            commands::cmd_trigger_enable(&name, &db.db_path).await
        }

        cli::TriggerCommands::Disable { name, db } => {
            commands::cmd_trigger_disable(&name, &db.db_path).await
        }

        cli::TriggerCommands::Add {
            name,
            pattern,
            handler,
            description,
            priority,
            db,
        } => {
            commands::cmd_trigger_add(
                &name,
                &pattern,
                &handler,
                description.as_deref(),
                priority,
                &db.db_path,
            )
            .await
        }

        cli::TriggerCommands::Remove { name, db } => {
            commands::cmd_trigger_remove(&name, &db.db_path).await
        }

        cli::TriggerCommands::Run {
            changeset_id,
            db,
            root,
        } => commands::cmd_trigger_run(changeset_id, &db.db_path, &root).await,
    }
}
