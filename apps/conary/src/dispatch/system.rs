// apps/conary/src/dispatch/system.rs

use std::borrow::Cow;
use std::io;

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::generate;

use super::context::require_live_mutation;
use super::system_generation::dispatch_system_generation_command;
use super::system_redirect::dispatch_system_redirect_command;
use super::system_state::dispatch_system_state_command;
use super::system_trigger::dispatch_system_trigger_command;
use super::system_update_channel::dispatch_system_update_channel_command;
use crate::cli::{self, Cli};
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};

pub(super) async fn dispatch_system_command(sys_cmd: cli::SystemCommands) -> Result<()> {
    match sys_cmd {
        cli::SystemCommands::Init { db } => commands::cmd_init(&db.db_path).await,

        cli::SystemCommands::RebuildDatabase {
            db,
            discard_state: _,
            yes,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes),
                Cow::Borrowed("conary system rebuild-db"),
                LiveMutationClass::LiveConaryState,
                false,
            )?;
            commands::cmd_rebuild_database(&db.db_path).await
        }

        cli::SystemCommands::Completions { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "conary", &mut io::stdout());
            Ok(())
        }

        cli::SystemCommands::History { db } => commands::cmd_history(&db.db_path).await,

        cli::SystemCommands::Verify {
            package,
            common,
            rpm,
        } => commands::cmd_verify(package, &common.db.db_path, &common.root, rpm).await,

        cli::SystemCommands::Restore {
            package,
            common,
            force,
            dry_run,
            yes,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes),
                Cow::Borrowed("conary system restore"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                dry_run,
            )?;
            if package == "all" {
                commands::cmd_restore_all(&common.db.db_path, &common.root, dry_run).await
            } else {
                commands::cmd_restore(
                    &package,
                    &common.db.db_path,
                    &common.root,
                    None,
                    None,
                    force,
                    dry_run,
                )
                .await
                .map(|_| ())
            }
        }

        cli::SystemCommands::Adopt {
            packages,
            db,
            package_manager,
            full,
            system,
            status,
            dry_run,
            pattern,
            exclude,
            explicit_only,
            refresh,
            sync_hook,
            remove_hook,
            quiet,
            from_sync_hook: _,
        } => {
            let package_manager = package_manager.map(Into::into);
            if sync_hook {
                commands::cmd_sync_hook_install(remove_hook, package_manager).await
            } else if status {
                commands::cmd_adopt_status(&db.db_path, package_manager).await
            } else if refresh {
                commands::cmd_adopt_refresh(&db.db_path, full, dry_run, quiet, package_manager)
                    .await
            } else if system {
                let outcome = commands::cmd_adopt_system(
                    &db.db_path,
                    full,
                    dry_run,
                    pattern.as_deref(),
                    exclude.as_deref(),
                    explicit_only,
                    package_manager,
                )
                .await?;
                if outcome.is_complete() {
                    Ok(())
                } else {
                    anyhow::bail!(
                        "Bulk adoption was incomplete:\n  {}",
                        outcome.failure_records().join("\n  ")
                    )
                }
            } else {
                commands::cmd_adopt(&packages, &db.db_path, full, dry_run, package_manager).await
            }
        }

        cli::SystemCommands::Unadopt {
            packages,
            db,
            all,
            dry_run,
            yes,
            keep_hooks,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes),
                Cow::Borrowed("conary system unadopt"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                dry_run,
            )?;
            commands::cmd_unadopt(
                commands::UnadoptOptions {
                    packages,
                    all,
                    dry_run,
                    keep_hooks,
                },
                &db.db_path,
            )
            .await
            .map(|_| ())
        }

        cli::SystemCommands::NativeHandoff {
            db,
            dry_run,
            yes,
            recover,
            keep_hooks,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes),
                Cow::Borrowed("conary system native-handoff"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                dry_run,
            )?;
            commands::cmd_native_handoff(
                commands::NativeHandoffOptions {
                    dry_run,
                    yes,
                    recover,
                    keep_hooks,
                },
                &db.db_path,
            )
            .await
            .map(|_| ())
        }

        cli::SystemCommands::Sbom {
            package_name,
            db,
            format,
            output,
        } => commands::cmd_sbom(&package_name, &db.db_path, &format, output.as_deref()).await,

        cli::SystemCommands::DbBackup { command } => match command {
            cli::DbBackupCommands::List { db } => commands::cmd_db_backup_list(&db.db_path),
            cli::DbBackupCommands::Verify { latest, db } => {
                commands::cmd_db_backup_verify(&db.db_path, latest)
            }
            cli::DbBackupCommands::Recover {
                latest,
                dry_run,
                yes,
                replace_healthy_db,
                db,
            } => {
                require_live_mutation(
                    MutationIntent::from_apply_intent(yes),
                    Cow::Borrowed("conary system db-backup recover"),
                    LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                    dry_run,
                )?;
                commands::cmd_db_backup_recover(
                    &db.db_path,
                    latest,
                    dry_run,
                    yes,
                    replace_healthy_db,
                )
            }
        },

        cli::SystemCommands::State(state_cmd) => dispatch_system_state_command(state_cmd).await,

        cli::SystemCommands::Generation(gen_cmd) => {
            dispatch_system_generation_command(gen_cmd).await
        }

        cli::SystemCommands::Takeover {
            up_to,
            yes,
            dry_run,
            package_manager,
            db,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes),
                Cow::Borrowed("conary system takeover"),
                LiveMutationClass::AlwaysLive,
                dry_run,
            )?;
            commands::generation::takeover::cmd_system_takeover(
                &db.db_path,
                up_to,
                yes,
                dry_run,
                package_manager.map(Into::into),
            )
            .await
        }

        cli::SystemCommands::Trigger(trigger_cmd) => {
            dispatch_system_trigger_command(trigger_cmd).await
        }

        cli::SystemCommands::Redirect(redirect_cmd) => {
            dispatch_system_redirect_command(redirect_cmd).await
        }

        cli::SystemCommands::UpdateChannel { action } => {
            dispatch_system_update_channel_command(action).await
        }
    }
}
