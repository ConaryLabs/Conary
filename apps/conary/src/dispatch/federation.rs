// apps/conary/src/dispatch/federation.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_federation_command(cmd: cli::FederationCommands) -> Result<()> {
    match cmd {
        cli::FederationCommands::Status { db, verbose } => {
            commands::cmd_federation_status(&db.db_path, verbose).await
        }
        cli::FederationCommands::Peers {
            db,
            tier,
            enabled_only,
        } => commands::cmd_federation_peers(&db.db_path, tier.as_deref(), enabled_only).await,
        cli::FederationCommands::AddPeer {
            url,
            db,
            tier,
            name,
            tls_fingerprint,
        } => {
            commands::cmd_federation_add_peer(
                &url,
                &db.db_path,
                &tier,
                name.as_deref(),
                tls_fingerprint.as_deref(),
            )
            .await
        }
        cli::FederationCommands::RemovePeer { peer, db } => {
            commands::cmd_federation_remove_peer(&peer, &db.db_path).await
        }
        cli::FederationCommands::Stats { db, days } => {
            commands::cmd_federation_stats(&db.db_path, days).await
        }
        cli::FederationCommands::EnablePeer { peer, db } => {
            commands::cmd_federation_enable_peer(&peer, &db.db_path, true).await
        }
        cli::FederationCommands::DisablePeer { peer, db } => {
            commands::cmd_federation_enable_peer(&peer, &db.db_path, false).await
        }
        cli::FederationCommands::Test { db, peer, timeout } => {
            commands::cmd_federation_test(&db.db_path, peer.as_deref(), timeout).await
        }
        cli::FederationCommands::Scan { db, duration, add } => {
            commands::cmd_federation_scan(&db.db_path, duration, add).await
        }
    }
}
