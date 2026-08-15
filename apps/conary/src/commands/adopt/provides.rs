// apps/conary/src/commands/adopt/provides.rs

//! Typed installed-provider persistence for native adoption.

use conary_core::db::models::ProvideEntry;
use conary_core::packages::InstalledPackageIdentity;
use conary_core::repository::dependency_model::ProvidedCapability;
use rusqlite::Connection;

pub(super) fn insert_package_provides(
    conn: &Connection,
    trove_id: i64,
    identity: &InstalledPackageIdentity,
    provides: &[ProvidedCapability],
) -> conary_core::Result<()> {
    ProvideEntry::insert_package_capabilities(
        conn,
        trove_id,
        identity.name(),
        &identity.version(),
        identity.version_scheme(),
        provides,
    )
}
