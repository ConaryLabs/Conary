// apps/conary/src/dispatch/automation.rs

use std::borrow::Cow;

use anyhow::Result;

use super::context::require_live_mutation;
use crate::cli;
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};

pub(super) async fn dispatch_automation_command(
    auto_cmd: cli::AutomationCommands,
    allow_live_system_mutation: bool,
) -> Result<()> {
    match auto_cmd {
        cli::AutomationCommands::Status {
            db,
            format,
            verbose,
        } => commands::cmd_automation_status(&db.db_path, &format, verbose).await,

        cli::AutomationCommands::Check {
            common,
            categories,
            quiet,
        } => {
            commands::cmd_automation_check(&common.db.db_path, &common.root, categories, quiet)
                .await
        }

        cli::AutomationCommands::Apply {
            common,
            yes,
            categories,
            dry_run,
            no_scripts,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary automation apply"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                dry_run,
            )?;
            commands::cmd_automation_apply(
                &common.db.db_path,
                &common.root,
                yes,
                categories,
                dry_run,
                no_scripts,
            )
            .await
        }

        cli::AutomationCommands::Configure {
            db,
            show,
            mode,
            enable,
            disable,
            interval,
            enable_ai,
            disable_ai,
        } => {
            commands::cmd_automation_configure(
                &db.db_path,
                show,
                mode,
                enable,
                disable,
                interval,
                enable_ai,
                disable_ai,
            )
            .await
        }

        cli::AutomationCommands::Daemon { common, pidfile } => {
            commands::cmd_automation_daemon(&common.db.db_path, &common.root, &pidfile).await
        }

        cli::AutomationCommands::History {
            db,
            limit,
            category,
            status,
            since,
        } => commands::cmd_automation_history(&db.db_path, limit, category, status, since).await,
    }
}
