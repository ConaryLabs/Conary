// crates/conary-core/src/resolver/provider/borrowed.rs

//! Borrow loaded facts across fresh SAT caches. All decisions delegate to the
//! owning provider; only the cache lifetime changes for missing-first probes.

use super::ConaryProvider;
use resolvo::{
    Candidates, Condition, ConditionId, Dependencies, DependencyProvider, Interner, NameId,
    SolvableId, SolverCache, StringId, VersionSetId, VersionSetUnionId,
};
use std::fmt::Display;

impl Interner for &ConaryProvider<'_> {
    type NameId = NameId;
    type SolvableId = SolvableId;
    fn display_solvable(&self, id: SolvableId) -> impl Display + '_ {
        (**self).display_solvable(id)
    }
    fn display_name(&self, id: NameId) -> impl Display + '_ {
        (**self).display_name(id)
    }
    fn display_version_set(&self, id: VersionSetId) -> impl Display + '_ {
        (**self).display_version_set(id)
    }
    fn display_string(&self, id: StringId) -> impl Display + '_ {
        (**self).display_string(id)
    }
    fn version_set_name(&self, id: VersionSetId) -> NameId {
        (**self).version_set_name(id)
    }
    fn solvable_name(&self, id: SolvableId) -> NameId {
        (**self).solvable_name(id)
    }
    fn version_sets_in_union(&self, id: VersionSetUnionId) -> impl Iterator<Item = VersionSetId> {
        (**self).version_sets_in_union(id)
    }
    fn resolve_condition(&self, id: ConditionId) -> Condition {
        (**self).resolve_condition(id)
    }
}

impl DependencyProvider for &ConaryProvider<'_> {
    async fn filter_candidates(
        &self,
        candidates: &[SolvableId],
        version_set: VersionSetId,
        inverse: bool,
    ) -> Vec<SolvableId> {
        (**self)
            .filter_candidates(candidates, version_set, inverse)
            .await
    }
    async fn get_candidates(&self, name: NameId) -> Option<Candidates> {
        (**self).get_candidates(name).await
    }
    async fn sort_candidates(&self, _solver: &SolverCache<Self>, solvables: &mut [SolvableId]) {
        self.sort_solvables(solvables);
    }
    async fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
        (**self).get_dependencies(solvable).await
    }
    fn should_cancel_with_value(&self) -> Option<Box<dyn std::any::Any>> {
        (**self).should_cancel_with_value()
    }
}
