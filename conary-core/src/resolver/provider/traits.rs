// conary-core/src/resolver/provider/traits.rs

//! resolvo trait implementations for `ConaryProvider`.
//!
//! Implements the `Interner` and `DependencyProvider` traits that bridge
//! Conary's data model to resolvo's SAT solver interface.

use std::fmt;

use resolvo::{
    Candidates, Condition, ConditionId, ConditionalRequirement, Dependencies, DependencyProvider,
    HintDependenciesAvailable, Interner, KnownDependencies, NameId, SolvableId, SolverCache,
    StringId, VersionSetId, VersionSetUnionId,
};

use super::ConaryProvider;
use super::matching::{constraint_matches_package, constraint_matches_provide};
use super::types::{ConaryConstraint, ConaryPackageVersion, SolverDep};

// --- Display helpers ---

struct DisplayName<'a>(&'a str);
impl fmt::Display for DisplayName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

struct DisplaySolvable<'a> {
    name: &'a str,
    version: &'a ConaryPackageVersion,
}
impl fmt::Display for DisplaySolvable<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.name, self.version)
    }
}

struct DisplayVersionSet<'a>(&'a ConaryConstraint);
impl fmt::Display for DisplayVersionSet<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

struct DisplayString<'a>(&'a str);
impl fmt::Display for DisplayString<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// --- Interner implementation ---

impl Interner for ConaryProvider<'_> {
    fn display_solvable(&self, solvable: SolvableId) -> impl fmt::Display + '_ {
        let pkg = &self.solvables[solvable.0 as usize];
        DisplaySolvable {
            name: &pkg.name,
            version: &pkg.version,
        }
    }

    fn display_name(&self, name: NameId) -> impl fmt::Display + '_ {
        DisplayName(&self.names[name.0 as usize])
    }

    fn display_version_set(&self, version_set: VersionSetId) -> impl fmt::Display + '_ {
        DisplayVersionSet(&self.version_sets[version_set.0 as usize].1)
    }

    fn display_string(&self, string_id: StringId) -> impl fmt::Display + '_ {
        DisplayString(&self.strings[string_id.0 as usize])
    }

    fn version_set_name(&self, version_set: VersionSetId) -> NameId {
        self.version_sets[version_set.0 as usize].0
    }

    fn solvable_name(&self, solvable: SolvableId) -> NameId {
        let pkg = &self.solvables[solvable.0 as usize];
        self.name_to_id[&pkg.name]
    }

    fn version_sets_in_union(
        &self,
        version_set_union: VersionSetUnionId,
    ) -> impl Iterator<Item = VersionSetId> {
        self.version_set_unions
            .get(version_set_union.0 as usize)
            .cloned()
            .unwrap_or_default()
            .into_iter()
    }

    fn resolve_condition(&self, _condition: ConditionId) -> Condition {
        // ConaryProvider does not use conditions; return a permissive default
        // rather than panicking if resolvo ever calls this unexpectedly.
        Condition::Requirement(VersionSetId::default())
    }
}

// --- DependencyProvider implementation ---

impl DependencyProvider for ConaryProvider<'_> {
    async fn filter_candidates(
        &self,
        candidates: &[SolvableId],
        version_set: VersionSetId,
        inverse: bool,
    ) -> Vec<SolvableId> {
        let (name_id, ref constraint) = self.version_sets[version_set.0 as usize];
        let requested_name = &self.names[name_id.0 as usize];
        candidates
            .iter()
            .copied()
            .filter(|&sid| {
                let pkg = &self.solvables[sid.0 as usize];
                let matches = if pkg.name == *requested_name {
                    constraint_matches_package(constraint, &pkg.version)
                } else if let Some(provided_version) = pkg
                    .provided_capabilities
                    .iter()
                    .find(|(capability, _version)| capability == requested_name)
                    .and_then(|(_capability, version)| version.as_ref())
                {
                    constraint_matches_provide(constraint, Some(provided_version), &pkg.version)
                } else {
                    constraint_matches_provide(constraint, None, &pkg.version)
                };
                if inverse { !matches } else { matches }
            })
            .collect()
    }

    async fn get_candidates(&self, name: NameId) -> Option<Candidates> {
        let mut candidates = self.solvables_for_name(name);

        if candidates.is_empty() {
            let name_str = &self.names[name.0 as usize];
            let providers = self.resolve_virtual_provide(name_str);
            for provider_name in &providers {
                if let Some(&provider_name_id) = self.name_to_id.get(provider_name) {
                    candidates.extend(self.solvables_for_name(provider_name_id));
                }
            }
            candidates.extend(self.solvables_for_provide(name_str));

            // Canonical fallback: check cross-distro equivalents when all
            // other lookup strategies fail. E.g. 'libssl3' -> 'openssl'.
            if candidates.is_empty() {
                for equiv in self.canonical_equivalents(name_str) {
                    if let Some(&equiv_name_id) = self.name_to_id.get(equiv) {
                        let equiv_candidates = self.solvables_for_name(equiv_name_id);
                        if !equiv_candidates.is_empty() {
                            tracing::debug!("Canonical fallback: {} -> {}", name_str, equiv);
                            candidates.extend(equiv_candidates);
                            break;
                        }
                    }
                }
            }

            if candidates.is_empty() {
                return None;
            }
        }

        let favored = self.installed_solvable_for_name(name);

        Some(Candidates {
            candidates,
            favored,
            locked: None,
            hint_dependencies_available: HintDependenciesAvailable::All,
            excluded: Vec::new(),
        })
    }

    async fn sort_candidates(&self, _solver: &SolverCache<Self>, solvables: &mut [SolvableId]) {
        solvables.sort_by(|a, b| {
            let pkg_a = &self.solvables[a.0 as usize];
            let pkg_b = &self.solvables[b.0 as usize];

            if let Some(version_cmp) =
                super::matching::compare_package_versions_desc(&pkg_a.version, &pkg_b.version)
                && version_cmp != std::cmp::Ordering::Equal
            {
                return version_cmp;
            }

            let a_installed = pkg_a.trove_id.is_some();
            let b_installed = pkg_b.trove_id.is_some();
            b_installed
                .cmp(&a_installed)
                .then_with(|| pkg_a.name.cmp(&pkg_b.name))
        });
    }

    async fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
        let mut requirements = Vec::new();

        if let Some(dep_list) = self.dependencies.get(&solvable.0) {
            for dep in dep_list {
                match dep {
                    SolverDep::Single(dep_name, constraint) => {
                        if let Some(req) = self.lookup_requirement(dep_name, constraint) {
                            requirements.push(req);
                        }
                    }
                    SolverDep::OrGroup(alternatives) => {
                        let mut vs_ids = Vec::new();
                        for (dep_name, constraint) in alternatives {
                            if let Some(&dep_name_id) = self.name_to_id.get(dep_name) {
                                let cache_key = (dep_name_id.0, constraint.clone());
                                if let Some(&vs_id) = self.version_set_cache.get(&cache_key) {
                                    vs_ids.push(vs_id);
                                }
                            }
                        }
                        if vs_ids.len() == 1 {
                            // Single-alternative OR group: emit as a simple requirement
                            requirements.push(ConditionalRequirement::from(vs_ids[0]));
                        } else if vs_ids.len() > 1 {
                            // Look up the union ID from our pool
                            if let Some(union_id) = self.find_union_id(&vs_ids) {
                                requirements.push(ConditionalRequirement::from(union_id));
                            }
                        }
                    }
                }
            }
        }

        Dependencies::Known(KnownDependencies {
            requirements,
            constrains: Vec::new(),
        })
    }
}
