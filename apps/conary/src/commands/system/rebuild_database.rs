// conary/src/commands/system/rebuild_database.rs

use super::init::{configure_current_database, require_init_privileges};
use anyhow::Result;
use std::path::Path;
use tracing::info;

/// Snapshot a retired pre-alpha database and replace its active state.
pub async fn cmd_rebuild_database(db_path: &str) -> Result<()> {
    let db_path = Path::new(db_path);
    require_init_privileges(db_path)?;
    info!(database = %db_path.display(), "Rebuilding retired Conary database");

    let outcome = conary_core::db::rebuild::rebuild_discarding_state(db_path)?;
    crate::ui::status(
        "Rebuilt",
        &format!("database at {} with the current schema", db_path.display()),
    );
    crate::ui::field("Retired schema", &outcome.observed_schema);
    crate::ui::field(
        "Retired snapshot",
        &outcome.retired_snapshot_path.display().to_string(),
    );
    configure_current_database(db_path.to_string_lossy().as_ref()).await?;
    crate::ui::note(
        "Repository metadata and installed-package state were discarded; resync repositories and explicitly re-adopt any native packages that Conary should track.",
    );
    Ok(())
}
