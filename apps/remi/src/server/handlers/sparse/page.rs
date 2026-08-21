// apps/remi/src/server/handlers/sparse/page.rs

//! Stable sparse-index page selection and resolution projection.

use anyhow::Context;
use conary_core::db::models::RepositoryPackage;
use conary_core::repository::remi_metadata::{
    REMI_SPARSE_MIN_PACKAGE_SIZE, RemiSparsePackageList, RemiSparsePackagePage,
    RemiSparseResolutionEntry, RemiSparseResolutionVersionEntry, RemiSparseRevision,
    validate_remi_public_name,
};
use rusqlite::Connection;
use std::collections::BTreeMap;

/// Build a paginated list of unique package names for a distro, aggregating
/// across all matching repos (e.g. arch-core + arch-extra).
pub(super) fn build_package_list(
    db_path: &std::path::Path,
    distro: &str,
    page: usize,
    per_page: usize,
) -> Result<RemiSparsePackageList, anyhow::Error> {
    let conn = Connection::open(db_path)?;
    let selection = select_sparse_name_page(&conn, distro, page, per_page)?;

    Ok(RemiSparsePackageList {
        distro: distro.to_string(),
        source_profile: selection.source_profile,
        packages: selection.names,
        total: selection.total,
        page,
        per_page,
    })
}

pub(super) fn build_package_page(
    db_path: &std::path::Path,
    distro: &str,
    page: usize,
    per_page: usize,
) -> Result<RemiSparsePackagePage, anyhow::Error> {
    let conn = Connection::open(db_path)?;
    let tx = conn.unchecked_transaction()?;
    let selection = select_sparse_name_page(&tx, distro, page, per_page)?;
    let revision = sparse_projection_revision(&tx, &selection.source_profile)?;
    let packages =
        super::load_visible_repository_packages(&tx, &selection.repo_ids, &selection.names)?;
    let package_ids = packages
        .iter()
        .map(|package| {
            package
                .id
                .ok_or_else(|| anyhow::anyhow!("repository package has no persisted ID"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut exact_metadata =
        crate::server::package_metadata::load_exact_package_metadata_batch(&tx, &package_ids)?;
    let mut packages_by_name = BTreeMap::<String, Vec<RepositoryPackage>>::new();
    for package in packages {
        packages_by_name
            .entry(package.name.clone())
            .or_default()
            .push(package);
    }

    let mut entries = Vec::with_capacity(selection.names.len());
    for name in selection.names {
        let packages = packages_by_name.remove(&name).ok_or_else(|| {
            anyhow::anyhow!(
                "sparse visibility query listed package '{name}' without a fetchable version"
            )
        })?;
        let mut versions = Vec::with_capacity(packages.len());
        for package in packages {
            let package_id = package
                .id
                .ok_or_else(|| anyhow::anyhow!("repository package has no persisted ID"))?;
            let exact = exact_metadata.remove(&package_id).ok_or_else(|| {
                anyhow::anyhow!("exact metadata missing for repository package {package_id}")
            })?;
            versions.push(build_resolution_version(package, exact)?);
        }
        entries.push(RemiSparseResolutionEntry {
            name,
            distro: distro.to_string(),
            versions,
        });
    }
    if !exact_metadata.is_empty() {
        anyhow::bail!("bulk sparse metadata included packages outside the selected name page");
    }
    tx.commit()?;

    Ok(RemiSparsePackagePage {
        distro: distro.to_string(),
        source_profile: selection.source_profile,
        revision,
        packages: entries,
        total: selection.total,
        page,
        per_page,
    })
}

fn sparse_projection_revision(
    conn: &Connection,
    source_profile: &str,
) -> Result<RemiSparseRevision, anyhow::Error> {
    let sequence = conn.query_row(
        "SELECT COALESCE((
             SELECT sequence
             FROM remi_sparse_projection_revisions
             WHERE source_profile = ?1
         ), 0)",
        [source_profile],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(RemiSparseRevision {
        sequence: u64::try_from(sequence).context("sparse projection revision is negative")?,
    })
}

fn build_resolution_version(
    package: RepositoryPackage,
    exact: crate::server::package_metadata::ExactPackageMetadata,
) -> Result<RemiSparseResolutionVersionEntry, anyhow::Error> {
    let metadata = package
        .metadata
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .with_context(|| {
            format!(
                "repository package '{}' version '{}' has malformed persisted metadata",
                package.name, package.version
            )
        })?;
    Ok(RemiSparseResolutionVersionEntry {
        version: package.version,
        release: (!package.package_release.is_empty()).then_some(package.package_release),
        provides: exact.provides,
        requirement_groups: exact.requirement_groups,
        architecture: package.architecture,
        size: package.size,
        metadata,
    })
}

struct SparseNamePageSelection {
    source_profile: String,
    repo_ids: Vec<i64>,
    names: Vec<String>,
    total: usize,
}

fn select_sparse_name_page(
    conn: &Connection,
    distro: &str,
    page: usize,
    per_page: usize,
) -> Result<SparseNamePageSelection, anyhow::Error> {
    let source_profile =
        conary_core::repository::supported_profiles::profile_for_remi_route(distro)
            .ok_or_else(|| anyhow::anyhow!("unsupported public route '{distro}'"))?;
    let repositories = super::find_repositories_for_profile(conn, source_profile.id())?;
    let repo_ids = repositories
        .iter()
        .map(super::super::require_persisted_repository_id)
        .collect::<anyhow::Result<Vec<_>>>()?;
    if repo_ids.is_empty() {
        return Ok(SparseNamePageSelection {
            source_profile: source_profile.id().to_string(),
            repo_ids,
            names: Vec::new(),
            total: 0,
        });
    }

    let placeholders = (1..=repo_ids.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let minimum_size_idx = repo_ids.len() + 1;
    let count_sql = format!(
        "SELECT COUNT(DISTINCT name) FROM repository_packages
         WHERE repository_id IN ({placeholders}) AND size >= ?{minimum_size_idx}"
    );
    let mut count_params: Vec<Box<dyn rusqlite::types::ToSql>> =
        repo_ids.iter().map(|id| Box::new(*id) as _).collect();
    count_params.push(Box::new(REMI_SPARSE_MIN_PACKAGE_SIZE));
    let total = conn.query_row(
        &count_sql,
        rusqlite::params_from_iter(&count_params),
        |row| row.get::<_, i64>(0),
    )?;
    let total = usize::try_from(total)
        .context("sparse package-name count does not fit the response platform")?;

    let offset = page
        .checked_sub(1)
        .and_then(|zero_based| zero_based.checked_mul(per_page))
        .ok_or_else(|| anyhow::anyhow!("sparse page offset overflow"))?;
    let limit = sqlite_page_value(per_page, "limit")?;
    let offset = sqlite_page_value(offset, "offset")?;
    let minimum_size_idx = repo_ids.len() + 1;
    let limit_idx = repo_ids.len() + 2;
    let offset_idx = repo_ids.len() + 3;
    let list_sql = format!(
        "SELECT DISTINCT name FROM repository_packages
         WHERE repository_id IN ({placeholders})
         AND size >= ?{minimum_size_idx}
         ORDER BY name
         LIMIT ?{limit_idx} OFFSET ?{offset_idx}"
    );

    let mut list_params: Vec<Box<dyn rusqlite::types::ToSql>> =
        repo_ids.iter().map(|id| Box::new(*id) as _).collect();
    list_params.push(Box::new(REMI_SPARSE_MIN_PACKAGE_SIZE));
    list_params.push(Box::new(limit));
    list_params.push(Box::new(offset));

    let mut stmt = conn.prepare(&list_sql)?;
    let packages: Vec<String> = stmt
        .query_map(rusqlite::params_from_iter(&list_params), |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for package in &packages {
        validate_remi_public_name(package).map_err(|reason| {
            anyhow::anyhow!("repository contains invalid sparse package name {package:?}: {reason}")
        })?;
    }

    Ok(SparseNamePageSelection {
        source_profile: source_profile.id().to_string(),
        repo_ids,
        names: packages,
        total,
    })
}

fn sqlite_page_value(value: usize, field: &str) -> Result<i64, anyhow::Error> {
    i64::try_from(value).with_context(|| format!("sparse page {field} exceeds SQLite's range"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{
        Repository, RepositoryPackage, RepositoryProvide, RepositoryRequirement,
        RepositoryRequirementGroup,
    };
    use conary_core::repository::versioning::VersionScheme;

    #[test]
    fn sqlite_page_values_fail_closed_instead_of_wrapping_negative() {
        assert_eq!(sqlite_page_value(128, "limit").unwrap(), 128);
        if let Some(overflow) = usize::try_from(i64::MAX)
            .ok()
            .and_then(|maximum| maximum.checked_add(1))
        {
            let error = sqlite_page_value(overflow, "offset").unwrap_err();
            assert!(error.to_string().contains("exceeds SQLite's range"));
        }
    }

    #[test]
    fn sparse_revision_tracks_visible_content_but_not_disabled_staging_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        let mut repository = Repository::new(
            "fedora-visible".to_string(),
            "https://packages.test".to_string(),
        );
        repository.source_profile = Some("fedora-44".to_string());
        let repository_id = repository.insert(&conn).unwrap();
        let mut package = RepositoryPackage::new(
            repository_id,
            "alpha".to_string(),
            "1".to_string(),
            VersionScheme::Rpm,
            "sha256:alpha".to_string(),
            1024,
            "https://packages.test/alpha".to_string(),
        );
        let package_id = package.insert(&conn).unwrap();
        let mut provide = RepositoryProvide::new(
            package_id,
            "alpha".to_string(),
            Some("1".to_string()),
            "package".to_string(),
            Some("alpha = 1".to_string()),
            VersionScheme::Rpm,
        );
        let provide_id = provide.insert(&conn).unwrap();
        let mut group = RepositoryRequirementGroup::new(
            package_id,
            "depends".to_string(),
            "hard".to_string(),
            r#"{"kind":"atom","clause":{"name":"glibc","version_constraint":null,"capability_kind":null,"native_text":null}}"#.to_string(),
        );
        let group_id = group.insert(&conn).unwrap();
        let mut requirement = RepositoryRequirement::new(
            package_id,
            group_id,
            "glibc".to_string(),
            None,
            "package".to_string(),
            "runtime".to_string(),
            Some("glibc".to_string()),
        );
        let requirement_id = requirement.insert(&conn).unwrap();
        let initial = sparse_projection_revision(&conn, "fedora-44").unwrap();

        conn.execute(
            "UPDATE repository_packages SET version = '2' WHERE id = ?1",
            [package_id],
        )
        .unwrap();
        let version_changed = sparse_projection_revision(&conn, "fedora-44").unwrap();
        assert!(version_changed.sequence > initial.sequence);

        conn.execute(
            "UPDATE repository_provides SET raw = 'alpha = 2' WHERE id = ?1",
            [provide_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE repository_requirements SET raw = 'glibc >= 2.39' WHERE id = ?1",
            [requirement_id],
        )
        .unwrap();
        let relations_changed = sparse_projection_revision(&conn, "fedora-44").unwrap();
        assert!(relations_changed.sequence > version_changed.sequence);

        let mut staging = repository;
        staging.id = None;
        staging.name = "disabled-candidate".to_string();
        staging.enabled = false;
        let staging_id = staging.insert(&conn).unwrap();
        let mut staged_package = RepositoryPackage::new(
            staging_id,
            "candidate".to_string(),
            "1".to_string(),
            VersionScheme::Rpm,
            "sha256:candidate".to_string(),
            1024,
            "https://packages.test/candidate".to_string(),
        );
        staged_package.insert(&conn).unwrap();

        assert_eq!(
            sparse_projection_revision(&conn, "fedora-44").unwrap(),
            relations_changed
        );
    }
}
