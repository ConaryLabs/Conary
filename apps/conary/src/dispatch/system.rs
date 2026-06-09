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

pub(super) async fn dispatch_system_command(
    sys_cmd: cli::SystemCommands,
    allow_live_system_mutation: bool,
) -> Result<()> {
    match sys_cmd {
        cli::SystemCommands::Init { db } => commands::cmd_init(&db.db_path).await,

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
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
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
            }
        }

        cli::SystemCommands::Adopt {
            packages,
            db,
            full,
            system,
            status,
            dry_run,
            pattern,
            exclude,
            explicit_only,
            refresh,
            convert,
            jobs,
            no_chunking,
            sync_hook,
            remove_hook,
            quiet,
            from_sync_hook: _,
        } => {
            if sync_hook {
                commands::cmd_sync_hook_install(remove_hook).await
            } else if convert {
                commands::cmd_adopt_convert(&db.db_path, jobs, no_chunking, dry_run).await
            } else if status {
                commands::cmd_adopt_status(&db.db_path).await
            } else if refresh {
                commands::cmd_adopt_refresh(&db.db_path, full, dry_run, quiet).await
            } else if system {
                commands::cmd_adopt_system(
                    &db.db_path,
                    full,
                    dry_run,
                    pattern.as_deref(),
                    exclude.as_deref(),
                    explicit_only,
                )
                .await
            } else {
                if dry_run {
                    anyhow::bail!(
                        "single-package adoption dry-run is not implemented yet; use `conary system adopt --system --dry-run` for a system-wide preview or rerun without --dry-run when ready to adopt package(s)"
                    );
                }
                commands::cmd_adopt(&packages, &db.db_path, full).await
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
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
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
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
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

        cli::SystemCommands::Gc {
            db,
            objects_dir,
            keep_days,
            dry_run,
            chunks,
        } => commands::cmd_gc(&db.db_path, &objects_dir, keep_days, dry_run, chunks).await,

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
                    MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
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

        cli::SystemCommands::State(state_cmd) => {
            dispatch_system_state_command(state_cmd, allow_live_system_mutation).await
        }

        cli::SystemCommands::Generation(gen_cmd) => {
            dispatch_system_generation_command(gen_cmd, allow_live_system_mutation).await
        }

        cli::SystemCommands::Takeover {
            up_to,
            yes,
            dry_run,
            db,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary system takeover"),
                LiveMutationClass::AlwaysLive,
                dry_run,
            )?;
            commands::generation::takeover::cmd_system_takeover(&db.db_path, up_to, yes, dry_run)
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
