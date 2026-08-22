// conary-core/src/repository/sync/remi/path.rs

//! Path-based Remi refresh boundary.
//!
//! The client-side sparse consumer remains distinct from Remi server profile
//! catalog activation. It fetches one complete typed sparse snapshot before a
//! short atomic replacement of the client's local repository rows.

use crate::db::models::Repository;
use crate::error::{Error, Result};
use crate::repository::sync::{RepositoryWriteAuthority, run_blocking_write};

use super::client::{begin_client_sync, fetch_client_candidate, publish_client_candidate};

pub(in crate::repository::sync) async fn sync_repository_remi_from_db_path<W>(
    db_path: std::path::PathBuf,
    repo: Repository,
    write_authority: W,
) -> Result<usize>
where
    W: RepositoryWriteAuthority,
{
    let repository_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let begin_path = db_path.clone();
    let begin_repo = repo.clone();
    let fence = run_blocking_write(write_authority.clone(), move || {
        let conn = crate::db::open_fast(&begin_path)?;
        begin_client_sync(&conn, &begin_repo)
    })
    .await?;
    let candidate = fetch_client_candidate(&repo).await?;
    run_blocking_write(write_authority, move || {
        let conn = crate::db::open_fast(&db_path)?;
        let mut persisted = Repository::find_by_id(&conn, repository_id)?.ok_or_else(|| {
            Error::NotFound(format!(
                "Repository {repository_id} not found during sparse sync"
            ))
        })?;
        publish_client_candidate(&conn, &mut persisted, &fence, candidate)
    })
    .await
}
