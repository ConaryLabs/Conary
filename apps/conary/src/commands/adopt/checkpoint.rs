// apps/conary/src/commands/adopt/checkpoint.rs

use anyhow::{Context, Result};
use conary_core::db::backup::{CheckpointReason, DbBackupRecord, create_checkpoint};
use std::path::Path;

pub(super) fn write_db_checkpoint(
    db_path: impl AsRef<Path>,
    reason: CheckpointReason,
) -> Result<DbBackupRecord> {
    let db_path = db_path.as_ref();
    create_checkpoint(db_path, reason).with_context(|| {
        format!(
            "failed to write {} Conary DB checkpoint backup for {}",
            reason.as_str(),
            db_path.display()
        )
    })
}
