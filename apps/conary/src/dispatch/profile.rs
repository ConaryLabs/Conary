// apps/conary/src/dispatch/profile.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_profile_command(profile_cmd: cli::ProfileCommands) -> Result<()> {
    match profile_cmd {
        cli::ProfileCommands::Generate { manifest, output } => {
            commands::cmd_profile_generate(&manifest, output.as_deref()).await
        }
        cli::ProfileCommands::Show { path } => commands::cmd_profile_show(&path).await,
        cli::ProfileCommands::Diff { old, new } => commands::cmd_profile_diff(&old, &new).await,
        cli::ProfileCommands::Publish {
            profile,
            endpoint,
            token,
        } => commands::cmd_profile_publish(&profile, endpoint.as_deref(), token.as_deref()).await,
    }
}
