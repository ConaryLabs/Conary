// crates/conary-core/src/repository/catalog/parity/alpm/resolution/conflict_probe.rs

//! Bounded, one-dependency-at-a-time native provider questions.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use alpm::{Alpm, Package, TransFlag};

mod native;
mod reachability;

use super::super::database_name;
use super::evidence::alpm_unavailable;
use super::{Error, NativeParityPackageV1, NativeRootResolutionError, ResolutionExplanationLimits};
use crate::repository::catalog::parity::{
    NativeResolutionSurveyAlpmResultV1, NativeResolutionSurveyErrorReasonV1,
    NativeResolutionSurveyNativeExplanationV1,
};

pub(super) use native::Preparation;
use native::prepare_once;
use reachability::relevant_providers;

/// Maximum actual native preparation/conflict evaluations for one exact root.
pub(super) const PROVIDER_SEARCH_CHECK_LIMIT: u32 = 256;

type ProbeResult<T> = std::result::Result<T, Box<NativeRootResolutionError>>;

pub(super) struct ConflictReport {
    source: ConflictSource,
    parties: BTreeSet<PackageId>,
    pub(super) explanation: NativeResolutionSurveyNativeExplanationV1,
}

#[derive(Clone, Copy)]
enum ConflictSource {
    Transaction,
    MissingDependencies,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PackageId {
    database: Option<String>,
    name: String,
    version: String,
}

fn package_id(package: &Package) -> PackageId {
    // conflict_new/_alpm_conflict_dup copy package objects. Their source
    // database and exact native identity survive; their addresses do not.
    PackageId {
        database: package.db().map(|database| database.name().to_string()),
        name: package.name().to_string(),
        version: package.version().to_string(),
    }
}

#[derive(Clone)]
struct ProviderChoice {
    dependency: String,
    selected: PackageId,
    providers: Vec<PackageId>,
}

#[derive(Default)]
struct ProviderAnswers {
    override_choice: Option<(String, PackageId)>,
    choices: Vec<ProviderChoice>,
}

struct CheckBudget<'a> {
    root: &'a NativeParityPackageV1,
    checks: u32,
}

impl CheckBudget<'_> {
    fn consume(&mut self) -> ProbeResult<()> {
        if self.checks == PROVIDER_SEARCH_CHECK_LIMIT {
            return Err(NativeRootResolutionError::new(
                Error::ProviderSearchBudgetExceeded {
                    root: self.root.name.clone(),
                    checks: self.checks,
                },
                NativeResolutionSurveyErrorReasonV1::ProviderSearchBudgetExceeded,
                NativeResolutionSurveyNativeExplanationV1::Alpm {
                    result: NativeResolutionSurveyAlpmResultV1::ProviderSearchBudgetExceeded {
                        root: self.root.name.clone(),
                        checks: self.checks,
                    },
                },
            ));
        }
        self.checks += 1;
        #[cfg(test)]
        CHECKS
            .lock()
            .unwrap()
            .insert(self.root.package_key_sha256.clone(), self.checks);
        Ok(())
    }
}

/// Keep every unrelated dependency at its native default. Only replay answers
/// for choices whose selected provider reaches a baseline conflict party,
/// in the order libalpm asked them. Answers are never combined across choices.
pub(super) fn prepare_with_conflict_probe(
    alpm: &mut Alpm,
    root: &NativeParityPackageV1,
    limits: ResolutionExplanationLimits,
) -> ProbeResult<Preparation> {
    let answers = Rc::new(RefCell::new(ProviderAnswers::default()));
    let previous_callback = alpm.take_raw_question_cb();
    alpm.set_question_cb(Rc::clone(&answers), |question, answers| {
        if let alpm::Question::SelectProvider(mut question) = question.question() {
            let mut answers = answers.borrow_mut();
            let dependency = question.depend().to_string();
            let providers = question
                .providers()
                .iter()
                .map(package_id)
                .collect::<Vec<_>>();
            if let Some((overridden, provider)) = &answers.override_choice
                && overridden == &dependency
                && let Some(index) = providers.iter().position(|id| id == provider)
            {
                // Native provider counts and callback indices are C ints.
                question
                    .set_index(i32::try_from(index).expect("native provider index exceeds i32"));
            }
            if let Ok(index) = usize::try_from(question.index())
                && let Some(selected) = providers.get(index).cloned()
                && !answers
                    .choices
                    .iter()
                    .any(|choice| choice.dependency == dependency && choice.selected == selected)
            {
                answers.choices.push(ProviderChoice {
                    dependency,
                    selected,
                    providers,
                });
            }
        }
    });
    let mut budget = CheckBudget { root, checks: 0 };
    let result = probe(alpm, root, limits, &answers, &mut budget);
    alpm.set_raw_question_cb(previous_callback);
    result
}

fn probe(
    alpm: &mut Alpm,
    root: &NativeParityPackageV1,
    limits: ResolutionExplanationLimits,
    answers: &RefCell<ProviderAnswers>,
    budget: &mut CheckBudget<'_>,
) -> ProbeResult<Preparation> {
    let baseline = prepare_once(alpm, root, limits, budget)?;
    let Preparation::Conflicting(conflict) = baseline else {
        return Ok(baseline);
    };
    let choices = std::mem::take(&mut answers.borrow_mut().choices);
    // Snapshot relevance while the failed native transaction still contains
    // its baseline choices; retries release and replace that transaction.
    let relevant = relevant_providers(alpm, root, &choices, &conflict)?;
    for choice in choices {
        if !relevant.contains(&choice.selected) {
            continue;
        }
        for provider in choice.providers {
            if provider == choice.selected {
                continue;
            }
            answers.borrow_mut().override_choice = Some((choice.dependency.clone(), provider));
            answers.borrow_mut().choices.clear();
            restart_transaction(alpm, root)?;
            let candidate = prepare_once(alpm, root, limits, budget)?;
            if !matches!(candidate, Preparation::Conflicting(_)) {
                return Ok(candidate);
            }
        }
    }
    Ok(Preparation::Conflicting(conflict))
}

fn restart_transaction(alpm: &mut Alpm, root: &NativeParityPackageV1) -> ProbeResult<()> {
    alpm.trans_release().map_err(|error| {
        native_error(
            error,
            NativeResolutionSurveyErrorReasonV1::TransactionReleaseFailed,
        )
    })?;
    alpm.trans_init(TransFlag::DB_ONLY | TransFlag::NO_LOCK | TransFlag::NO_HOOKS)
        .map_err(|error| {
            native_error(
                error,
                NativeResolutionSurveyErrorReasonV1::TransactionInitializationFailed,
            )
        })?;
    let package = exact_root(alpm, root)?;
    alpm.trans_add_pkg(package).map_err(|error| {
        native_error(
            error.error,
            NativeResolutionSurveyErrorReasonV1::TransactionAddRootFailed,
        )
    })
}

fn exact_root<'a>(alpm: &'a Alpm, root: &NativeParityPackageV1) -> ProbeResult<&'a Package> {
    let name = database_name(root.member_ordinal as usize).map_err(|error| {
        NativeRootResolutionError::new(
            error,
            NativeResolutionSurveyErrorReasonV1::ExactRootProjectionFailed,
            alpm_unavailable("invalid_root_database"),
        )
    })?;
    alpm.syncdbs()
        .iter()
        .find(|db| db.name() == name)
        .and_then(|db| db.pkg(root.name.as_str()).ok())
        .ok_or_else(|| {
            NativeRootResolutionError::new(
                Error::InternalError(
                    "exact ALPM root disappeared during provider probing".to_string(),
                ),
                NativeResolutionSurveyErrorReasonV1::ExactRootProjectionFailed,
                alpm_unavailable("exact_root_disappeared_during_provider_probe"),
            )
        })
}

fn native_error(
    error: alpm::Error,
    reason: NativeResolutionSurveyErrorReasonV1,
) -> Box<NativeRootResolutionError> {
    NativeRootResolutionError::new(
        Error::ResolutionError(error.to_string()),
        reason,
        alpm_unavailable("native_provider_probe_transaction_failed"),
    )
}

#[cfg(test)]
static CHECKS: std::sync::Mutex<std::collections::BTreeMap<String, u32>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

#[cfg(test)]
pub(in crate::repository::catalog::parity::alpm) fn native_probe_checks(root_key: &str) -> u32 {
    CHECKS.lock().unwrap()[root_key]
}
