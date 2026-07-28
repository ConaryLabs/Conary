// apps/remi/src/server/package_metadata.rs

//! Exact repository package metadata projections shared by Remi HTTP surfaces.

use anyhow::{Context, Result, bail};
use conary_core::db::models::{
    RepositoryProvide, RepositoryRequirement, RepositoryRequirementGroup,
};
use conary_core::repository::remi_metadata::{RemiProvide, RemiRequirement, RemiRequirementGroup};
use rusqlite::Connection;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub(crate) struct ExactPackageMetadata {
    pub(crate) provides: Vec<RemiProvide>,
    pub(crate) requirement_groups: Vec<RemiRequirementGroup>,
}

pub(crate) fn load_exact_package_metadata(
    conn: &Connection,
    repository_package_id: i64,
) -> Result<ExactPackageMetadata> {
    load_exact_package_metadata_batch(conn, &[repository_package_id])?
        .remove(&repository_package_id)
        .context("requested repository package metadata was not loaded")
}

pub(crate) fn load_exact_package_metadata_batch(
    conn: &Connection,
    repository_package_ids: &[i64],
) -> Result<HashMap<i64, ExactPackageMetadata>> {
    let mut metadata = repository_package_ids
        .iter()
        .copied()
        .map(|package_id| (package_id, ExactPackageMetadata::default()))
        .collect::<HashMap<_, _>>();

    for provide in RepositoryProvide::find_by_repository_packages(conn, repository_package_ids)? {
        let package_metadata = metadata
            .get_mut(&provide.repository_package_id)
            .context("repository provide belongs to an unrequested package")?;
        package_metadata.provides.push(RemiProvide {
            capability: provide.capability,
            version: provide.version,
            version_relation: provide.version_relation,
            kind: provide.kind,
            raw: provide.raw,
            version_scheme: provide.version_scheme,
            architecture_qualifier: provide.architecture_qualifier,
        });
    }

    let atoms = RepositoryRequirement::find_by_repository_packages(conn, repository_package_ids)?;
    let mut atoms_by_group = HashMap::<i64, Vec<RepositoryRequirement>>::new();
    for atom in atoms {
        atoms_by_group.entry(atom.group_id).or_default().push(atom);
    }

    for group in
        RepositoryRequirementGroup::find_by_repository_packages(conn, repository_package_ids)?
    {
        let group_id = group
            .id
            .context("repository requirement group has no persisted ID")?;
        let atoms = atoms_by_group.remove(&group_id).unwrap_or_default();
        if atoms.is_empty() {
            bail!(
                "repository requirement group {group_id} for package {} has no searchable atoms",
                group.repository_package_id
            );
        }
        if atoms
            .iter()
            .any(|atom| atom.repository_package_id != group.repository_package_id)
        {
            bail!("repository requirement group {group_id} crosses package ownership boundaries");
        }
        let clauses = atoms
            .into_iter()
            .map(|requirement| RemiRequirement {
                capability: requirement.capability,
                version_constraint: requirement.version_constraint,
                kind: requirement.kind,
                dependency_type: requirement.dependency_type,
                raw: requirement.raw,
            })
            .collect();
        metadata
            .get_mut(&group.repository_package_id)
            .context("repository requirement group belongs to an unrequested package")?
            .requirement_groups
            .push(RemiRequirementGroup {
                kind: group.kind,
                behavior: group.behavior,
                description: group.description,
                native_text: group.native_text,
                expression_json: group.expression_json,
                clauses,
            });
    }

    if let Some((group_id, atoms)) = atoms_by_group.into_iter().next() {
        let package_id = atoms
            .first()
            .map(|atom| atom.repository_package_id)
            .unwrap_or_default();
        bail!(
            "repository package {package_id} has requirement atoms outside authoritative group \
             {group_id}"
        );
    }

    Ok(metadata)
}
