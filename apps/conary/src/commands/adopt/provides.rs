// apps/conary/src/commands/adopt/provides.rs

//! Typed installed-provider acquisition and persistence for native adoption.

use anyhow::{Context, Result, bail};
use conary_core::db::models::ProvideEntry;
use conary_core::packages::{
    InstalledPackageIdentity, SystemPackageManager, dpkg_query, pacman_query, rpm_query,
};
use conary_core::repository::dependency_model::ProvidedCapability;
use rusqlite::Connection;

pub(super) fn query_package_provides(
    manager: SystemPackageManager,
    identity: &InstalledPackageIdentity,
) -> Result<Vec<ProvidedCapability>> {
    let provides = match manager {
        SystemPackageManager::Rpm => rpm_query::query_package_provides(identity),
        SystemPackageManager::Dpkg => dpkg_query::query_package_provides(identity),
        SystemPackageManager::Pacman => pacman_query::query_package_provides(identity),
        SystemPackageManager::Eopkg => {
            conary_core::packages::eopkg::query::query_package_provides(identity)
        }
        SystemPackageManager::Unknown => bail!("unsupported native package manager"),
    }
    .with_context(|| {
        format!(
            "could not inspect exact provides for '{}'",
            identity.selector()
        )
    })?;

    Ok(provides)
}

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
