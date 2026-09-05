// crates/conary-core/src/resolver/sat/hidden_conflict.rs

//! One bounded missing-first probe; loaded facts survive fresh SAT caches.

use super::*;
use crate::resolver::provider::types::RepositoryRequirementGroupIdentity;
use std::time::Instant;

/// Re-solves exclude the initial, unmodified exact-root solve.
pub(super) const MAX_HIDDEN_CONFLICT_RESOLVES: u32 = 64;
pub(super) const HIDDEN_CONFLICT_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

struct ProbeBudget<'a> {
    root: &'a str,
    start: Instant,
    resolves: u32,
}

impl ProbeBudget<'_> {
    fn exceeded(&self, now: Instant) -> Error {
        Error::HiddenConflictProbeBudgetExceeded {
            root: self.root.to_string(),
            resolves: self.resolves,
            elapsed: now.duration_since(self.start),
        }
    }

    fn check_time_at(&self, now: Instant) -> Result<()> {
        if now.duration_since(self.start) >= HIDDEN_CONFLICT_PROBE_TIMEOUT {
            return Err(self.exceeded(now));
        }
        Ok(())
    }

    fn check_time(&self) -> Result<()> {
        self.check_time_at(Instant::now())
    }

    fn before_resolve(&mut self) -> Result<()> {
        self.check_time()?;
        if self.resolves == MAX_HIDDEN_CONFLICT_RESOLVES {
            return Err(self.exceeded(Instant::now()));
        }
        self.resolves += 1;
        #[cfg(test)]
        COUNTS.with(|counts| counts.set((self.resolves, counts.get().1)));
        Ok(())
    }
}

/// None means conflicting; Some retains all exact missing groups discovered.
pub(super) fn probe(
    conn: &Connection,
    root_name: &str,
    repository_package_id: i64,
    architecture: &str,
    policy: &ResolutionPolicy,
    initial_missing: &[SatUnresolvedDependency],
) -> Result<Option<Vec<SatUnresolvedDependency>>> {
    let mut budget = ProbeBudget {
        root: root_name,
        start: Instant::now(),
        resolves: 0,
    };
    let requests = vec![(root_name.to_string(), VersionConstraint::Any)];
    let mut ignored = initial_missing
        .iter()
        .map(|dependency| RepositoryRequirementGroupIdentity {
            repository_package_id: dependency.repository_package_id,
            repository_requirement_group_id: dependency.repository_requirement_group_id,
        })
        .collect::<BTreeSet<_>>();
    budget.check_time()?;
    // Load once for the entire probe, not once per minimized missing group.
    // A fresh solver/cache borrows these facts after each monotonic discharge.
    let loaded = install::build_provider_for_install_ignoring_groups(
        conn,
        &requests,
        policy,
        ignored.iter().copied(),
    );
    budget.check_time()?;
    let mut provider = loaded?;
    provider.probe_deadline = Some(budget.start + HIDDEN_CONFLICT_PROBE_TIMEOUT);
    provider.set_native_architecture(architecture);
    let exact = provider.intern_exact_repository_package(root_name, repository_package_id)?;
    loop {
        budget.before_resolve()?;
        provider.discharge_requirement_groups(ignored.iter().copied())?;
        budget.check_time()?;
        let mut solver = Solver::new(&provider);
        let result = solver.solve(Problem::new().requirements(vec![exact.into()]));
        // Also fence completion: a late success/conflict/missing result is not
        // authority. Resolvo cooperatively checks this same monotonic deadline.
        budget.check_time()?;
        match result {
            Ok(solvable_ids) => {
                let plan = relations::plan_selected_relations(&provider, &solvable_ids)?;
                budget.check_time()?;
                return Ok(
                    (plan.conflict.is_none() && plan.removals.is_empty()).then(|| {
                        ignored
                            .into_iter()
                            .map(|group| SatUnresolvedDependency {
                                repository_package_id: group.repository_package_id,
                                repository_requirement_group_id: group
                                    .repository_requirement_group_id,
                            })
                            .collect()
                    }),
                );
            }
            Err(UnsolvableOrCancelled::Unsolvable(conflict)) => {
                let graph = conflict.graph(&solver);
                let has_conflict = conflict_graph_has_conflict_class(&graph);
                let Some(unresolved) = graph.unresolved_node else {
                    budget.check_time()?;
                    if has_conflict {
                        return Ok(None);
                    }
                    return Err(Error::InternalError(
                        "hidden conflict probe has no attributed failure".to_string(),
                    ));
                };
                let previous = ignored.len();
                for edge in graph.graph.edges_directed(unresolved, Direction::Incoming) {
                    let resolvo::conflict::ConflictEdge::Requires(requirement) = *edge.weight()
                    else {
                        return Err(Error::InternalError(
                            "hidden conflict probe has a non-requirement unresolved edge"
                                .to_string(),
                        ));
                    };
                    if let resolvo::conflict::ConflictNode::Solvable(requiring) =
                        graph.graph[edge.source()]
                    {
                        ignored
                            .extend(provider.unresolved_requirement_groups(requiring, requirement));
                    }
                }
                budget.check_time()?;
                if ignored.len() == previous {
                    if has_conflict {
                        return Ok(None);
                    }
                    return Err(Error::InternalError(
                        "hidden conflict probe made no typed progress".to_string(),
                    ));
                }
            }
            Err(UnsolvableOrCancelled::Cancelled(_)) => return Err(budget.exceeded(Instant::now())),
        }
    }
}

#[cfg(test)]
std::thread_local! { static COUNTS: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) }; }
#[cfg(test)]
pub(super) fn reset_counts() {
    COUNTS.with(|counts| counts.set((0, 0)));
}
#[cfg(test)]
pub(super) fn loaded_provider() {
    COUNTS.with(|counts| counts.set((counts.get().0, counts.get().1 + 1)));
}
#[cfg(test)]
pub(super) fn counts() -> (u32, u32) {
    COUNTS.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monotonic_deadline_fences_before_rebuild_and_after_completion() {
        let start = Instant::now();
        let budget = ProbeBudget {
            root: "timed-root",
            start,
            resolves: 7,
        };
        assert!(
            budget
                .check_time_at(start + HIDDEN_CONFLICT_PROBE_TIMEOUT - Duration::from_nanos(1))
                .is_ok()
        );
        assert!(
            matches!(budget.check_time_at(start + HIDDEN_CONFLICT_PROBE_TIMEOUT),
            Err(Error::HiddenConflictProbeBudgetExceeded { resolves: 7, elapsed, .. }) if elapsed == HIDDEN_CONFLICT_PROBE_TIMEOUT)
        );
    }
}
