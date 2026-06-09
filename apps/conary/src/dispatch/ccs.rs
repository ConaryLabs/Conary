// apps/conary/src/dispatch/ccs.rs

use std::borrow::Cow;

use anyhow::Result;

use super::context::{legacy_replay_options, require_live_mutation};
use crate::cli;
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};

pub(super) async fn dispatch_ccs_command(
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
