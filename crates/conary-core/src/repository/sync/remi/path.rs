// crates/conary-core/src/repository/sync/remi/path.rs

//! File-backed entry point for signed Remi universe synchronization.

use crate::db::models::Repository;
use crate::error::{Error, Result};
use crate::repository::sync::RepositoryWriteAuthority;

use super::resolved_package_count;

pub(in crate::repository::sync) async fn sync_repository_remi_from_db_path<W>(
    db_path: std::path::PathBuf,
    repository: Repository,
    _write_authority: W,
) -> Result<usize>
where
    W: RepositoryWriteAuthority,
{
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
    let conn = crate::db::open_fast(&db_path)?;
    resolved_package_count(&conn, repository_id)
}
