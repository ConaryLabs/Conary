// apps/conary/src/dispatch/catalog.rs

// Groups the small distro, canonical, groups, and registry routers.

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_distro_command(distro_cmd: cli::DistroCommands) -> Result<()> {
    match distro_cmd {
        cli::DistroCommands::List { db } => commands::distro::cmd_distro_list(&db.db_path).await,
        cli::DistroCommands::Info { db } => commands::distro::cmd_distro_info(&db.db_path).await,
    }
}

pub(super) async fn dispatch_canonical_command(can_cmd: cli::CanonicalCommands) -> Result<()> {
    match can_cmd {
        cli::CanonicalCommands::Show { name, db } => {
            commands::canonical::cmd_canonical_show(&db.db_path, &name).await
        }
        cli::CanonicalCommands::Search { query, db } => {
            commands::canonical::cmd_canonical_search(&db.db_path, &query).await
        }
        cli::CanonicalCommands::Unmapped { db } => {
            commands::canonical::cmd_canonical_unmapped(&db.db_path).await
        }
    }
}

pub(super) async fn dispatch_groups_command(grp_cmd: cli::GroupsCommands) -> Result<()> {
    match grp_cmd {
        cli::GroupsCommands::List { db } => commands::groups::cmd_groups_list(&db.db_path).await,
        cli::GroupsCommands::Show { name, distro, db } => {
            commands::groups::cmd_groups_show(&db.db_path, &name, distro.as_deref()).await
        }
    }
}

pub(super) async fn dispatch_registry_command(reg_cmd: cli::RegistryCommands) -> Result<()> {
    match reg_cmd {
        cli::RegistryCommands::Update { db } => {
            commands::registry::cmd_registry_update(&db.db_path).await
        }
        cli::RegistryCommands::Stats { db } => {
            commands::registry::cmd_registry_stats(&db.db_path).await
        }
    }
}
