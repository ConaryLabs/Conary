// apps/remi/src/server/package_metadata.rs

//! Exact repository package metadata projections shared by Remi HTTP surfaces.

use anyhow::{Context, Result, bail};
use conary_core::db::models::{
    RepositoryProvide, RepositoryRequirement, RepositoryRequirementGroup,
};
use conary_core::repository::remi_metadata::{RemiProvide, RemiRequirement, RemiRequirementGroup};
use rusqlite::Connection;

#[derive(Debug, Clone, Default)]
pub(crate) struct ExactPackageMetadata {
    pub(crate) provides: Vec<RemiProvide>,
    pub(crate) requirement_groups: Vec<RemiRequirementGroup>,
}

pub(crate) fn load_exact_package_metadata(
    conn: &Connection,
    repository_package_id: i64,
) -> Result<ExactPackageMetadata> {
    let provides = RepositoryProvide::find_by_repository_package(conn, repository_package_id)?
        .into_iter()
        .map(|provide| RemiProvide {
            capability: provide.capability,
            version: provide.version,
            kind: provide.kind,
            raw: provide.raw,
            version_scheme: provide.version_scheme,
        })
        .collect();

    let atoms = RepositoryRequirement::find_by_repository_package(conn, repository_package_id)?;
    let groups =
        RepositoryRequirementGroup::find_by_repository_package(conn, repository_package_id)?;
    let mut requirement_groups = Vec::with_capacity(groups.len());
    let mut group_ids = Vec::with_capacity(groups.len());
    for group in groups {
        let group_id = group
            .id
            .context("repository requirement group has no persisted ID")?;
        group_ids.push(group_id);
        let clauses = atoms
            .iter()
            .filter(|requirement| requirement.group_id == group_id)
            .map(|requirement| RemiRequirement {
                capability: requirement.capability.clone(),
                version_constraint: requirement.version_constraint.clone(),
                kind: requirement.kind.clone(),
                dependency_type: requirement.dependency_type.clone(),
                raw: requirement.raw.clone(),
            })
            .collect::<Vec<_>>();
        if clauses.is_empty() {
            bail!(
                "repository requirement group {group_id} for package \
                 {repository_package_id} has no searchable atoms"
            );
        }
        requirement_groups.push(RemiRequirementGroup {
            kind: group.kind,
            behavior: group.behavior,
            description: group.description,
            native_text: group.native_text,
            expression_json: group.expression_json,
            clauses,
        });
    }

    if atoms.iter().any(|atom| !group_ids.contains(&atom.group_id)) {
        bail!(
            "repository package {repository_package_id} has requirement atoms \
             outside its authoritative exact groups"
        );
    }

    Ok(ExactPackageMetadata {
        provides,
        requirement_groups,
    })
}
