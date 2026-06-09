// apps/conary/src/dispatch/system_generation.rs

use std::borrow::Cow;

use anyhow::Result;

use super::context::require_live_mutation;
use crate::cli;
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};

pub(super) async fn dispatch_system_generation_command(
    gen_cmd: cli::GenerationCommands,
    allow_live_system_mutation: bool,
) -> Result<()> {
    match gen_cmd {
        cli::GenerationCommands::List => {
            commands::generation::commands::cmd_generation_list().await
        }
        cli::GenerationCommands::Build { summary, yes, db } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary system generation build"),
                LiveMutationClass::AlwaysLive,
                false,
            )?;
            commands::generation::commands::cmd_generation_build(&db.db_path, &summary)
        }
        cli::GenerationCommands::Publish { changeset, yes, db } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary system generation publish"),
                LiveMutationClass::AlwaysLive,
                false,
            )?;
            commands::generation::commands::cmd_generation_publish(&db.db_path, changeset)
        }
        cli::GenerationCommands::Pending { db } => {
            commands::generation::commands::cmd_generation_pending(&db.db_path)
        }
        cli::GenerationCommands::VerifyDbBackup {
            generation,
            current,
            db,
        } => commands::generation::commands::cmd_generation_verify_db_backup(
            &db.db_path,
            generation,
            current,
        ),
        cli::GenerationCommands::RecoverDb {
            generation,
            dry_run,
            keep_temp,
            yes,
            replace_healthy_db,
            db,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary system generation recover-db"),
                LiveMutationClass::AlwaysLive,
                dry_run,
            )?;
            commands::generation::commands::cmd_generation_recover_db(
                &db.db_path,
                generation,
                dry_run,
                keep_temp,
                yes,
                replace_healthy_db,
            )
        }
        cli::GenerationCommands::Export {
            generation,
            path,
            format,
            output,
            size,
        } => {
            commands::generation::export::cmd_generation_export(
                generation,
                path.as_deref(),
                &format,
                &output,
                size.as_deref(),
            )
            .await
        }
        cli::GenerationCommands::Switch {
            number,
            reboot,
            yes,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary system generation switch"),
                LiveMutationClass::AlwaysLive,
                false,
            )?;
            commands::generation::commands::cmd_generation_switch(number, reboot)
        }
        cli::GenerationCommands::Rollback { yes } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary system generation rollback"),
                LiveMutationClass::AlwaysLive,
                false,
            )?;
            commands::generation::commands::cmd_generation_rollback()
        }
        cli::GenerationCommands::Gc { keep, yes, db } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary system generation gc"),
                LiveMutationClass::AlwaysLive,
                false,
            )?;
            commands::generation::commands::cmd_generation_gc(keep, &db.db_path).await
        }
        cli::GenerationCommands::Info { number } => {
            commands::generation::commands::cmd_generation_info(number).await
        }
        cli::GenerationCommands::Recover { yes, db } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary system generation recover"),
                LiveMutationClass::AlwaysLive,
                false,
            )?;
            commands::generation::commands::cmd_generation_recover(&db.db_path)
        }
    }
}
