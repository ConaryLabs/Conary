// conary-core/src/repository/sync/native.rs

use crate::db::models::{
    Repository, RepositoryPackage, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup as DbRequirementGroup,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::{
    ConditionalRequirementBehavior, RepositoryCapabilityKind, RepositoryDependencyFlavor,
    RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::parsers::{DependencyType, PackageMetadata};
use crate::repository::versioning::VersionScheme;
use rusqlite::Connection;

use super::{current_timestamp, link_canonical_ids};

/// A single synced package row with all its normalized capability data.
#[derive(Debug, Clone)]
pub(super) struct SyncedPackageRow {
    pub(super) package: RepositoryPackage,
    pub(super) provides: Vec<RepositoryProvide>,
    pub(super) requirements: Vec<RepositoryRequirement>,
    pub(super) requirement_groups: Vec<DbRequirementGroup>,
    pub(super) requirement_group_clauses: Vec<Vec<RepositoryRequirement>>,
}

pub(super) fn persist_native_sync_rows(
    conn: &Connection,
    repo: &mut Repository,
    repo_packages: &mut [RepositoryPackage],
    synced_packages: Vec<SyncedPackageRow>,
) -> Result<usize> {
    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let count = synced_packages.len();

    let tx = conn.unchecked_transaction()?;

    RepositoryPackage::delete_by_repository(&tx, repo_id)?;
    RepositoryPackage::batch_insert_with_ids(&tx, repo_packages)?;

    let mut repo_provides = Vec::new();
    let mut repo_requirements = Vec::new();
    let mut all_groups: Vec<DbRequirementGroup> = Vec::new();
    let mut all_group_clauses: Vec<Vec<RepositoryRequirement>> = Vec::new();

    for (pkg, row) in repo_packages.iter().zip(synced_packages.into_iter()) {
        let Some(repository_package_id) = pkg.id else {
            return Err(Error::InitError(
                "Inserted repository package missing generated ID".to_string(),
            ));
        };

        repo_provides.extend(row.provides.into_iter().map(|mut provide| {
            provide.repository_package_id = repository_package_id;
            provide
        }));
        repo_requirements.extend(row.requirements.into_iter().map(|mut requirement| {
            requirement.repository_package_id = repository_package_id;
            requirement
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

    RepositoryProvide::batch_insert(&tx, &repo_provides)?;
    RepositoryRequirement::batch_insert(&tx, &repo_requirements)?;

    DbRequirementGroup::batch_insert_with_ids(&tx, &mut all_groups)?;
    let mut grouped_clauses = Vec::new();
    for (group, clauses) in all_groups.iter().zip(all_group_clauses.into_iter()) {
        let group_id = group.id.ok_or_else(|| {
            Error::InitError("Inserted requirement group missing generated ID".to_string())
        })?;
        grouped_clauses.extend(
            clauses
                .into_iter()
                .map(|clause| clause.with_group(group_id)),
        );
    }
    RepositoryRequirement::batch_insert(&tx, &grouped_clauses)?;

    link_canonical_ids(&tx, repo_id)?;

    repo.last_sync = Some(current_timestamp());
    repo.update(&tx)?;

    tx.commit()?;

    Ok(count)
}

pub(super) fn normalized_repository_capabilities(
    pkg_meta: &PackageMetadata,
) -> (Vec<RepositoryProvide>, Vec<RepositoryRequirement>) {
    let scheme_str = pkg_meta.version_scheme.map(version_scheme_to_db);

    let provides = if !pkg_meta.provides.is_empty() {
        pkg_meta
            .provides
            .iter()
            .map(|provide| {
                let kind = capability_kind_to_db(provide.kind);
                let mut db_provide = RepositoryProvide::new(
                    0,
                    provide.name.clone(),
                    provide.version.clone(),
                    kind,
                    provide.native_text.clone(),
                );
                if let Some(ref scheme) = scheme_str {
                    db_provide = db_provide.with_version_scheme(scheme.clone());
                }
                db_provide
            })
            .collect()
    } else {
        let mut self_provide = RepositoryProvide::new(
            0,
            pkg_meta.name.clone(),
            Some(pkg_meta.version.clone()),
            "package".to_string(),
            Some(pkg_meta.name.clone()),
        );
        if let Some(ref scheme) = scheme_str {
            self_provide = self_provide.with_version_scheme(scheme.clone());
        }
        let mut fallback = vec![self_provide];

        fallback.extend(
            extract_extra_metadata_provides(&pkg_meta.extra_metadata)
                .into_iter()
                .map(|(capability, version, raw)| {
                    let mut provide = RepositoryProvide::new(
                        0,
                        capability,
                        version,
                        "package".to_string(),
                        Some(raw),
                    );
                    if let Some(ref scheme) = scheme_str {
                        provide = provide.with_version_scheme(scheme.clone());
                    }
                    provide
                }),
        );

        fallback
    };

    let requirements = pkg_meta
        .dependencies
        .iter()
        .map(|dependency| {
            let version_constraint = dependency
                .constraint
                .as_deref()
                .filter(|constraint| !constraint.is_empty())
                .map(String::from);

            let raw = match &version_constraint {
                Some(constraint) => format!("{} {constraint}", dependency.name),
                None => dependency.name.clone(),
            };

            let dependency_type = match dependency.dep_type {
                DependencyType::Runtime => "runtime",
                DependencyType::Optional => "optional",
                DependencyType::Build => "build",
            };

            RepositoryRequirement::new(
                0,
                dependency.name.clone(),
                version_constraint,
                "package".to_string(),
                dependency_type.to_string(),
                Some(raw),
            )
        })
        .collect();

    (provides, requirements)
}

pub(super) fn extract_extra_metadata_provides(
    metadata: &serde_json::Value,
) -> Vec<(String, Option<String>, String)> {
    let mut parsed = Vec::new();

    for key in ["rpm_provides", "deb_provides", "arch_provides"] {
        let Some(entries) = metadata.get(key).and_then(|value| value.as_array()) else {
            continue;
        };

        for raw in entries.iter().filter_map(|value| value.as_str()) {
            let (capability, version) = parse_metadata_provide_entry(raw);
            parsed.push((capability, version, raw.to_string()));
        }
    }

    parsed
}

/// Split a string like `"name OP version"` on the first version-constraint
/// operator. Returns `(name, operator, version)` or `None` if no operator
/// is found.
pub(super) fn split_on_version_op(entry: &str) -> Option<(String, &'static str, String)> {
    const OPS: [&str; 5] = ["<=", ">=", "=", "<", ">"];

    for op in OPS {
        if let Some((name, version)) = entry.split_once(op) {
            let name = name.trim();
            let version = version.trim();
            if name.is_empty() || version.is_empty() {
                continue;
            }
            return Some((name.to_string(), op, version.to_string()));
        }
    }

    None
}

pub(super) fn parse_metadata_provide_entry(entry: &str) -> (String, Option<String>) {
    match split_on_version_op(entry) {
        Some((name, _, version)) => (name, Some(version)),
        None => (entry.trim().to_string(), None),
    }
}

pub(super) fn distro_flavor_to_db(flavor: RepositoryDependencyFlavor) -> String {
    match flavor {
        RepositoryDependencyFlavor::Rpm => "rpm".to_string(),
        RepositoryDependencyFlavor::Deb => "deb".to_string(),
        RepositoryDependencyFlavor::Arch => "arch".to_string(),
    }
}

pub(super) fn version_scheme_to_db(scheme: VersionScheme) -> String {
    match scheme {
        VersionScheme::Rpm => "rpm".to_string(),
        VersionScheme::Debian => "debian".to_string(),
        VersionScheme::Arch => "arch".to_string(),
    }
}

fn capability_kind_to_db(kind: RepositoryCapabilityKind) -> String {
    match kind {
        RepositoryCapabilityKind::PackageName => "package".to_string(),
        RepositoryCapabilityKind::Virtual => "virtual".to_string(),
        RepositoryCapabilityKind::Soname => "soname".to_string(),
        RepositoryCapabilityKind::File => "file".to_string(),
        RepositoryCapabilityKind::Generic => "generic".to_string(),
    }
}

fn requirement_kind_to_db(kind: RepositoryRequirementKind) -> String {
    match kind {
        RepositoryRequirementKind::Depends => "depends".to_string(),
        RepositoryRequirementKind::PreDepends => "pre_depends".to_string(),
        RepositoryRequirementKind::Optional => "optional".to_string(),
        RepositoryRequirementKind::Build => "build".to_string(),
        RepositoryRequirementKind::Conflict => "conflict".to_string(),
        RepositoryRequirementKind::Breaks => "breaks".to_string(),
    }
}

fn behavior_to_db(behavior: ConditionalRequirementBehavior) -> String {
    match behavior {
        ConditionalRequirementBehavior::Hard => "hard".to_string(),
        ConditionalRequirementBehavior::Conditional => "conditional".to_string(),
        ConditionalRequirementBehavior::UnsupportedRich => "unsupported_rich".to_string(),
    }
}

/// Convert parser-level requirement groups into DB model groups and their linked clauses.
///
/// Returns `(groups, clauses)` where each clause has a placeholder `group_id` of 0
/// that will be fixed up after the groups are inserted with real IDs.
pub(super) fn convert_requirement_groups(
    repository_package_id: i64,
    groups: &[RepositoryRequirementGroup],
) -> (Vec<DbRequirementGroup>, Vec<Vec<RepositoryRequirement>>) {
    let mut db_groups = Vec::with_capacity(groups.len());
    let mut clause_batches = Vec::with_capacity(groups.len());

    for group in groups {
        let mut db_group = DbRequirementGroup::new(
            repository_package_id,
            requirement_kind_to_db(group.kind),
            behavior_to_db(group.behavior),
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
                    clause.name.clone(),
                    clause.version_constraint.clone(),
                    "package".to_string(),
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
