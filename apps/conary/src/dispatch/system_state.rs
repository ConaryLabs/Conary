// apps/conary/src/dispatch/system_state.rs

use std::borrow::Cow;

use anyhow::Result;

use super::context::require_live_mutation;
use crate::cli;
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};

pub(super) async fn dispatch_system_state_command(
    state_cmd: cli::StateCommands,
    allow_live_system_mutation: bool,
) -> Result<()> {
    match state_cmd {
        cli::StateCommands::List { db, limit } => {
            commands::cmd_state_list(&db.db_path, limit).await
        }

        cli::StateCommands::Show { state_number, db } => {
            commands::cmd_state_show(&db.db_path, state_number).await
        }

        cli::StateCommands::Diff {
            from_state,
            to_state,
            db,
        } => commands::cmd_state_diff(&db.db_path, from_state, to_state).await,

        cli::StateCommands::Revert {
            state_number,
            db,
            dry_run,
            yes,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary system state revert"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                dry_run,
            )?;
            commands::cmd_state_restore(&db.db_path, state_number, dry_run).await
        }

        cli::StateCommands::Prune { keep, db, dry_run } => {
            commands::cmd_state_prune(&db.db_path, keep, dry_run).await
        }

        cli::StateCommands::Create {
            summary,
            description,
            db,
        } => commands::cmd_state_create(&db.db_path, &summary, description.as_deref()).await,

        cli::StateCommands::Rollback {
            changeset_id,
            common,
            yes,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary system state rollback"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                false,
            )?;
            commands::cmd_rollback(changeset_id, &common.db.db_path, &common.root).await
        }
    }
}
