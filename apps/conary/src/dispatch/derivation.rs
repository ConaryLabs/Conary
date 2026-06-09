// apps/conary/src/dispatch/derivation.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_derivation_command(
    derivation_cmd: cli::DerivationCommands,
) -> Result<()> {
    match derivation_cmd {
        cli::DerivationCommands::Build {
            recipe,
            env,
            cas_dir,
            db_path,
        } => commands::cmd_derivation_build(&recipe, &env, &cas_dir, db_path.as_deref()).await,
        cli::DerivationCommands::Show { recipe, env_hash } => {
            commands::cmd_derivation_show(&recipe, &env_hash).await
        }
    }
}
