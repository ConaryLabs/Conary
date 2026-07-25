// apps/conary/src/commands/adopt/requirements.rs

//! Exact installed requirement acquisition and persistence for native adoption.

use anyhow::{Context, Result, bail};
use conary_core::db::models::InstalledRequirementGroup;
use conary_core::packages::{SystemPackageManager, dpkg_query, pacman_query, rpm_query};
use conary_core::repository::dependency_model::RepositoryRequirementGroup;
use conary_core::repository::versioning::VersionScheme;
use rusqlite::Connection;

pub(super) fn query_package_requirements(
    manager: SystemPackageManager,
    package_name: &str,
) -> Result<Vec<RepositoryRequirementGroup>> {
    match manager {
        SystemPackageManager::Rpm => rpm_query::query_package_requirement_groups(package_name),
        SystemPackageManager::Dpkg => dpkg_query::query_package_requirement_groups(package_name),
        SystemPackageManager::Pacman => {
            pacman_query::query_package_requirement_groups(package_name)
        }
        SystemPackageManager::Unknown => bail!("unsupported native package manager"),
    }
    .with_context(|| format!("could not inspect exact requirements for '{package_name}'"))
}

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
