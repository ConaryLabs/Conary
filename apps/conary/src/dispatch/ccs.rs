// apps/conary/src/dispatch/ccs.rs

use std::borrow::Cow;

use anyhow::Result;

use super::context::require_live_mutation;
use crate::cli;
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};

pub(super) fn dispatch_ccs_command(ccs_cmd: cli::CcsCommands) -> Result<()> {
    match ccs_cmd {
        cli::CcsCommands::Init {
            path,
            name,
            version,
            force,
            template,
        } => commands::ccs::cmd_ccs_init(&path, name, &version, force, template),

        cli::CcsCommands::Build {
            path,
            output,
            target,
            source,
            install_prefix,
            no_chunked,
            dry_run,
            local_dev,
            key,
        } => commands::ccs::cmd_ccs_build(commands::ccs::CcsBuildOptions {
            path,
            output,
            target,
            source,
            install_prefix,
            chunked: !no_chunked,
            dry_run,
            local_dev,
            key,
        }),

        cli::CcsCommands::Lint { path, format } => commands::ccs::cmd_ccs_lint(&path, format),

        cli::CcsCommands::Inspect {
            package,
            files,
            hooks,
            deps,
            format,
        } => commands::ccs::cmd_ccs_inspect(&package, files, hooks, deps, &format),

        cli::CcsCommands::Verify { package, policy } => {
            commands::ccs::cmd_ccs_verify(&package, policy)
        }

        cli::CcsCommands::Test {
            package,
            dry_run,
            policy,
            keep_workspace,
        } => commands::ccs::cmd_ccs_test(&package, dry_run, policy, keep_workspace),

        cli::CcsCommands::Sign {
            package,
            key,
            output,
        } => commands::ccs::cmd_ccs_sign(&package, &key, output),

        cli::CcsCommands::Keygen {
            output,
            key_id,
            force,
        } => commands::ccs::cmd_ccs_keygen(&output, key_id, force),

        cli::CcsCommands::Install {
            package,
            common,
            dry_run,
            policy,
            components,
            sandbox,
            no_deps,
            yes,
            reinstall,
        } => {
            require_live_mutation(
                MutationIntent::from_apply_intent(yes),
                Cow::Borrowed("conary ccs install"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                dry_run,
            )?;
            commands::ccs::cmd_ccs_install(
                &package,
                &common.db.db_path,
                &common.root,
                dry_run,
                policy,
                components,
                sandbox.into(),
                no_deps,
                reinstall,
            )
        }

        cli::CcsCommands::Export {
            packages,
            output,
            format,
            policy,
        } => commands::ccs::cmd_ccs_export(&packages, &output, &format, &policy),

        cli::CcsCommands::Enhance {
            db,
            trove_id,
            all_pending,
            update_outdated,
            types,
            force,
            stats,
            dry_run,
        } => commands::ccs::cmd_ccs_enhance(
            &db.db_path,
            trove_id,
            all_pending,
            update_outdated,
            types,
            force,
            stats,
            dry_run,
        ),
    }
}
