// apps/conary/src/commands/try_session/session/watch_marker.rs
//! Durable marker identifying sessions created by `conary try --watch`.

use std::path::Path;

use anyhow::{Context, Result};
use conary_core::db::models::TrySession;

use super::super::TryWatchMarkerRequest;

const TRY_WATCH_MARKER_FILE: &str = ".conary-try-watch-session.json";

#[derive(serde::Serialize)]
struct TryWatchMarker<'a> {
    schema_version: u16,
    operation_id: &'a str,
}

pub(super) fn write_try_watch_marker(
    work_dir: &Path,
    marker: TryWatchMarkerRequest<'_>,
) -> Result<()> {
    #[cfg(test)]
    if std::env::var_os("CONARY_TEST_TRY_WATCH_MARKER_FAIL").is_some() {
        anyhow::bail!("failed to write try watch marker: forced test failure");
    }

    let path = work_dir.join(TRY_WATCH_MARKER_FILE);
    let payload = TryWatchMarker {
        schema_version: 1,
        operation_id: marker.operation_id,
    };
    let json = serde_json::to_vec(&payload)?;
    std::fs::write(&path, json)
        .with_context(|| format!("failed to write try watch marker {}", path.display()))?;
    Ok(())
}

pub(super) fn is_watch_created_try_session(session: &TrySession) -> bool {
    Path::new(&session.work_dir)
        .join(TRY_WATCH_MARKER_FILE)
        .is_file()
}
