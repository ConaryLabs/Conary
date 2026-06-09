// apps/conary/src/dispatch/verify_derivation.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_verify_derivation_command(
    verify_cmd: cli::VerifyCommands,
) -> Result<()> {
    match verify_cmd {
        cli::VerifyCommands::Chain {
            profile,
            verbose,
            json,
            db,
        } => commands::verify::cmd_verify_chain(&profile, verbose, json, &db.db_path).await,
        cli::VerifyCommands::Rebuild {
            derivation,
            work_dir,
            db,
        } => commands::verify::cmd_verify_rebuild(&derivation, &work_dir, &db.db_path).await,
        cli::VerifyCommands::Diverse {
            profile_a,
            profile_b,
            db,
        } => commands::verify::cmd_verify_diverse(&profile_a, &profile_b, &db.db_path).await,
    }
}
