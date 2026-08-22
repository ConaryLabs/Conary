// crates/conary-core/src/repository/sync/remi.rs

//! Signed endpoint-wide Remi universe synchronization.

use std::path::PathBuf;

use rusqlite::Connection;

use crate::db::models::Repository;
use crate::error::{Error, Result};

mod path;
mod run;

pub(super) use path::sync_repository_remi_from_db_path;
pub use run::{
    PROFILE_SYNC_HEARTBEAT_INTERVAL, ProfileSyncFailureCategory, ProfileSyncFailureStage,
    ProfileSyncRun, ProfileSyncRunMember, ProfileSyncRunRecovery, abort_profile_sync_run,
    acknowledge_profile_sync_candidate_cleanup, begin_profile_sync_run,
    begin_profile_sync_run_with_input, begin_profile_sync_run_with_members,
    heartbeat_profile_sync_run, ready_profile_sync_run, record_profile_sync_run_member,
    recover_expired_profile_sync_runs,
};

pub(super) async fn sync_repository_remi(
    conn: &Connection,
    repository: &mut Repository,
) -> Result<usize> {
    let db_path = main_database_path(conn)?;
    let endpoint = repository
        .default_strategy_endpoint
        .as_deref()
        .ok_or_else(|| {
            Error::ConfigError(format!(
                "repository '{}' has no Remi universe endpoint",
                repository.name
            ))
        })?;
    crate::repository::universe::sync_remi_universe(&db_path, endpoint).await?;
    let repository_id = repository
        .id
        .ok_or_else(|| Error::MissingId("Remi repository has no ID".to_string()))?;
    let refreshed = crate::db::open_fast(&db_path)?;
    let count = resolved_package_count(&refreshed, repository_id)?;
    let persisted = Repository::find_by_id(&refreshed, repository_id)?.ok_or_else(|| {
        Error::NotFound(format!(
            "repository {repository_id} disappeared during universe sync"
        ))
    })?;
    repository.last_checked_at = persisted.last_checked_at;
    repository.last_changed_at = persisted.last_changed_at;
    repository.last_validated_at = persisted.last_validated_at;
    repository.last_published_at = persisted.last_published_at;
    Ok(count)
}

pub(super) fn main_database_path(conn: &Connection) -> Result<PathBuf> {
    let mut statement = conn.prepare("PRAGMA database_list")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;
    for row in rows {
        let (name, path) = row?;
        if name == "main" && !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    Err(Error::ConfigError(
        "Remi universe sync requires a file-backed Conary database".to_string(),
    ))
}

pub(super) fn resolved_package_count(conn: &Connection, repository_id: i64) -> Result<usize> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM resolved_repository_packages WHERE repository_id = ?1",
        [repository_id],
        |row| row.get::<_, i64>(0),
    )?;
    usize::try_from(count).map_err(|_| {
        Error::ConflictError(format!(
            "Remi repository {repository_id} has a negative or oversized package count"
        ))
    })
}
