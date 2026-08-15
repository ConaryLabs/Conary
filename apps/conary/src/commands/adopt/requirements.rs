// apps/conary/src/commands/adopt/requirements.rs

//! Exact installed requirement persistence for native adoption.

use anyhow::Result;
use conary_core::db::models::InstalledRequirementGroup;
use conary_core::repository::dependency_model::RepositoryRequirementGroup;
use conary_core::repository::versioning::VersionScheme;
use rusqlite::Connection;

pub(super) fn insert_package_requirements(
    conn: &Connection,
    trove_id: i64,
    version_scheme: VersionScheme,
    _package_name: &str,
    requirements: &[RepositoryRequirementGroup],
) -> Result<()> {
    InstalledRequirementGroup::insert_groups(conn, trove_id, version_scheme, requirements)?;
    Ok(())
}

pub(super) fn replace_package_requirements(
    conn: &Connection,
    trove_id: i64,
    version_scheme: VersionScheme,
    package_name: &str,
    requirements: &[RepositoryRequirementGroup],
) -> Result<()> {
    InstalledRequirementGroup::delete_by_trove(conn, trove_id)?;
    insert_package_requirements(conn, trove_id, version_scheme, package_name, requirements)
}
