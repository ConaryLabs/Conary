// apps/conary/src/commands/adopt/packages/file_validation.rs

//! Apply-time validation of file ownership captured during adoption planning.

use super::{LIVE_ROOT_PACKAGE_NAME, PlannedPackage};
use conary_core::db::models::{FileEntry, Trove};
use rusqlite::Connection;

pub(super) fn validate_planned_file_claims(
    conn: &Connection,
    package: &PlannedPackage,
) -> conary_core::Result<()> {
    for file in &package.files {
        let Some(existing) = FileEntry::find_by_path(conn, &file.0)? else {
            continue;
        };
        let owner = Trove::find_by_id(conn, existing.trove_id)?;
        let owner_name = owner
            .as_ref()
            .map(|trove| trove.name.as_str())
            .unwrap_or("<missing tracked owner>");
        if owner_name == LIVE_ROOT_PACKAGE_NAME || owner_name == package.identity.native.name() {
            continue;
        }
        return Err(conary_core::Error::ConflictError(format!(
            "Path {} became tracked by package {} after adoption planning",
            file.0, owner_name
        )));
    }
    Ok(())
}
