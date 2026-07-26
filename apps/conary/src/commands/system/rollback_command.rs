// apps/conary/src/commands/system/rollback_command.rs

//! Serialized, retryable changeset rollback.

use super::rollback_restore;
use anyhow::{Context, Result, anyhow, bail};
use conary_core::db::models::{
    Changeset, ChangesetKind, ChangesetStatus, GenerationPublication, Trove,
};
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use rusqlite::{OptionalExtension, TransactionBehavior};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use tracing::info;

struct CommittedRollback {
    rollback_id: i64,
    publication_debt: GenerationPublication,
    forward_debts: Vec<GenerationPublication>,
    summary: String,
    snapshots: Vec<crate::commands::TroveSnapshot>,
    removed_troves: Vec<Trove>,
}

/// Roll back one effective forward mutation.
pub async fn cmd_rollback(changeset_id: i64, db_path: &str) -> Result<()> {
    rollback_changeset(changeset_id, db_path, || Ok(()))
}

#[cfg(test)]
pub(super) async fn cmd_rollback_with_forced_precommit_failure(
    changeset_id: i64,
    db_path: &str,
) -> Result<()> {
    rollback_changeset(changeset_id, db_path, || {
        bail!("forced rollback failure after publication candidate persistence")
    })
}

fn rollback_changeset<F>(changeset_id: i64, db_path: &str, precommit_probe: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    let mut engine = TransactionEngine::new(TransactionConfig::for_runtime_root(&runtime_root))?;
    engine
        .begin()
        .context("failed to acquire the system mutation lock for rollback")?;

    info!("Rolling back changeset: {}", changeset_id);
    println!("Rolling back changeset: {}", changeset_id);
    std::io::stdout().flush()?;
    if let Ok(delay_ms) = std::env::var("CONARY_TEST_HOLD_DURING_ROLLBACK_MS")
        && let Ok(delay_ms) = delay_ms.parse::<u64>()
        && delay_ms > 0
    {
        std::thread::sleep(Duration::from_millis(delay_ms));
    }

    require_active_generation(changeset_id, &runtime_root)?;
    let mut conn = crate::commands::open_db(db_path)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut staged_candidate = None;
    let execution = (|| -> Result<CommittedRollback> {
        let changeset = require_effective_rollback_target(&tx, changeset_id)?;
        let authority = exact_rollback_authority(&changeset)?;
        let crate::commands::RollbackAuthority {
            removed_troves: snapshots,
            selected_root: rollback_root,
            system: rollback_system,
            materialized_directories,
        } = authority;
        for snapshot in &snapshots {
            snapshot.validate().with_context(|| {
                format!(
                    "Changeset {changeset_id} contains invalid rollback authority for {}",
                    snapshot.name
                )
            })?;
        }

        let removed_troves = troves_installed_by_changeset(&tx, changeset_id)?;
        validate_rollback_removal_targets(&tx, &removed_troves)?;
        let summary = rollback_summary(changeset_id, &snapshots, &removed_troves);
        let pending_debts = GenerationPublication::pending_recoverable(&tx)?;
        let (forward_debts, unrelated_debts): (Vec<_>, Vec<_>) = pending_debts
            .into_iter()
            .partition(|debt| debt.trigger_changeset_id == Some(changeset_id));
        if !unrelated_debts.is_empty() {
            bail!(
                "cannot roll back changeset {changeset_id} while another exact selected-root publication is pending"
            );
        }

        let mut rollback_changeset = Changeset::new_rollback(
            format!(
                "Rollback of changeset {} ({})",
                changeset_id, changeset.description
            ),
            changeset_id,
        );
        let rollback_id = rollback_changeset.insert(&tx)?;
        for trove in removed_troves.iter().rev() {
            delete_reverted_trove(&tx, rollback_id, trove)?;
        }
        rollback_restore::restore_snapshots(&tx, rollback_id, &snapshots)?;
        rollback_restore::restore_materialized_directories(&tx, &materialized_directories)?;
        rollback_system.restore(&tx)?;

        GenerationPublication::abandon_recoverable_for_changeset(&tx, changeset_id)?;
        rollback_changeset.update_status(&tx, ChangesetStatus::Applied)?;
        let updated = tx.execute(
            "UPDATE changesets
             SET status = 'rolled_back',
                 rolled_back_at = CURRENT_TIMESTAMP,
                 reversed_by_changeset_id = ?1
             WHERE id = ?2
               AND kind = 'mutation'
               AND status = 'applied'
               AND reversed_by_changeset_id IS NULL",
            [rollback_id, changeset_id],
        )?;
        if updated != 1 {
            bail!("changeset {changeset_id} stopped being eligible before rollback commit");
        }

        let publication_debt =
            crate::commands::generation::publication::record_selected_root_state(
                &tx,
                &crate::commands::generation::publication::PublicationRequest {
                    db_path,
                    summary: &summary,
                    trigger_changeset_id: Some(rollback_id),
                    tx_uuid: rollback_changeset.tx_uuid.as_deref(),
                    config_transaction:
                        conary_core::config_transaction::GenerationConfigTransaction::default(),
                },
            )?;
        crate::commands::generation::selected_root::persist_captured_publication_candidate(
            &runtime_root,
            &publication_debt,
            &rollback_root,
        )?;
        staged_candidate = Some(publication_debt.clone());
        precommit_probe()?;

        Ok(CommittedRollback {
            rollback_id,
            publication_debt,
            forward_debts,
            summary,
            snapshots,
            removed_troves,
        })
    })();

    let committed = match execution {
        Ok(committed) => committed,
        Err(error) => {
            drop(tx);
            if let Some(debt) = staged_candidate.as_ref() {
                let _ = crate::commands::generation::selected_root::remove_publication_candidate(
                    &runtime_root,
                    debt,
                );
            }
            return Err(error);
        }
    };
    if let Err(error) = tx.commit() {
        let _ = crate::commands::generation::selected_root::remove_publication_candidate(
            &runtime_root,
            &committed.publication_debt,
        );
        return Err(error.into());
    }

    for forward_debt in &committed.forward_debts {
        crate::commands::generation::selected_root::remove_publication_candidate(
            &runtime_root,
            forward_debt,
        )?;
    }
    let publication = crate::commands::generation::publication::publish_recorded_selected_root(
        &conn,
        db_path,
        &committed.summary,
        committed.publication_debt,
    )?;
    if publication.needs_publication {
        crate::commands::append_deferred_follow_up_metadata(
            &conn,
            committed.rollback_id,
            crate::commands::publication_deferred_follow_up(
                "rollback generation publication is pending".to_string(),
            ),
        )?;
        crate::commands::generation::publication::warn_if_publication_pending(
            committed.rollback_id,
            &publication,
        );
    }

    for trove in &committed.removed_troves {
        println!(
            "  Removed reverted package {} {}",
            trove.name, trove.version
        );
    }
    println!(
        "Rollback complete. Changeset {} has been reversed.",
        changeset_id
    );
    for snapshot in &committed.snapshots {
        println!("  Restored {} version {}", snapshot.name, snapshot.version);
    }
    let restored_file_count: usize = committed
        .snapshots
        .iter()
        .map(|snapshot| snapshot.files.len())
        .sum();
    println!("  Files in DB: {}", restored_file_count);
    Ok(())
}

fn require_effective_rollback_target(
    conn: &rusqlite::Connection,
    changeset_id: i64,
) -> Result<Changeset> {
    let changeset = Changeset::find_by_id(conn, changeset_id)?
        .ok_or_else(|| anyhow!("Changeset {changeset_id} not found"))?;
    if changeset.kind != ChangesetKind::Mutation {
        bail!(
            "Changeset {changeset_id} is a {} lineage record, not a forward mutation",
            changeset.kind
        );
    }
    match changeset.status {
        ChangesetStatus::RolledBack => {
            bail!("Changeset {changeset_id} is already rolled back")
        }
        ChangesetStatus::Pending => {
            bail!("Cannot rollback pending changeset {changeset_id}")
        }
        ChangesetStatus::Applied => {}
    }
    if let Some(reverse_id) = changeset.reversed_by_changeset_id {
        bail!("Changeset {changeset_id} has already been reversed by changeset {reverse_id}");
    }
    let later_effective_mutation: Option<i64> = conn
        .query_row(
            "SELECT id
             FROM changesets
             WHERE id > ?1
               AND kind = 'mutation'
               AND status = 'applied'
               AND reversed_by_changeset_id IS NULL
             ORDER BY id DESC
             LIMIT 1",
            [changeset_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(later_id) = later_effective_mutation {
        bail!(
            "Cannot roll back changeset {changeset_id} while later applied mutation {later_id} exists; roll back newest mutations first"
        );
    }
    Ok(changeset)
}

fn exact_rollback_authority(changeset: &Changeset) -> Result<crate::commands::RollbackAuthority> {
    let changeset_id = changeset.id.unwrap_or_default();
    let json = changeset.metadata.as_deref().ok_or_else(|| {
        anyhow!(
            "Changeset {changeset_id} has no exact v5 rollback authority; current-schema rollback refuses retired or incomplete mutations"
        )
    })?;
    crate::commands::parse_rollback_authority(json)?.ok_or_else(|| {
        anyhow!(
            "Changeset {changeset_id} metadata has no exact v5 rollback authority; current-schema rollback refuses retired or incomplete mutations"
        )
    })
}

fn require_active_generation(changeset_id: i64, runtime_root: &ConaryRuntimeRoot) -> Result<()> {
    if conary_core::generation::mount::current_generation(runtime_root.root())?.is_none() {
        bail!(
            "Cannot roll back changeset {changeset_id} without an active composefs generation. \
             Build or activate a generation first, then retry rollback."
        );
    }
    Ok(())
}

fn troves_installed_by_changeset(
    conn: &rusqlite::Connection,
    changeset_id: i64,
) -> Result<Vec<Trove>> {
    Ok(Trove::list_all(conn)?
        .into_iter()
        .filter(|trove| trove.installed_by_changeset_id == Some(changeset_id))
        .collect())
}

fn validate_rollback_removal_targets(conn: &rusqlite::Connection, troves: &[Trove]) -> Result<()> {
    for expected in troves {
        let trove_id = expected
            .id
            .ok_or_else(|| anyhow!("rollback removal target has no database identity"))?;
        let current = Trove::find_by_id(conn, trove_id)?
            .ok_or_else(|| anyhow!("rollback removal target {trove_id} disappeared"))?;
        if current.name != expected.name
            || current.version != expected.version
            || current.architecture != expected.architecture
            || current.installed_by_changeset_id != expected.installed_by_changeset_id
        {
            bail!("rollback removal target {trove_id} changed after lifecycle preflight");
        }
    }
    Ok(())
}

fn rollback_summary(
    changeset_id: i64,
    snapshots: &[crate::commands::TroveSnapshot],
    removed_troves: &[Trove],
) -> String {
    if !removed_troves.is_empty() {
        format!("Rollback changeset {changeset_id}")
    } else if let Some(snapshot) = snapshots.first() {
        format!("Rollback removal of {}", snapshot.name)
    } else {
        format!("Rollback changeset {changeset_id}")
    }
}

fn delete_reverted_trove(
    tx: &rusqlite::Transaction<'_>,
    rollback_changeset_id: i64,
    trove: &Trove,
) -> Result<()> {
    let trove_id = trove
        .id
        .ok_or_else(|| anyhow!("rollback removal target has no database identity"))?;
    let payload = conary_core::db::models::PackagePayloadOwnership::load(tx, trove_id)?;
    let entries = payload
        .entries()
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();
    for path in payload.materialized_removal_paths() {
        let entry = entries.get(path.as_str()).ok_or_else(|| {
            anyhow!(
                "rollback removal path {} has no package payload authority for {}",
                path,
                trove.name
            )
        })?;
        tx.execute(
            "INSERT INTO file_history (changeset_id, path, sha256_hash, action)
             VALUES (?1, ?2, ?3, 'delete')",
            rusqlite::params![
                rollback_changeset_id,
                path,
                entry
                    .content
                    .as_ref()
                    .map(|content| content.sha256.as_str())
            ],
        )?;
    }
    conary_core::db::models::ConfigFile::delete_by_trove(tx, trove_id)?;
    Trove::delete(tx, trove_id)?;
    Ok(())
}
