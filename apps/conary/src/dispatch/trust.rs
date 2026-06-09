// apps/conary/src/dispatch/trust.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_trust_command(cmd: cli::TrustCommands) -> Result<()> {
    match cmd {
        cli::TrustCommands::KeyGen { role, output } => {
            commands::cmd_trust_key_gen(&role, &output).await
        }
        cli::TrustCommands::Init { repo, root, db } => {
            commands::cmd_trust_init(&repo, &root, &db.db_path).await
        }
        cli::TrustCommands::Enable { repo, tuf_url, db } => {
            commands::cmd_trust_enable(&repo, tuf_url.as_deref(), &db.db_path).await
        }
        cli::TrustCommands::Disable { repo, force, db } => {
            commands::cmd_trust_disable(&repo, force, &db.db_path).await
        }
        cli::TrustCommands::Status { repo, db } => {
            commands::cmd_trust_status(&repo, &db.db_path).await
        }
        cli::TrustCommands::Verify { repo, db } => {
            commands::cmd_trust_verify(&repo, &db.db_path).await
        }
    }
}
