// crates/conary-core/src/repository/sync/remi/client.rs

//! Bounded client-side Remi sparse candidates and monotonic publication fencing.

use std::io::{BufRead, BufReader, BufWriter, Write};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::RemiSparseSync;
use crate::db::models::{Repository, RepositoryPackage};
use crate::error::{Error, Result};
use crate::repository::remi_metadata::RemiSparseRevision;
use crate::repository::sync::native::append_synced_package_rows;
use crate::repository::sync::types::SyncedPackageRow;
use crate::repository::sync::{current_timestamp, link_canonical_ids};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RemiClientSyncFence {
    repository_id: i64,
    fencing_epoch: i64,
    source_profile: String,
    endpoint: String,
}

pub(super) struct RemiClientCatalogCandidate {
    file: tempfile::NamedTempFile,
    package_count: usize,
    revision: RemiSparseRevision,
}

/// Start one client refresh by monotonically invalidating every older fetch.
pub(super) fn begin_client_sync(
    conn: &Connection,
    repository: &Repository,
) -> Result<RemiClientSyncFence> {
    let repository_id = repository
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let source_profile = repository.source_profile.clone().ok_or_else(|| {
        Error::ConfigError(format!(
            "repository '{}' has no exact Remi source profile",
            repository.name
        ))
    })?;
    let endpoint = repository
        .default_strategy_endpoint
        .clone()
        .ok_or_else(|| {
            Error::ConfigError(format!(
                "repository '{}' has no Remi endpoint",
                repository.name
            ))
        })?;
    if repository.default_strategy.as_deref() != Some("remi") {
        return Err(Error::ConfigError(format!(
            "repository '{}' is not a Remi sparse client",
            repository.name
        )));
    }

    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let prior_epoch = tx
        .query_row(
            "SELECT fencing_epoch FROM remi_client_sync_state WHERE repository_id = ?1",
            [repository_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    let fencing_epoch = prior_epoch.checked_add(1).ok_or_else(|| {
        Error::InternalError("Remi client sync fencing epoch overflow".to_string())
    })?;
    tx.execute(
        "INSERT INTO remi_client_sync_state (
             repository_id, fencing_epoch, active_revision_json
         ) VALUES (?1, ?2, NULL)
         ON CONFLICT(repository_id) DO UPDATE SET
             fencing_epoch = excluded.fencing_epoch",
        params![repository_id, fencing_epoch],
    )?;
    tx.commit()?;

    Ok(RemiClientSyncFence {
        repository_id,
        fencing_epoch,
        source_profile,
        endpoint,
    })
}

/// Fetch fixed-size sparse pages into a private disk-backed candidate.
pub(super) async fn fetch_client_candidate(
    repository: &Repository,
) -> Result<RemiClientCatalogCandidate> {
    let mut fetcher = RemiSparseSync::new(repository)?;
    let mut file = tempfile::NamedTempFile::new()?;
    let mut package_count = 0_usize;
    {
        let mut writer = BufWriter::new(file.as_file_mut());
        while let Some(rows) = fetcher.next_rows().await? {
            package_count = package_count.checked_add(rows.len()).ok_or_else(|| {
                Error::InternalError("Remi client package count overflow".to_string())
            })?;
            write_candidate_page(&mut writer, &rows)?;
        }
        writer.flush()?;
    }
    file.as_file().sync_all()?;
    let revision = fetcher.revision().ok_or_else(|| {
        Error::InternalError("Remi sparse sync completed without a server revision".to_string())
    })?;
    Ok(RemiClientCatalogCandidate {
        file,
        package_count,
        revision,
    })
}

fn write_candidate_page(writer: &mut impl Write, rows: &[SyncedPackageRow]) -> Result<()> {
    serde_json::to_writer(&mut *writer, rows)?;
    writer.write_all(b"\n")?;
    Ok(())
}

/// Atomically replace one client repository only while its exact fence remains
/// current. The private candidate is consumed a bounded page at a time.
pub(super) fn publish_client_candidate(
    conn: &Connection,
    repository: &mut Repository,
    fence: &RemiClientSyncFence,
    candidate: RemiClientCatalogCandidate,
) -> Result<usize> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    require_current_fence(&tx, fence)?;
    let mut persisted = Repository::find_by_id(&tx, fence.repository_id)?.ok_or_else(|| {
        Error::NotFound(format!(
            "Repository {} not found during sparse publication",
            fence.repository_id
        ))
    })?;
    if persisted.default_strategy.as_deref() != Some("remi")
        || persisted.source_profile.as_deref() != Some(fence.source_profile.as_str())
        || persisted.default_strategy_endpoint.as_deref() != Some(fence.endpoint.as_str())
    {
        return Err(Error::ConflictError(format!(
            "repository {} Remi source binding changed during refresh",
            fence.repository_id
        )));
    }
    reject_revision_regression(&tx, fence.repository_id, &candidate.revision)?;

    RepositoryPackage::delete_by_repository(&tx, fence.repository_id)?;
    let mut reader = BufReader::new(candidate.file.reopen()?);
    let mut line = String::new();
    let mut persisted_count = 0_usize;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let rows: Vec<SyncedPackageRow> = serde_json::from_str(line.trim_end())?;
        persisted_count = persisted_count
            .checked_add(append_synced_package_rows(&tx, rows)?)
            .ok_or_else(|| {
                Error::InternalError("Remi client persisted package count overflow".to_string())
            })?;
    }
    if persisted_count != candidate.package_count {
        return Err(Error::ConflictError(format!(
            "Remi client candidate declared {} packages but contained {persisted_count}",
            candidate.package_count
        )));
    }
    link_canonical_ids(&tx, fence.repository_id)?;
    let revision_json = serde_json::to_string(&candidate.revision)?;
    let updated = tx.execute(
        "UPDATE remi_client_sync_state
         SET active_revision_json = ?1
         WHERE repository_id = ?2 AND fencing_epoch = ?3",
        params![revision_json, fence.repository_id, fence.fencing_epoch],
    )?;
    if updated != 1 {
        return Err(stale_fence_error(fence));
    }
    persisted.last_sync = Some(current_timestamp());
    persisted.update(&tx)?;
    tx.commit()?;
    repository.last_sync = persisted.last_sync;
    Ok(persisted_count)
}

fn require_current_fence(tx: &Transaction<'_>, fence: &RemiClientSyncFence) -> Result<()> {
    let current = tx
        .query_row(
            "SELECT fencing_epoch FROM remi_client_sync_state WHERE repository_id = ?1",
            [fence.repository_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if current != Some(fence.fencing_epoch) {
        return Err(stale_fence_error(fence));
    }
    Ok(())
}

fn reject_revision_regression(
    tx: &Transaction<'_>,
    repository_id: i64,
    candidate: &RemiSparseRevision,
) -> Result<()> {
    let active = tx
        .query_row(
            "SELECT active_revision_json FROM remi_client_sync_state WHERE repository_id = ?1",
            [repository_id],
            |row| row.get::<_, Option<String>>(0),
        )?
        .map(|value| serde_json::from_str::<RemiSparseRevision>(&value))
        .transpose()?;
    if let Some(active) = active
        && (candidate.projection_schema != active.projection_schema
            || candidate.sequence < active.sequence
            || (candidate.sequence == active.sequence && candidate != &active))
    {
        return Err(Error::ConflictError(format!(
            "repository {repository_id} Remi sparse revision regressed or forked"
        )));
    }
    Ok(())
}

fn stale_fence_error(fence: &RemiClientSyncFence) -> Error {
    Error::ConflictError(format!(
        "repository {} Remi client sync lost fencing epoch {}",
        fence.repository_id, fence.fencing_epoch
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::ensure_current;
    use crate::repository::versioning::VersionScheme;

    fn repository(conn: &Connection) -> Repository {
        let mut repository = Repository::new(
            "remi-client".to_string(),
            "https://remi.example.test".to_string(),
        );
        repository.default_strategy = Some("remi".to_string());
        repository.default_strategy_endpoint = Some("https://remi.example.test".to_string());
        repository.source_profile = Some("fedora-44".to_string());
        repository.id = Some(repository.insert(conn).unwrap());
        repository
    }

    fn row(repository_id: i64, name: &str) -> SyncedPackageRow {
        SyncedPackageRow {
            package: RepositoryPackage::new(
                repository_id,
                name.to_string(),
                "1".to_string(),
                VersionScheme::Rpm,
                format!("checksum-{name}"),
                1,
                format!("https://packages.example.test/{name}.rpm"),
            ),
            provides: Vec::new(),
            requirement_groups: Vec::new(),
            requirement_group_clauses: Vec::new(),
        }
    }

    fn candidate(
        pages: &[Vec<SyncedPackageRow>],
        revision: RemiSparseRevision,
    ) -> RemiClientCatalogCandidate {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let mut package_count = 0;
        {
            let mut writer = BufWriter::new(file.as_file_mut());
            for page in pages {
                package_count += page.len();
                write_candidate_page(&mut writer, page).unwrap();
            }
            writer.flush().unwrap();
        }
        RemiClientCatalogCandidate {
            file,
            package_count,
            revision,
        }
    }

    #[test]
    fn newer_client_fence_rejects_stale_publication() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let mut repository = repository(&conn);
        let first = begin_client_sync(&conn, &repository).unwrap();
        let second = begin_client_sync(&conn, &repository).unwrap();
        let repository_id = repository.id.unwrap();

        let error = publish_client_candidate(
            &conn,
            &mut repository,
            &first,
            candidate(
                &[vec![row(repository_id, "stale")]],
                RemiSparseRevision::new(1, "00000000000000000000000000000001").unwrap(),
            ),
        )
        .unwrap_err();
        assert!(error.to_string().contains("lost fencing epoch"));
        assert!(
            RepositoryPackage::find_by_repository(&conn, repository_id)
                .unwrap()
                .is_empty()
        );

        publish_client_candidate(
            &conn,
            &mut repository,
            &second,
            candidate(
                &[vec![row(repository_id, "current")]],
                RemiSparseRevision::new(2, "00000000000000000000000000000002").unwrap(),
            ),
        )
        .unwrap();
        assert_eq!(
            RepositoryPackage::find_by_repository(&conn, repository_id).unwrap()[0].name,
            "current"
        );
    }

    #[test]
    fn malformed_disk_candidate_rolls_back_existing_snapshot() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let mut repository = repository(&conn);
        let repository_id = repository.id.unwrap();
        let first = begin_client_sync(&conn, &repository).unwrap();
        publish_client_candidate(
            &conn,
            &mut repository,
            &first,
            candidate(
                &[vec![row(repository_id, "existing")]],
                RemiSparseRevision::new(1, "00000000000000000000000000000001").unwrap(),
            ),
        )
        .unwrap();

        let second = begin_client_sync(&conn, &repository).unwrap();
        let mut malformed = candidate(
            &[vec![row(repository_id, "replacement")]],
            RemiSparseRevision::new(2, "00000000000000000000000000000002").unwrap(),
        );
        malformed.file.as_file_mut().set_len(0).unwrap();
        malformed
            .file
            .as_file_mut()
            .write_all(b"not-json\n")
            .unwrap();
        let error =
            publish_client_candidate(&conn, &mut repository, &second, malformed).unwrap_err();
        assert!(matches!(error, Error::Json(_)));
        assert_eq!(
            RepositoryPackage::find_by_repository(&conn, repository_id).unwrap()[0].name,
            "existing"
        );
    }
}
