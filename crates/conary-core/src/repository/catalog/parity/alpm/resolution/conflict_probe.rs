// crates/conary-core/src/repository/catalog/parity/alpm/resolution/conflict_probe.rs

//! Path-sensitive libalpm conflict probing for missing-first transactions.

use std::collections::{BTreeMap, BTreeSet};

use alpm::{Alpm, Package};

use super::evidence::alpm_conflict_explanation;
use crate::repository::catalog::parity::NativeResolutionSurveyNativeExplanationV1;

/// Re-probe an unsatisfied transaction without committing to libalpm's first
/// satisfier. A conflict dominates only when every provider path for the
/// root-reachable closure is blocked by a native conflict.
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
    let mut providers = Vec::new();
    for database in alpm.syncdbs().iter() {
        for package in database.pkgs().iter() {
            if alpm_package_satisfies(package, dependency) {
                providers.push(package);
            }
        }
    }
    providers
}

/// Exact typed equivalent of pinned libalpm's `_alpm_depcmp`: literal package
/// versions and versioned provides use libalpm's own version comparator.
fn alpm_package_satisfies(package: &Package, dependency: &alpm::Dep) -> bool {
    if package.name() == dependency.name() && alpm_version_satisfies(package.version(), dependency)
    {
        return true;
    }
    package.provides().iter().any(|provided| {
        if provided.name() != dependency.name() {
            return false;
        }
        if dependency.depmod() == alpm::DepMod::Any {
            return true;
        }
        provided.depmod() == alpm::DepMod::Eq
            && provided
                .version()
                .is_some_and(|version| alpm_version_satisfies(version, dependency))
    })
}

fn alpm_version_satisfies(version: &alpm::Ver, dependency: &alpm::Dep) -> bool {
    use std::cmp::Ordering;

    let Some(required) = dependency.version() else {
        return dependency.depmod() == alpm::DepMod::Any;
    };
    let comparison = version.vercmp(required);
    match dependency.depmod() {
        alpm::DepMod::Any => true,
        alpm::DepMod::Eq => comparison == Ordering::Equal,
        alpm::DepMod::Ge => comparison != Ordering::Less,
        alpm::DepMod::Le => comparison != Ordering::Greater,
        alpm::DepMod::Gt => comparison == Ordering::Greater,
        alpm::DepMod::Lt => comparison == Ordering::Less,
    }
}
