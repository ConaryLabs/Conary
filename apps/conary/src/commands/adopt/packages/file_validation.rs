// apps/conary/src/commands/adopt/packages/file_validation.rs

//! Apply-time validation of file ownership captured during adoption planning.

use super::PlannedPackage;
use conary_core::db::models::{FileEntry, InstallSource, Trove};
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
        if owner
            .as_ref()
            .is_some_and(|trove| trove.install_source == InstallSource::CapturedRoot)
            || owner_name == package.identity.native.name()
            || (matches!(
                existing.node.source.kind,
                conary_core::payload::PayloadNodeKind::Directory
            ) && super::live_path_is_directory(&file.0)
                .map_err(|error| conary_core::Error::IoError(error.to_string()))?)
        {
            continue;
        }
        return Err(conary_core::Error::ConflictError(format!(
            "Path {} became tracked by package {} after adoption planning",
            file.0, owner_name
        )));
    }
    Ok(())
}
