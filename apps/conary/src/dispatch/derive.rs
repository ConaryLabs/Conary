// apps/conary/src/dispatch/derive.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_derive_command(derive_cmd: cli::DeriveCommands) -> Result<()> {
    match derive_cmd {
        cli::DeriveCommands::List { db, verbose } => {
            commands::cmd_derive_list(&db.db_path, verbose).await
        }

        cli::DeriveCommands::Show { name, db } => {
            commands::cmd_derive_show(&name, &db.db_path).await
        }

        cli::DeriveCommands::Create {
            name,
            from,
            version_suffix,
            description,
            db,
        } => {
            commands::cmd_derive_create(
                &name,
                &from,
                version_suffix.as_deref(),
                description.as_deref(),
                &db.db_path,
            )
            .await
        }

        cli::DeriveCommands::Patch {
            name,
            patch_file,
            strip,
            db,
        } => commands::cmd_derive_patch(&name, &patch_file, strip, &db.db_path).await,

        cli::DeriveCommands::Override {
            name,
            target,
            source,
            mode,
            db,
        } => {
            commands::cmd_derive_override(&name, &target, source.as_deref(), mode, &db.db_path)
                .await
        }

        cli::DeriveCommands::Build { name, db } => {
            commands::cmd_derive_build(&name, &db.db_path).await
        }

        cli::DeriveCommands::Delete { name, db } => {
            commands::cmd_derive_delete(&name, &db.db_path).await
        }

        cli::DeriveCommands::Stale { db } => commands::cmd_derive_stale(&db.db_path).await,
    }
}
