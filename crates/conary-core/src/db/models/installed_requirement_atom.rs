// conary-core/src/db/models/installed_requirement_atom.rs

//! Read-only atom projection of installed exact requirement groups.
//!
//! Package mutation and resolution use [`InstalledRequirementGroup`]. This
//! projection exists only for human-facing dependency queries that need one
//! row per referenced capability; it has no independently persisted table.

use super::installed_requirement_group::InstalledRequirementGroup;
use super::trove::Trove;
use crate::error::Result;
use crate::repository::dependency_model::{RepositoryCapabilityKind, RepositoryRequirementKind};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

/// One searchable atom projected from an exact installed requirement group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRequirementAtom {
    /// Exact group ID. Multiple atoms in one Boolean expression share this ID.
    pub id: Option<i64>,
    pub trove_id: i64,
    pub depends_on_name: String,
    pub depends_on_version: Option<String>,
    pub dependency_type: String,
    pub version_constraint: Option<String>,
    pub kind: String,
    pub group_id: Option<i64>,
}

impl InstalledRequirementAtom {
    /// Project every positive requirement atom for one installed trove.
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        Ok(InstalledRequirementGroup::find_by_trove(conn, trove_id)?
            .into_iter()
            .flat_map(project_group)
            .collect())
    }

    /// Batch-project positive requirement atoms for installed troves.
    pub fn find_by_troves(conn: &Connection, trove_ids: &[i64]) -> Result<HashMap<i64, Vec<Self>>> {
        let wanted = trove_ids.iter().copied().collect::<HashSet<_>>();
        let mut result: HashMap<i64, Vec<Self>> = HashMap::new();
        for group in InstalledRequirementGroup::list_all(conn)? {
            if wanted.contains(&group.trove_id) {
                result
                    .entry(group.trove_id)
                    .or_default()
                    .extend(project_group(group));
            }
        }
        Ok(result)
    }

    /// Find installed requirements whose exact expression mentions a capability.
    ///
    /// This is a query projection, not an assertion that every Boolean branch
    /// currently requires the named capability.
    pub fn find_dependents(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        Ok(InstalledRequirementGroup::list_all(conn)?
            .into_iter()
            .flat_map(project_group)
            .filter(|entry| entry.depends_on_name == capability)
            .collect())
    }

    /// Find installed requirements mentioning one exact capability kind/name.
    pub fn find_typed_dependents(conn: &Connection, kind: &str, name: &str) -> Result<Vec<Self>> {
        Ok(Self::find_dependents(conn, name)?
            .into_iter()
            .filter(|entry| entry.kind == kind)
            .collect())
    }

    /// Find projected atoms of one capability kind for a trove.
    pub fn find_by_trove_and_kind(
        conn: &Connection,
        trove_id: i64,
        kind: &str,
    ) -> Result<Vec<Self>> {
        Ok(Self::find_by_trove(conn, trove_id)?
            .into_iter()
            .filter(|entry| entry.kind == kind)
            .collect())
    }

    /// Find package-name providers for a diagnostic dependency atom.
    pub fn find_providers(conn: &Connection, dependency_name: &str) -> Result<Vec<Trove>> {
        Trove::find_by_name(conn, dependency_name)
    }

    /// Format this projected atom as a typed string.
    pub fn to_typed_string(&self) -> String {
        if self.kind == "package" {
            match &self.version_constraint {
                Some(version) => format!("{}{}", self.depends_on_name, version),
                None => self.depends_on_name.clone(),
            }
        } else {
            match &self.version_constraint {
                Some(version) => format!("{}({}{})", self.kind, self.depends_on_name, version),
                None => format!("{}({})", self.kind, self.depends_on_name),
            }
        }
    }
}

fn project_group(group: InstalledRequirementGroup) -> Vec<InstalledRequirementAtom> {
    let Some(dependency_type) = positive_dependency_type(group.kind) else {
        return Vec::new();
    };
    let group_id = group.id;
    group
        .requirement
        .expression
        .atoms()
        .into_iter()
        .map(|atom| InstalledRequirementAtom {
            id: group_id,
            trove_id: group.trove_id,
            depends_on_name: atom.name.clone(),
            depends_on_version: None,
            dependency_type: dependency_type.to_string(),
            version_constraint: atom.version_constraint.clone(),
            kind: capability_kind(atom.capability_kind),
            group_id,
        })
        .collect()
}

fn positive_dependency_type(kind: RepositoryRequirementKind) -> Option<&'static str> {
    match kind {
        RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends => {
            Some("runtime")
        }
        RepositoryRequirementKind::Optional => Some("optional"),
        RepositoryRequirementKind::Build => Some("build"),
        RepositoryRequirementKind::Conflict
        | RepositoryRequirementKind::Breaks
        | RepositoryRequirementKind::Replace
        | RepositoryRequirementKind::Obsolete => None,
    }
}

fn capability_kind(kind: Option<RepositoryCapabilityKind>) -> String {
    match kind.unwrap_or(RepositoryCapabilityKind::PackageName) {
        RepositoryCapabilityKind::PackageName => "package",
        RepositoryCapabilityKind::Virtual => "virtual",
        RepositoryCapabilityKind::Soname => "soname",
        RepositoryCapabilityKind::File => "file",
        RepositoryCapabilityKind::Path => "path",
        RepositoryCapabilityKind::Binary => "binary",
        RepositoryCapabilityKind::PkgConfig => "pkgconfig",
        RepositoryCapabilityKind::Generic => "generic",
    }
    .to_string()
}
