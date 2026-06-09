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
mod system;
mod system_generation;
mod system_redirect;
mod system_state;
mod system_trigger;
mod system_update_channel;
mod trust;
mod verify_derivation;

use anyhow::Result;
use std::borrow::Cow;

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
