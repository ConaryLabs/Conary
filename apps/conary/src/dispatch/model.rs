// apps/conary/src/dispatch/model.rs

use std::borrow::Cow;

use anyhow::Result;

use super::context::require_live_mutation;
use crate::cli;
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};

pub(super) async fn dispatch_model_command(
    model_cmd: cli::ModelCommands,
    allow_live_system_mutation: bool,
) -> Result<()> {
    match model_cmd {
        cli::ModelCommands::Diff { model, offline, db } => {
            commands::cmd_model_diff(&model, &db.db_path, offline).await
        }

        cli::ModelCommands::Apply {
            model,
            common,
            dry_run,
            yes,
            skip_optional,
            strict,
            no_autoremove,
            offline,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary model apply"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                dry_run,
            )?;
            commands::cmd_model_apply(commands::ApplyOptions {
                model_path: &model,
                db_path: &common.db.db_path,
                root: &common.root,
                dry_run,
                skip_optional,
                strict,
                autoremove: !no_autoremove,
                offline,
            })
            .await
        }

        cli::ModelCommands::Check {
            model,
            db,
            verbose,
            offline,
        } => commands::cmd_model_check(&model, &db.db_path, verbose, offline).await,

        cli::ModelCommands::RemoteDiff { model, refresh, db } => {
            commands::cmd_model_remote_diff(&model, &db.db_path, refresh).await
        }

        cli::ModelCommands::Snapshot {
            output,
            db,
            description,
        } => commands::cmd_model_snapshot(&output, &db.db_path, description.as_deref()).await,

        cli::ModelCommands::Lock { model, output, db } => {
            commands::cmd_model_lock(&model, output.as_deref(), &db.db_path).await
        }

        cli::ModelCommands::Update { model, db } => {
            commands::cmd_model_update(&model, &db.db_path).await
        }

        cli::ModelCommands::Publish {
            model,
            name,
            version,
            repo,
            description,
            force,
            sign_key,
            db,
        } => {
            commands::cmd_model_publish(
                &model,
                &name,
                &version,
                &repo,
                description.as_deref(),
                &db.db_path,
                force,
                sign_key.as_deref(),
            )
            .await
        }
    }
}
