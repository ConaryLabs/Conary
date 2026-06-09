// apps/conary/src/dispatch/capability.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_capability_command(cmd: cli::CapabilityCommands) -> Result<()> {
    match cmd {
        cli::CapabilityCommands::Show {
            package,
            db,
            format,
        } => commands::cmd_capability_show(&db.db_path, &package, &format).await,
        cli::CapabilityCommands::Validate { path, verbose } => {
            commands::cmd_capability_validate(&path, verbose).await
        }
        cli::CapabilityCommands::List {
            db,
            missing,
            format,
        } => commands::cmd_capability_list(&db.db_path, missing, &format).await,
        cli::CapabilityCommands::Generate {
            binary,
            args,
            output,
            timeout,
        } => commands::cmd_capability_generate(&binary, &args, output.as_deref(), timeout).await,
        cli::CapabilityCommands::Audit {
            package,
            db,
            command,
            timeout,
        } => {
            commands::cmd_capability_audit(&db.db_path, &package, command.as_deref(), timeout).await
        }
        cli::CapabilityCommands::Run {
            package,
            command,
            db,
            audit,
        } => commands::cmd_capability_run(&db.db_path, &package, &command, audit).await,
    }
}
