// conary-core/src/repository/sync/native.rs

use crate::db::models::{
    AuthenticatedSnapshotIdentity, ConvertedPackage, Repository, RepositoryPackage,
    RepositoryProvide, RepositoryRequirement, RepositoryRequirementGroup as DbRequirementGroup,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::{
    ConditionalRequirementBehavior, RepositoryCapabilityKind, RepositoryRequirementGroup,
    RepositoryRequirementKind,
};
use crate::repository::parsers::PackageMetadata;
use rusqlite::Connection;

use super::types::SyncedPackageRow;
use super::{current_timestamp, link_canonical_ids};

pub(super) fn persist_native_sync_rows(
    conn: &Connection,
    repo: &mut Repository,
    synced_packages: Vec<SyncedPackageRow>,
    snapshot: AuthenticatedSnapshotIdentity,
) -> Result<usize> {
    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let count = synced_packages.len();

    let tx = conn.unchecked_transaction()?;

    repo.admit_authenticated_snapshot(snapshot)?;
    persist_synced_package_rows(&tx, repo_id, synced_packages)?;
    link_canonical_ids(&tx, repo_id)?;
    if let Some(source_profile) = repo.source_profile.as_deref() {
        ConvertedPackage::reconcile_repository_metadata(&tx, source_profile)?;
    }

    repo.last_sync = Some(current_timestamp());
    repo.update(&tx)?;

    tx.commit()?;

    Ok(count)
}

pub(in crate::repository) fn persist_synced_package_rows(
    conn: &Connection,
    repo_id: i64,
    synced_packages: Vec<SyncedPackageRow>,
) -> Result<usize> {
    RepositoryPackage::delete_by_repository(conn, repo_id)?;
    append_synced_package_rows(conn, synced_packages)
}

/// Append one bounded batch of normalized package rows.
///
/// Callers own replacement/transaction semantics. Sparse Remi sync uses this
/// with a fixed-size staging page so neither package rows nor their capability
/// projections accumulate in process memory for the full distribution.
pub(super) fn append_synced_package_rows(
    conn: &Connection,
    synced_packages: Vec<SyncedPackageRow>,
) -> Result<usize> {
    let count = synced_packages.len();
    for (index, row) in synced_packages.iter().enumerate() {
        if row.requirement_groups.len() != row.requirement_group_clauses.len() {
            return Err(Error::InitError(format!(
                "normalized repository row {index} has {} requirement groups but {} clause batches",
                row.requirement_groups.len(),
                row.requirement_group_clauses.len()
            )));
        }
    }

    let mut repo_packages = synced_packages
        .iter()
        .map(|row| row.package.clone())
        .collect::<Vec<_>>();
    RepositoryPackage::batch_insert_with_ids(conn, &mut repo_packages)?;

    let mut repo_provides = Vec::new();
    let mut all_groups: Vec<DbRequirementGroup> = Vec::new();
    let mut all_group_clauses: Vec<Vec<RepositoryRequirement>> = Vec::new();

    for (pkg, row) in repo_packages.iter().zip(synced_packages) {
        let Some(repository_package_id) = pkg.id else {
            return Err(Error::InitError(
                "Inserted repository package missing generated ID".to_string(),
            ));
        };

        repo_provides.extend(row.provides.into_iter().map(|mut provide| {
            provide.repository_package_id = repository_package_id;
            provide
        }));
        for mut group in row.requirement_groups {
            group.repository_package_id = repository_package_id;
            all_groups.push(group);
        }
        for mut clauses in row.requirement_group_clauses {
            for clause in &mut clauses {
                clause.repository_package_id = repository_package_id;
            }
            all_group_clauses.push(clauses);
        }
    }

    RepositoryProvide::batch_insert(conn, &repo_provides)?;

    DbRequirementGroup::batch_insert_with_ids(conn, &mut all_groups)?;
    let mut grouped_clauses = Vec::new();
    for (group, clauses) in all_groups.iter().zip(all_group_clauses) {
        let group_id = group.id.ok_or_else(|| {
            Error::InitError("Inserted requirement group missing generated ID".to_string())
        })?;
        grouped_clauses.extend(
            clauses
                .into_iter()
                .map(|clause| clause.with_group(group_id)),
        );
    }
    RepositoryRequirement::batch_insert(conn, &grouped_clauses)?;

    Ok(count)
}

/// Build the complete normalized row set for one parsed package.
///
/// Native sync and its tests share this owner so the capability projection,
/// requirement conversion, and reference-mirror rebasing that turn one parsed
/// package into persistable rows cannot drift apart.
pub(in crate::repository) fn synced_package_row(
    repo_id: i64,
    source_profile: Option<&str>,
    repo_url: &str,
    content_url: Option<&str>,
    pkg_meta: PackageMetadata,
) -> SyncedPackageRow {
    let provides = normalized_repository_capabilities(&pkg_meta);
    let download_url = super::rebase_download_url(&pkg_meta.download_url, repo_url, content_url);
    let version_scheme = pkg_meta.version_scheme;
    let checksum = match pkg_meta.checksum_type {
        crate::repository::parsers::ChecksumType::Sha1 => {
            format!("sha1:{}", pkg_meta.checksum.trim_start_matches("sha1:"))
        }
        crate::repository::parsers::ChecksumType::Sha256 => {
            format!("sha256:{}", pkg_meta.checksum.trim_start_matches("sha256:"))
        }
        crate::repository::parsers::ChecksumType::Sha512
        | crate::repository::parsers::ChecksumType::Md5 => pkg_meta.checksum,
    };
    let mut repo_pkg = RepositoryPackage::new(
        repo_id,
        pkg_meta.name,
        pkg_meta.version,
        version_scheme,
        checksum,
        pkg_meta.size as i64,
        download_url,
    );

    repo_pkg.architecture = pkg_meta.architecture;
    repo_pkg.debian_multi_arch = pkg_meta.debian_multi_arch;
    repo_pkg.description = pkg_meta.description;
    repo_pkg.metadata = match &pkg_meta.extra_metadata {
        serde_json::Value::Null => None,
        value => Some(value.to_string()),
    };

    // Persist the exact repository profile. Package format remains a separate
    // typed field and is never promoted to distro authority.
    repo_pkg.source_profile = source_profile.map(str::to_string);

    let (requirement_groups, requirement_group_clauses) =
        convert_requirement_groups(0, &pkg_meta.requirements);

    SyncedPackageRow {
        package: repo_pkg,
        provides,
        requirement_groups,
        requirement_group_clauses,
    }
}

pub(super) fn normalized_repository_capabilities(
    pkg_meta: &PackageMetadata,
) -> Vec<RepositoryProvide> {
    let scheme = pkg_meta.version_scheme;

    if !pkg_meta.provides.is_empty() {
        pkg_meta
            .provides
            .iter()
            .map(|provide| {
                let kind = capability_kind_to_db(provide.kind);
                RepositoryProvide::new(
                    0,
                    provide.name.clone(),
                    provide.version.clone(),
                    kind,
                    provide.native_text.clone(),
                    scheme,
                )
                .with_version_relation(provide.version_relation)
                .with_architecture_qualifier(provide.architecture_qualifier.clone())
                .with_provenance(provide.provenance.clone())
            })
            .collect()
    } else {
        vec![RepositoryProvide::new(
            0,
            pkg_meta.name.clone(),
            Some(pkg_meta.version.clone()),
            "package".to_string(),
            Some(pkg_meta.name.clone()),
            scheme,
        )]
    }
}

pub(in crate::repository) fn capability_kind_to_db(kind: RepositoryCapabilityKind) -> String {
    match kind {
        RepositoryCapabilityKind::PackageName => "package".to_string(),
        RepositoryCapabilityKind::Virtual => "virtual".to_string(),
        RepositoryCapabilityKind::Soname => "soname".to_string(),
        RepositoryCapabilityKind::File => "file".to_string(),
        RepositoryCapabilityKind::Path => "path".to_string(),
        RepositoryCapabilityKind::Binary => "binary".to_string(),
        RepositoryCapabilityKind::PkgConfig => "pkgconfig".to_string(),
        RepositoryCapabilityKind::PkgConfig32 => "pkgconfig32".to_string(),
        RepositoryCapabilityKind::Comar => "comar".to_string(),
        RepositoryCapabilityKind::Generic => "generic".to_string(),
    }
}

fn requirement_kind_to_db(kind: RepositoryRequirementKind) -> String {
    kind.as_str().to_string()
}

fn behavior_to_db(behavior: ConditionalRequirementBehavior) -> String {
    match behavior {
        ConditionalRequirementBehavior::Hard => "hard".to_string(),
        ConditionalRequirementBehavior::Conditional => "conditional".to_string(),
    }
}

/// Convert parser-level requirement groups into DB model groups and their linked clauses.
///
/// Returns `(groups, clauses)` where each clause has a placeholder `group_id` of 0
/// that will be fixed up after the groups are inserted with real IDs.
pub(in crate::repository) fn convert_requirement_groups(
    repository_package_id: i64,
    groups: &[RepositoryRequirementGroup],
) -> (Vec<DbRequirementGroup>, Vec<Vec<RepositoryRequirement>>) {
    let mut db_groups = Vec::with_capacity(groups.len());
    let mut clause_batches = Vec::with_capacity(groups.len());

    for (group_index, group) in groups.iter().enumerate() {
        let group_token =
            i64::try_from(group_index + 1).expect("requirement group count cannot exceed i64");
        let mut db_group = DbRequirementGroup::new(
            repository_package_id,
            requirement_kind_to_db(group.kind),
            behavior_to_db(group.behavior),
            serde_json::to_string(&group.expression)
                .expect("repository requirement expression serialization cannot fail"),
        );
        db_group.description = group.description.clone();
        db_group.native_text = group.native_text.clone();

        let clauses = group
            .alternatives
            .iter()
            .map(|clause| {
                let dependency_type = match group.kind {
                    RepositoryRequirementKind::Optional => "optional",
                    RepositoryRequirementKind::Build => "build",
                    _ => "runtime",
                };
                RepositoryRequirement::new(
                    repository_package_id,
                    group_token,
                    clause.name.clone(),
                    clause.version_constraint.clone(),
                    capability_kind_to_db(
                        clause
                            .capability_kind
                            .unwrap_or(RepositoryCapabilityKind::PackageName),
                    ),
                    dependency_type.to_string(),
                    clause.native_text.clone(),
                )
            })
            .collect();

        db_groups.push(db_group);
        clause_batches.push(clauses);
    }

    (db_groups, clause_batches)
}

#[cfg(test)]
mod append_tests {
    use super::*;
    use crate::db::testing::create_test_db;
    use crate::repository::versioning::VersionScheme;

    fn package(name: &str) -> RepositoryPackage {
        RepositoryPackage::new(
            1,
            name.to_string(),
            "1".to_string(),
            VersionScheme::Rpm,
            "checksum".to_string(),
            1,
            format!("https://repo.test/{name}"),
        )
    }

    fn synced(package: RepositoryPackage) -> SyncedPackageRow {
        SyncedPackageRow {
            package,
            provides: Vec::new(),
            requirement_groups: Vec::new(),
            requirement_group_clauses: Vec::new(),
        }
    }

    #[test]
    fn append_rejects_group_and_clause_batch_mismatch_before_writing() {
        let (_temp, conn) = create_test_db();
        let package = package("alpha");
        let mut row = synced(package);
        row.requirement_groups.push(DbRequirementGroup::new(
            0,
            "depends".to_string(),
            "hard".to_string(),
            r#"{"kind":"all","items":[]}"#.to_string(),
        ));

        let error = append_synced_package_rows(&conn, vec![row]).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("1 requirement groups but 0 clause batches")
        );
        let stored: i64 = conn
            .query_row("SELECT COUNT(*) FROM repository_packages", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored, 0);
    }
}
