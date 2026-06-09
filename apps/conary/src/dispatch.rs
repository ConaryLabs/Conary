// apps/conary/src/dispatch.rs
//! Conary CLI command dispatch.

mod bootstrap;
mod cache;
mod capability;
mod catalog;
mod collection;
mod config;
mod context;
mod derivation;
mod derive;
mod federation;
mod profile;
mod provenance;
mod query;
mod repo;
mod root;
mod trust;
mod verify_derivation;

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::generate;
use std::borrow::Cow;
use std::io;

use self::context::{legacy_replay_options, require_live_mutation};
use crate::cli::{self, Cli};
use crate::command_risk;
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};

pub async fn dispatch(cli: Cli) -> Result<()> {
    let allow_live_system_mutation = cli.allow_live_system_mutation;
    command_risk::enforce_cli_policy(allow_live_system_mutation, &cli)?;
    root::dispatch_command(cli.command, allow_live_system_mutation).await
}

async fn dispatch_system_command(
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

        cli::SystemCommands::State(state_cmd) => match state_cmd {
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
        },

        cli::SystemCommands::Generation(gen_cmd) => match gen_cmd {
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
        },

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

        cli::SystemCommands::Trigger(trigger_cmd) => match trigger_cmd {
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
        },

        cli::SystemCommands::Redirect(redirect_cmd) => match redirect_cmd {
            cli::RedirectCommands::List {
                db,
                r#type,
                verbose,
            } => commands::cmd_redirect_list(&db.db_path, r#type.as_deref(), verbose).await,

            cli::RedirectCommands::Add {
                source,
                target,
                db,
                r#type,
                source_version,
                target_version,
                message,
            } => {
                commands::cmd_redirect_add(
                    &source,
                    &target,
                    &db.db_path,
                    &r#type,
                    source_version.as_deref(),
                    target_version.as_deref(),
                    message.as_deref(),
                )
                .await
            }

            cli::RedirectCommands::Show {
                source,
                db,
                version,
            } => commands::cmd_redirect_show(&source, &db.db_path, version.as_deref()).await,

            cli::RedirectCommands::Remove { source, db } => {
                commands::cmd_redirect_remove(&source, &db.db_path).await
            }

            cli::RedirectCommands::Resolve {
                package,
                db,
                version,
            } => commands::cmd_redirect_resolve(&package, &db.db_path, version.as_deref()).await,
        },

        cli::SystemCommands::UpdateChannel { action } => match action {
            cli::UpdateChannelAction::Get { db } => {
                commands::cmd_update_channel_get(&db.db_path).await
            }
            cli::UpdateChannelAction::Set { url, db } => {
                commands::cmd_update_channel_set(&db.db_path, &url).await
            }
            cli::UpdateChannelAction::Reset { db } => {
                commands::cmd_update_channel_reset(&db.db_path).await
            }
        },
    }
}

async fn dispatch_ccs_command(
    ccs_cmd: cli::CcsCommands,
    allow_live_system_mutation: bool,
) -> Result<()> {
    match ccs_cmd {
        cli::CcsCommands::Init {
            path,
            name,
            version,
            force,
        } => commands::ccs::cmd_ccs_init(&path, name, &version, force).await,

        cli::CcsCommands::Build {
            path,
            output,
            target,
            source,
            no_classify,
            no_chunked,
            dry_run,
        } => {
            commands::ccs::cmd_ccs_build(
                &path,
                &output,
                &target,
                source,
                no_classify,
                !no_chunked,
                dry_run,
            )
            .await
        }

        cli::CcsCommands::Inspect {
            package,
            files,
            hooks,
            deps,
            format,
        } => commands::ccs::cmd_ccs_inspect(&package, files, hooks, deps, &format).await,

        cli::CcsCommands::Verify {
            package,
            policy,
            allow_unsigned,
        } => commands::ccs::cmd_ccs_verify(&package, policy, allow_unsigned).await,

        cli::CcsCommands::Sign {
            package,
            key,
            output,
        } => commands::ccs::cmd_ccs_sign(&package, &key, output).await,

        cli::CcsCommands::Keygen {
            output,
            key_id,
            force,
        } => commands::ccs::cmd_ccs_keygen(&output, key_id, force).await,

        cli::CcsCommands::Install {
            package,
            common,
            dry_run,
            allow_unsigned,
            policy,
            components,
            sandbox,
            no_deps,
            no_scripts,
            yes,
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            reinstall,
            allow_capabilities,
            capability_policy,
        } => {
            let legacy_replay =
                legacy_replay_options(allow_legacy_replay, allow_foreign_legacy_replay);
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary ccs install"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                dry_run,
            )?;
            commands::ccs::cmd_ccs_install_with_replay_options(
                &package,
                &common.db.db_path,
                &common.root,
                dry_run,
                allow_unsigned,
                policy,
                components,
                sandbox.into(),
                no_deps,
                no_scripts,
                reinstall,
                allow_capabilities,
                capability_policy,
                legacy_replay,
            )
            .await
        }

        cli::CcsCommands::Export {
            packages,
            output,
            format,
            db,
        } => commands::ccs::cmd_ccs_export(&packages, &output, &format, &db.db_path).await,

        cli::CcsCommands::Shell {
            packages,
            db,
            shell,
            env,
            keep,
        } => {
            commands::ccs::cmd_ccs_shell(&packages, &db.db_path, shell.as_deref(), &env, keep).await
        }

        cli::CcsCommands::Run {
            package,
            command,
            db,
            env,
        } => commands::ccs::cmd_ccs_run(&package, &command, &db.db_path, &env).await,

        cli::CcsCommands::Enhance {
            db,
            trove_id,
            all_pending,
            update_outdated,
            types,
            force,
            stats,
            dry_run,
            install_root,
        } => {
            commands::ccs::cmd_ccs_enhance(
                &db.db_path,
                trove_id,
                all_pending,
                update_outdated,
                types,
                force,
                stats,
                dry_run,
                &install_root,
            )
            .await
        }
    }
}

async fn dispatch_model_command(
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

async fn dispatch_automation_command(
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
