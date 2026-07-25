// conary-core/src/repository/sync/support.rs

use crate::error::{Error, Result};
use rusqlite::{Connection, params};

/// Get the current timestamp as ISO 8601.
pub fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Parse an ISO 8601 timestamp to Unix seconds.
pub fn parse_timestamp(timestamp: &str) -> Result<u64> {
    let parsed = chrono::DateTime::parse_from_rfc3339(timestamp)
        .map_err(|error| Error::ParseError(format!("Invalid timestamp: {error}")))?;

    u64::try_from(parsed.timestamp()).map_err(|_| {
        Error::ParseError(format!(
            "Timestamp is before Unix epoch (negative): {timestamp}"
        ))
    })
}

pub(super) async fn run_blocking_sync<F, T>(operation: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| Error::InternalError(format!("blocking sync task failed: {error}")))?
}

pub(super) fn has_trusted_root(conn: &Connection, repository_id: i64) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM tuf_roots WHERE repository_id = ?1
        )",
        params![repository_id],
        |row| row.get(0),
    )
    .map_err(Error::from)
}

/// Rebase a metadata-owned download URL onto an explicit content origin.
///
/// Package checksums from trusted metadata remain the content authority.
pub(super) fn rebase_download_url(
    download_url: &str,
    metadata_url: &str,
    content_url: Option<&str>,
) -> String {
    let Some(content_base) = content_url else {
        return download_url.to_string();
    };

    let metadata_base = metadata_url.trim_end_matches('/');
    let content_base = content_base.trim_end_matches('/');
    let Some(relative) = download_url.strip_prefix(metadata_base) else {
        return download_url.to_string();
    };

    format!("{}/{}", content_base, relative.trim_start_matches('/'))
}
