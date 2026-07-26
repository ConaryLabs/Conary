// conary-core/src/resolver/provider/traits.rs

//! resolvo trait implementations for `ConaryProvider`.
//!
//! Implements the `Interner` and `DependencyProvider` traits that bridge
//! Conary's data model to resolvo's SAT solver interface.
//! Solvables are now `PackageIdentity` instances.

use std::fmt;

use resolvo::{
    Candidates, Condition, ConditionId, Dependencies, DependencyProvider,
    HintDependenciesAvailable, Interner, KnownDependencies, NameId, SolvableId, SolverCache,
    StringId, VersionSetId, VersionSetUnionId,
};

use super::ConaryProvider;
use super::matching::{
    constraint_architecture_matches_package, constraint_architecture_matches_provide,
    constraint_matches_package, constraint_matches_provide, provider_expression_matches_package,
};
use super::types::ConaryConstraint;

// --- Display helpers ---

struct DisplayName<'a>(&'a str);
impl fmt::Display for DisplayName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

struct DisplaySolvable<'a> {
    name: &'a str,
    version: &'a str,
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
            .expect("resolvo requested a version-set union minted by this provider")
            .clone()
            .into_iter()
    }

    fn resolve_condition(&self, condition: ConditionId) -> Condition {
        self.conditions[condition.as_u32() as usize].clone()
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
        let requested_kind = match constraint {
            ConaryConstraint::Repository {
                capability_kind, ..
            } => *capability_kind,
            ConaryConstraint::Requested(_)
            | ConaryConstraint::RpmRuntime(_)
            | ConaryConstraint::ProviderExpression { .. } => None,
        };
        candidates
            .iter()
            .copied()
            .filter(|&sid| {
                let pkg = &self.solvables[sid.0 as usize];
                let matches = match constraint {
                    ConaryConstraint::RpmRuntime(_) => false,
                    ConaryConstraint::ProviderExpression { expression } => {
                        constraint_architecture_matches_package(
                            constraint,
                            pkg,
                            &self.native_architecture,
                        ) && provider_expression_matches_package(expression, pkg)
                            .expect("resolver package versions are validated before SAT filtering")
                    }
                    _ if matches!(
                        requested_kind,
                        None
                            | Some(
                                crate::repository::dependency_model::RepositoryCapabilityKind::PackageName
                            )
                    ) && (pkg.name == *requested_name
                        || self
                            .canonical_equivalents(requested_name)
                            .iter()
                            .any(|equivalent| equivalent == &pkg.name)) =>
                    {
                        constraint_architecture_matches_package(
                            constraint,
                            pkg,
                            &self.native_architecture,
                        ) && constraint_matches_package(
                            constraint,
                            &pkg.version,
                            pkg.version_scheme,
                        )
                        .expect("resolver package versions are validated before SAT filtering")
                    }
                    _ => pkg
                        .provided_capabilities
                        .iter()
                        .filter(|capability| {
                            capability.name == *requested_name
                                && requested_kind.is_none_or(|kind| capability.kind == kind)
                        })
                        .any(|capability| {
                            constraint_architecture_matches_provide(
                                constraint,
                                pkg,
                                &capability.architecture_qualifier,
                                capability.version_scheme,
                                &self.native_architecture,
                            ) && constraint_matches_provide(
                                constraint,
                                capability.version.as_deref(),
                                capability.version_relation,
                                capability.version_scheme,
                            )
                            .expect(
                                "resolver capability versions are validated before SAT filtering",
                            )
                        }),
                };
                if inverse { !matches } else { matches }
            })
            .collect()
    }

    async fn get_candidates(&self, name: NameId) -> Option<Candidates> {
        let name_str = &self.names[name.0 as usize];
        if name_str == "\0conary:false" {
            return None;
        }
        if self.provider_expression_name_ids.contains(&name.0) {
            let candidates = self.solvable_ids.clone();
            return (!candidates.is_empty()).then_some(Candidates {
                candidates,
                favored: None,
                locked: None,
                hint_dependencies_available: HintDependenciesAvailable::All,
                excluded: Vec::new(),
            });
        }
        let mut candidates = self.solvables_for_name(name);

        // Always include canonical equivalents so the solver can fall back to
        // them when version constraints filter out all exact-name candidates.
        // sort_candidates ranks exact-name matches above canonical ones.
        for equiv in self.canonical_equivalents(name_str) {
            if let Some(&equiv_name_id) = self.name_to_id.get(equiv) {
                let equiv_candidates = self.solvables_for_name(equiv_name_id);
                if !equiv_candidates.is_empty() {
                    tracing::debug!(
                        "Canonical candidates for {}: {} ({} candidates)",
                        name_str,
                        equiv,
                        equiv_candidates.len()
                    );
                    candidates.extend(equiv_candidates);
                }
            }
        }

        // Virtual providers were resolved from typed repository/installed
        // provide rows during provider construction. SAT callbacks only
        // consume those preloaded facts and never fall back to metadata text.
        candidates.extend(self.solvables_for_provide(name_str));

        if candidates.is_empty() {
            return None;
        }

        let favored = self.installed_solvable_for_name(name);

        // If the package is pinned (troves.pinned = 1), lock the solver to
        // the installed version so the SAT solver cannot choose a different
        // version.  This implements G3: respect per-package version pins.
        let locked = candidates
            .iter()
            .find(|&&sid| {
                let pkg = &self.solvables[sid.0 as usize];
                pkg.name == *name_str && pkg.installed_pinned
            })
            .copied();

        Some(Candidates {
            candidates,
            favored,
            locked,
            hint_dependencies_available: HintDependenciesAvailable::All,
            excluded: Vec::new(),
        })
    }

    async fn sort_candidates(&self, _solver: &SolverCache<Self>, solvables: &mut [SolvableId]) {
        // Determine the "primary" name: the first solvable's name is assumed to
        // be the exact-name match. Canonical equivalents have different names and
        // should sort after exact-name candidates.
        let primary_name = solvables
            .first()
            .map(|s| self.solvables[s.0 as usize].name.as_str());

        solvables.sort_by(|a, b| {
            let pkg_a = &self.solvables[a.0 as usize];
            let pkg_b = &self.solvables[b.0 as usize];

            // Exact-name candidates sort before canonical fallbacks
            if let Some(primary) = primary_name {
                let a_exact = pkg_a.name == primary;
                let b_exact = pkg_b.name == primary;
                if a_exact != b_exact {
                    return b_exact.cmp(&a_exact);
                }
            }

            // Higher repository priority is preferred
            if pkg_a.repository_priority != pkg_b.repository_priority {
                return pkg_b.repository_priority.cmp(&pkg_a.repository_priority);
            }

            if pkg_a.version_scheme == pkg_b.version_scheme {
                let version_cmp = crate::repository::versioning::compare_package_identities(
                    pkg_b.version_scheme,
                    &pkg_b.version,
                    pkg_b.package_release.as_deref(),
                    pkg_a.version_scheme,
                    &pkg_a.version,
                    pkg_a.package_release.as_deref(),
                )
                .expect(
                    "resolver package versions and releases are validated before candidate sorting",
                );
                if version_cmp != std::cmp::Ordering::Equal {
                    return version_cmp;
                }
            }

            let a_installed = pkg_a.installed_trove_id.is_some();
            let b_installed = pkg_b.installed_trove_id.is_some();
            b_installed
                .cmp(&a_installed)
                .then_with(|| pkg_a.name.cmp(&pkg_b.name))
                .then_with(|| pkg_a.repository_name.cmp(&pkg_b.repository_name))
        });
    }

    async fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
        match self.compiled_dependencies.get(&solvable.0) {
            Some(requirements) => Dependencies::Known(KnownDependencies {
                requirements: requirements.clone(),
                constrains: Vec::new(),
            }),
            None => Dependencies::Unknown(self.missing_dependency_authority),
        }
    }
}
