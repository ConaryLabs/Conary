// crates/conary-core/src/resolver/provider/evidence.rs

//! Exact-root and typed conflict evidence helpers for resolution proof production.

use resolvo::{DenseIndex, Requirement, SolvableId, VersionSetId};

use crate::error::Result;

use super::ConaryProvider;
use super::types::{ConaryConstraint, RepositoryRequirementGroupIdentity};

impl ConaryProvider<'_> {
    pub(crate) fn set_native_architecture(&mut self, architecture: impl Into<String>) {
        self.native_architecture = architecture.into();
    }

    pub(crate) fn intern_exact_repository_package(
        &mut self,
        name: &str,
        repository_package_id: i64,
    ) -> Result<VersionSetId> {
        let name = self.intern_name(name)?;
        self.intern_conary_version_set(
            name,
            ConaryConstraint::ExactRepositoryPackage(repository_package_id),
        )
    }

    pub(crate) fn unresolved_requirement_groups(
        &self,
        solvable: SolvableId,
        requirement: Requirement,
    ) -> Vec<RepositoryRequirementGroupIdentity> {
        let version_sets = match requirement {
            Requirement::Single(version_set) => vec![version_set],
            Requirement::Union(union) => self
                .version_set_unions
                .get(union.to_index())
                .cloned()
                .unwrap_or_default(),
        };
        version_sets
            .into_iter()
            .flat_map(|version_set| {
                self.compiled_requirement_groups
                    .get(&(solvable.into_raw(), version_set.0))
                    .into_iter()
                    .flatten()
                    .copied()
            })
            .collect()
    }
}
