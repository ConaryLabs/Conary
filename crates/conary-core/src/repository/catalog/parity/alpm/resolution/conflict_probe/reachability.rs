// crates/conary-core/src/repository/catalog/parity/alpm/resolution/conflict_probe/reachability.rs

//! Reachability over native-selected packages, never alternate provider sets.

use std::collections::{BTreeMap, BTreeSet};
use std::{cell::RefCell, rc::Rc};

use alpm::{Alpm, Package};

use super::native::answered_reachable;
use super::{
    ConflictReport, ConflictSource, Error, NativeParityPackageV1,
    NativeResolutionSurveyErrorReasonV1, NativeRootResolutionError, PackageId, ProbeResult,
    ProviderAnswers, ProviderChoice, alpm_unavailable, exact_root, package_id,
};

pub(super) fn relevant_providers(
    alpm: &Alpm,
    root: &NativeParityPackageV1,
    choices: &[ProviderChoice],
    conflict: &ConflictReport,
    answers: &Rc<RefCell<ProviderAnswers>>,
) -> ProbeResult<BTreeSet<PackageId>> {
    let root = exact_root(alpm, root)?;
    let selected = alpm.trans_add();
    let root_id = package_id(root);
    let populated = selected
        .iter()
        .any(|package| package_id(package) != root_id);
    match (conflict.source, populated) {
        (ConflictSource::Transaction, _) | (ConflictSource::MissingDependencies, true) => {
            // ConflictingDeps follows native dependency resolution. A later
            // UnsatisfiedDeps may also retain a populated add set. In both
            // cases bind edges within that chosen set using the native
            // find_dep_satisfier rule, also retaining the exact root if native
            // conflict handling displaced it. Never consult unselected sync
            // packages here: a different default can change reachability.
            let mut packages = selected.iter().collect::<Vec<_>>();
            if !packages
                .iter()
                .any(|package| package_id(package) == package_id(root))
            {
                packages.push(root);
            }
            let mut requiring = BTreeMap::<PackageId, Vec<PackageId>>::new();
            for package in packages {
                for dependency in package.depends().iter() {
                    let text = dependency.to_string();
                    let provider = selected.find_satisfier(text.clone()).or_else(|| {
                        // The borrowed add list cannot be extended by this
                        // binding. Use native check_deps for the extra root,
                        // rather than implementing package/provides matching.
                        let missing = alpm.check_deps(
                            std::iter::once(root.as_ref()),
                            std::iter::empty::<&alpm::Pkg>(),
                            std::iter::once(package.as_ref()),
                            false,
                        );
                        (!missing
                            .iter()
                            .any(|entry| entry.depend().to_string() == text))
                        .then_some(root)
                    });
                    if let Some(provider) = provider {
                        requiring
                            .entry(package_id(provider))
                            .or_default()
                            .push(package_id(package));
                    }
                }
            }
            // One reverse graph walk finds all selected providers whose
            // closures reach a party. A visited set terminates dependency
            // cycles without enumerating any provider combinations.
            let mut relevant = BTreeSet::new();
            let mut pending = conflict.parties.iter().cloned().collect::<Vec<_>>();
            while let Some(package) = pending.pop() {
                if relevant.insert(package.clone())
                    && let Some(parents) = requiring.get(&package)
                {
                    pending.extend(parents.iter().cloned());
                }
            }
            Ok(relevant)
        }
        (ConflictSource::MissingDependencies, false) => {
            // UnsatisfiedDeps failed before libalpm populated trans_add;
            // resolvedeps restores its prior package list on this early exit.
            // Fall back to each selected provider's sync-db closure, replaying
            // every active answer through native precedence selection. This
            // follows the current path without searching alternatives.
            let mut relevant = BTreeSet::new();
            for choice in choices {
                let selected = lookup_selected(alpm, &choice.selected)?;
                if answered_reachable(alpm, selected, answers)
                    .iter()
                    .any(|package| conflict.parties.contains(&package_id(package)))
                {
                    relevant.insert(choice.selected.clone());
                }
            }
            Ok(relevant)
        }
    }
}

fn lookup_selected<'a>(alpm: &'a Alpm, selected: &PackageId) -> ProbeResult<&'a Package> {
    alpm.syncdbs()
        .iter()
        .find(|database| Some(database.name()) == selected.database.as_deref())
        .and_then(|database| database.pkg(selected.name.as_str()).ok())
        .filter(|package| package_id(package) == *selected)
        .ok_or_else(|| {
            NativeRootResolutionError::new(
                Error::InternalError(
                    "native selected provider disappeared during conflict reachability".to_string(),
                ),
                NativeResolutionSurveyErrorReasonV1::NativeSolverUnexpectedFailure,
                alpm_unavailable("selected_provider_disappeared_during_conflict_reachability"),
            )
        })
}
