// apps/conary/src/dispatch/cache.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_cache_command(cmd: cli::CacheCommands) -> Result<()> {
    match cmd {
        cli::CacheCommands::Populate {
            profile,
            sources_only,
            full,
            db,
        } => commands::cmd_cache_populate(&profile, sources_only, full, &db.db_path).await,
        cli::CacheCommands::Status { db } => commands::cmd_cache_status(&db.db_path).await,
    }
}
