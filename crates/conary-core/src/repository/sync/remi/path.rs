// crates/conary-core/src/repository/sync/remi/path.rs

//! Path-based Remi sparse-sync coordination.

use crate::db::models::Repository;
use crate::error::{Error, Result};
use tracing::info;

use super::RemiSparseSync;
use super::run::{
    RemiSyncFailureCategory, RemiSyncFailureStage, abort_remi_sync_run, append_remi_sync_page,
    begin_remi_sync_run, finish_remi_sync_run,
};
use crate::repository::sync::{RepositoryWriteAuthority, run_blocking_write};

pub(in crate::repository::sync) async fn sync_repository_remi_from_db_path<W>(
    db_path: std::path::PathBuf,
    repo: Repository,
    write_authority: W,
) -> Result<usize>
where
    W: RepositoryWriteAuthority,
{
    info!(
        "Syncing repository {} from the paginated Remi sparse index",
        repo.name
    );
    let mut fetcher = RemiSparseSync::new(&repo)?;
    let begin_path = db_path.clone();
    let begin_repo = repo.clone();
    let run = run_blocking_write(write_authority.clone(), move || {
        let conn = crate::db::open_fast(&begin_path)?;
        begin_remi_sync_run(&conn, &begin_repo)
    })
    .await?;

    loop {
        let rows = match fetcher.next_rows().await {
            Ok(Some(rows)) => rows,
            Ok(None) => break,
            Err(error) => {
                let abort_path = db_path.clone();
                let abort_run = run.clone();
                let category = RemiSyncFailureCategory::from_error(&error);
                let evidence = error.to_string();
                let _ = run_blocking_write(write_authority.clone(), move || {
                    let conn = crate::db::open_fast(&abort_path)?;
                    abort_remi_sync_run(
                        &conn,
                        &abort_run,
                        RemiSyncFailureStage::FetchingObjects,
                        category,
                        &evidence,
                    )
                })
                .await;
                return Err(error);
            }
        };
        let revision = fetcher.revision().ok_or_else(|| {
            Error::InternalError("Remi sparse page did not establish a server revision".to_string())
        })?;
        let page_path = db_path.clone();
        let page_run = run.clone();
        if let Err(error) = run_blocking_write(write_authority.clone(), move || {
            let conn = crate::db::open_fast(&page_path)?;
            append_remi_sync_page(&conn, &page_run, rows, revision)
        })
        .await
        {
            let abort_path = db_path.clone();
            let abort_run = run.clone();
            let category = RemiSyncFailureCategory::from_error(&error);
            let evidence = error.to_string();
            let _ = run_blocking_write(write_authority.clone(), move || {
                let conn = crate::db::open_fast(&abort_path)?;
                abort_remi_sync_run(
                    &conn,
                    &abort_run,
                    RemiSyncFailureStage::Ingesting,
                    category,
                    &evidence,
                )
            })
            .await;
            return Err(error);
        }
    }

    let finish_path = db_path.clone();
    let repository_id = run.repository_id;
    let finish_run = run.clone();
    let revision = fetcher.revision().ok_or_else(|| {
        Error::InternalError("Remi sparse sync completed without a server revision".to_string())
    })?;
    let finish_result = run_blocking_write(write_authority.clone(), move || {
        let conn = crate::db::open_fast(&finish_path)?;
        let mut persisted = Repository::find_by_id(&conn, repository_id)?.ok_or_else(|| {
            Error::NotFound(format!(
                "Repository {repository_id} not found during sparse sync"
            ))
        })?;
        finish_remi_sync_run(&conn, &mut persisted, &finish_run, revision)
    })
    .await;
    let count = match finish_result {
        Ok(count) => count,
        Err(error) => {
            let abort_path = db_path.clone();
            let abort_run = run;
            let category = RemiSyncFailureCategory::from_error(&error);
            let evidence = error.to_string();
            let _ = run_blocking_write(write_authority, move || {
                let conn = crate::db::open_fast(&abort_path)?;
                abort_remi_sync_run(
                    &conn,
                    &abort_run,
                    RemiSyncFailureStage::Publishing,
                    category,
                    &evidence,
                )
            })
            .await;
            return Err(error);
        }
    };
    info!(
        "Synchronized {} packages from Remi repository {}",
        count, repo.name
    );
    Ok(count)
}
