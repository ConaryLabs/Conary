// crates/conary-core/src/repository/catalog/parity/alpm/resolution/conflict_probe/native.rs

//! One native preparation and the missing-first answer-replayed conflict check.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use alpm::{Alpm, Package, PrepareData};

use super::super::super::project_requirement;
use super::super::evidence::{
    alpm_conflict_explanation, alpm_unavailable, alpm_unsatisfied_explanation,
};
use super::{
    CheckBudget, ConflictReport, ConflictSource, Error, NativeParityPackageV1,
    NativeRootResolutionError, ProbeResult, ProjectPackages, ProviderAnswers,
    ResolutionExplanationLimits, exact_root, install_provider_answers, package_id,
};
use crate::repository::catalog::parity::{
    NativeResolutionSurveyAlpmResultV1, NativeResolutionSurveyErrorReasonV1,
    NativeResolutionSurveyNativeExplanationV1, native_requirement_group_sha256,
};
use crate::repository::dependency_model::RepositoryRequirementKind;

pub(in crate::repository::catalog::parity::alpm::resolution) enum Preparation {
    Prepared,
    Unsatisfied(MissingResult),
    Conflicting(ConflictReport),
}

pub(in crate::repository::catalog::parity::alpm::resolution) struct MissingResult {
    pub dependencies: Vec<(String, String)>,
    pub selected_packages: BTreeMap<String, String>,
}

pub(super) fn prepare_once(
    alpm: &mut Alpm,
    root: &NativeParityPackageV1,
    limits: ResolutionExplanationLimits,
    budget: &mut CheckBudget<'_>,
    answers: &Rc<RefCell<ProviderAnswers>>,
    project_packages: &ProjectPackages<'_>,
) -> ProbeResult<Preparation> {
    let target_architecture = alpm
        .architectures()
        .first()
        .unwrap_or("<unset>")
        .to_string();
    budget.consume()?;
    let mut preparation = match alpm.trans_prepare() {
        Ok(()) => Preparation::Prepared,
        Err(error) => {
            let error_class = error.error();
            match error.data() {
                Some(PrepareData::UnsatisfiedDeps(source_missing)) => {
                    let missing = source_missing
                        .iter()
                        .map(|dependency| {
                            Ok((
                                dependency.target().to_string(),
                                native_requirement_group_sha256(&project_requirement(
                                    dependency.depend(),
                                    RepositoryRequirementKind::Depends,
                                )?)?,
                            ))
                        })
                        .collect::<crate::error::Result<Vec<_>>>()
                        .map_err(|error| {
                            NativeRootResolutionError::new(
                                error,
                                NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                                alpm_unsatisfied_explanation(
                                    source_missing.iter(),
                                    limits.failure_bytes(),
                                ),
                            )
                        })?;
                    if missing.is_empty() {
                        return Err(NativeRootResolutionError::new(
                            Error::ConflictError(format!(
                                "libalpm reported unsatisfied dependencies for '{}' without typed records",
                                root.name
                            )),
                            NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                            alpm_unsatisfied_explanation(
                                source_missing.iter(),
                                limits.failure_bytes(),
                            ),
                        ));
                    }
                    Preparation::Unsatisfied(MissingResult {
                        dependencies: missing,
                        selected_packages: BTreeMap::new(),
                    })
                }
                Some(PrepareData::ConflictingDeps(conflicts)) => {
                    Preparation::Conflicting(conflict_report(
                        conflicts.iter(),
                        limits.diagnostic_outcome_bytes(),
                        ConflictSource::Transaction,
                    ))
                }
                Some(PrepareData::PkgInvalidArch(_)) => {
                    return Err(NativeRootResolutionError::new(
                        Error::ConfigError(format!("libalpm rejected architecture '{target_architecture}' while resolving exact root '{}'", root.name)),
                        NativeResolutionSurveyErrorReasonV1::NativeArchitectureRejected,
                        NativeResolutionSurveyNativeExplanationV1::Alpm {
                            result: NativeResolutionSurveyAlpmResultV1::InvalidArchitecture {
                                packages: Vec::new(),
                                detail_unavailable_reason: Some("pinned_alpm_binding_does_not_safely_expose_invalid_architecture_entries".to_string()),
                            },
                        },
                    ));
                }
                None => {
                    return Err(NativeRootResolutionError::new(
                        Error::ConfigError(format!(
                            "libalpm failed to prepare exact root '{}' with unexpected error {error_class}",
                            root.name
                        )),
                        NativeResolutionSurveyErrorReasonV1::NativeSolverUnexpectedFailure,
                        alpm_unavailable("libalpm_returned_no_typed_prepare_data"),
                    ));
                }
            }
        }
    };
    if let Preparation::Unsatisfied(missing) = &mut preparation {
        missing.selected_packages = project_packages(alpm).map_err(|error| {
            NativeRootResolutionError::new(
                error,
                NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                alpm_unavailable("native_unsatisfied_transaction_projection_failed"),
            )
        })?;
        // Preparation can report missing before reaching its conflict check.
        // Replay the entire active answer path through the same native provider
        // callback as preparation. There is no alternative search in this walk.
        let reachable = answered_reachable(alpm, exact_root(alpm, root)?, answers);
        #[cfg(test)]
        MISSING_CLOSURES.lock().unwrap().insert(
            root.package_key_sha256.clone(),
            reachable
                .iter()
                .map(|package| package.name().to_string())
                .collect(),
        );
        budget.consume()?;
        let conflicts = alpm.check_conflicts(reachable.iter().map(|package| package.as_ref()));
        if !conflicts.is_empty() {
            return Ok(Preparation::Conflicting(conflict_report(
                conflicts.iter(),
                limits.diagnostic_outcome_bytes(),
                ConflictSource::MissingDependencies,
            )));
        }
    }
    Ok(preparation)
}

fn conflict_report<'a>(
    conflicts: impl Iterator<Item = &'a alpm::Conflict>,
    byte_limit: u64,
    source: ConflictSource,
) -> ConflictReport {
    let conflicts = conflicts.collect::<Vec<_>>();
    ConflictReport {
        source,
        parties: conflicts
            .iter()
            .flat_map(|conflict| {
                [
                    package_id(conflict.package1()),
                    package_id(conflict.package2()),
                ]
            })
            .collect(),
        explanation: alpm_conflict_explanation(conflicts.iter().copied(), byte_limit),
    }
}

/// Follow required dependencies depth-first, in native dependency-list order.
/// Both satisfaction by already selected packages and database provider choice
/// remain native queries. There is no alternate-set search or custom matching.
pub(super) fn answered_reachable<'a>(
    alpm: &'a Alpm,
    root: &'a Package,
    answers: &Rc<RefCell<ProviderAnswers>>,
) -> Vec<&'a Package> {
    let mut reachable = vec![root];
    let mut pending = root
        .depends()
        .iter()
        .map(|dep| (root, dep))
        .collect::<Vec<_>>();
    pending.reverse();
    while let Some((requiring, dependency)) = pending.pop() {
        let missing = alpm.check_deps(
            reachable.iter().map(|package| package.as_ref()),
            std::iter::empty::<&alpm::Pkg>(),
            std::iter::once(requiring.as_ref()),
            false,
        );
        let text = dependency.to_string();
        if !missing
            .iter()
            .any(|entry| entry.depend().to_string() == text)
        {
            continue;
        }
        // alpm_find_dbs_satisfier shares resolvedep with preparation. Install
        // the same answer callback explicitly for every dependency on the path,
        // including questions first encountered beneath an overridden provider.
        let previous = install_provider_answers(alpm, answers);
        let provider = alpm.syncdbs().find_satisfier(text);
        alpm.set_raw_question_cb(previous);
        let Some(provider) = provider else {
            continue;
        };
        // Native resolvedeps excludes package names already in its closure.
        if reachable
            .iter()
            .any(|package| package.name() == provider.name())
        {
            continue;
        }
        reachable.push(provider);
        let mut dependencies = provider
            .depends()
            .iter()
            .map(|dep| (provider, dep))
            .collect::<Vec<_>>();
        dependencies.reverse();
        pending.extend(dependencies);
    }
    reachable
}

#[cfg(test)]
static MISSING_CLOSURES: std::sync::Mutex<BTreeMap<String, Vec<String>>> =
    std::sync::Mutex::new(BTreeMap::new());

#[cfg(test)]
pub(in crate::repository::catalog::parity::alpm) fn native_probe_missing_closure(
    root_key: &str,
) -> Vec<String> {
    MISSING_CLOSURES.lock().unwrap()[root_key].clone()
}
