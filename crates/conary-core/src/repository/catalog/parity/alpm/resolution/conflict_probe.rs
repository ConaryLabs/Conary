// crates/conary-core/src/repository/catalog/parity/alpm/resolution/conflict_probe.rs

//! Path-sensitive libalpm conflict probing for missing-first transactions.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use alpm::{Alpm, Package};

use super::evidence::alpm_conflict_explanation;
use crate::repository::catalog::parity::NativeResolutionSurveyNativeExplanationV1;

/// Re-probe an unsatisfied transaction using libalpm's eligible provider set.
/// A conflict dominates only when every native-selectable provider path for
/// the root-reachable closure is blocked by a native conflict.
pub(super) fn unavoidable_reachable_conflict_explanation(
    alpm: &Alpm,
    root: &Package,
    explanation_byte_limit: u64,
) -> Option<NativeResolutionSurveyNativeExplanationV1> {
    let mut pending = vec![vec![root]];
    let mut visited = BTreeSet::new();
    let mut satisfiers = BTreeMap::new();
    let mut first_conflict = None;

    while let Some(reachable) = pending.pop() {
        let mut identity = reachable
            .iter()
            .map(|package| std::ptr::from_ref(*package).cast::<()>() as usize)
            .collect::<Vec<_>>();
        identity.sort_unstable();
        if !visited.insert(identity) {
            continue;
        }

        let conflicts = alpm.check_conflicts(reachable.iter().map(|package| package.as_ref()));
        if !conflicts.is_empty() {
            first_conflict.get_or_insert_with(|| {
                alpm_conflict_explanation(conflicts.iter(), explanation_byte_limit)
            });
            continue;
        }

        let missing = alpm.check_deps(
            std::iter::empty::<&alpm::Pkg>(),
            std::iter::empty::<&alpm::Pkg>(),
            reachable.iter().map(|package| package.as_ref()),
            false,
        );
        let provider_choices = missing.iter().find_map(|dependency| {
            let dependency = dependency.depend();
            let providers = satisfiers
                .entry(dependency.to_string())
                .or_insert_with(|| alpm_satisfiers(alpm, dependency))
                .iter()
                .copied()
                .filter(|provider| {
                    !reachable
                        .iter()
                        .any(|candidate| std::ptr::eq(*candidate, *provider))
                })
                .collect::<Vec<_>>();
            (!providers.is_empty()).then_some(providers)
        });
        let provider_choices = provider_choices?;
        pending.extend(provider_choices.into_iter().map(|provider| {
            let mut branch = reachable.clone();
            branch.push(provider);
            branch
        }));
    }

    first_conflict
}

fn alpm_satisfiers<'a>(alpm: &'a Alpm, dependency: &alpm::Dep) -> Vec<&'a Package> {
    // `alpm_find_dbs_satisfier` uses the same `resolvedep` routine as native
    // transaction preparation: first select a satisfying literal in database
    // order; only without one does it offer virtual-provider alternatives.
    // Observe those alternatives instead of recreating native eligibility,
    // version matching, or database precedence here. Keep the default answer.
    let alternatives = Rc::new(RefCell::new(BTreeSet::new()));
    let previous_callback = alpm.take_raw_question_cb();
    alpm.set_question_cb(Rc::clone(&alternatives), |question, alternatives| {
        if let alpm::Question::SelectProvider(question) = question.question() {
            alternatives.borrow_mut().extend(
                question
                    .providers()
                    .iter()
                    .map(|package| std::ptr::from_ref(package).cast::<()>() as usize),
            );
        }
    });
    let selected = alpm.syncdbs().find_satisfier(dependency.to_string());
    alpm.set_raw_question_cb(previous_callback);
    let alternatives = alternatives.borrow();
    if alternatives.is_empty() {
        return selected.into_iter().collect();
    }

    // Callback package borrows cannot escape the callback. Rebind their exact
    // identities to the same immutable handle's package cache without unsafe
    // lifetime extension or another satisfier-selection implementation.
    let mut providers = Vec::new();
    for database in alpm.syncdbs().iter() {
        for package in database.pkgs().iter() {
            if alternatives.contains(&(std::ptr::from_ref(package).cast::<()>() as usize)) {
                providers.push(package);
            }
        }
    }
    providers
}
