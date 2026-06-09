// apps/conary/src/dispatch/system_redirect.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_system_redirect_command(
    redirect_cmd: cli::RedirectCommands,
) -> Result<()> {
    match redirect_cmd {
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
    }
}
