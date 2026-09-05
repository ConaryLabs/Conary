// crates/conary-core/src/repository/catalog/parity/alpm/resolution/conflict_probe.rs

//! Bounded, one-dependency-at-a-time native provider questions.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
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
#[cfg(test)]
pub(in crate::repository::catalog::parity::alpm) use native::native_probe_missing_closure;
use native::prepare_once;
use reachability::relevant_providers;

/// Maximum actual native preparation/conflict evaluations for one exact root.
pub(super) const PROVIDER_SEARCH_CHECK_LIMIT: u32 = 256;

type ProbeResult<T> = std::result::Result<T, Box<NativeRootResolutionError>>;
type ProjectPackages<'a> = dyn Fn(&Alpm) -> crate::error::Result<BTreeMap<String, String>> + 'a;

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
    overrides: Vec<(String, PackageId)>,
    choices: Vec<ProviderChoice>,
}

/// One failed native context, with alternatives in native question/provider
/// order. Ancestor answers are retained only while exploring relevant questions
/// not yet probed on that path; siblings never inherit an exhausted answer.
struct QuestionFrame {
    alternatives: std::vec::IntoIter<(String, PackageId)>,
    answer: Option<(String, PackageId)>,
}

impl QuestionFrame {
    fn from_failure(
        alpm: &Alpm,
        root: &NativeParityPackageV1,
        choices: Vec<ProviderChoice>,
        conflict: &ConflictReport,
        explored_questions: BTreeSet<String>,
        answers: &Rc<RefCell<ProviderAnswers>>,
    ) -> ProbeResult<Self> {
        // Read this failure's chosen set before a retry replaces it. Questions
        // already explored on this path stay owned by their ancestor frames.
        // Mere visibility never suppresses a question: a retry can make a
        // previously irrelevant selected provider reach a new conflict party.
        let relevant = relevant_providers(alpm, root, &choices, conflict, answers)?;
        let mut alternatives = Vec::new();
        for choice in choices {
            if !explored_questions.contains(&choice.dependency)
                && relevant.contains(&choice.selected)
            {
                alternatives.extend(
                    choice
                        .providers
                        .into_iter()
                        .filter(|provider| *provider != choice.selected)
                        .map(|provider| (choice.dependency.clone(), provider)),
                );
            }
        }
        Ok(Self {
            alternatives: alternatives.into_iter(),
            answer: None,
        })
    }
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
/// for choices whose selected provider reaches the current conflict, descending
/// through newly relevant native questions before backtracking to alternatives.
pub(super) fn prepare_with_conflict_probe(
    alpm: &mut Alpm,
    root: &NativeParityPackageV1,
    limits: ResolutionExplanationLimits,
    project_packages: &ProjectPackages<'_>,
) -> ProbeResult<Preparation> {
    let answers = Rc::new(RefCell::new(ProviderAnswers::default()));
    let previous_callback = install_provider_answers(alpm, &answers);
    let mut budget = CheckBudget { root, checks: 0 };
    let result = probe(alpm, root, limits, &answers, &mut budget, project_packages);
    alpm.set_raw_question_cb(previous_callback);
    result
}

/// Both native preparation and missing-first satisfier queries use this exact
/// callback. Every active answer is replayed only among native-offered providers.
fn install_provider_answers(
    alpm: &Alpm,
    answers: &Rc<RefCell<ProviderAnswers>>,
) -> alpm::RawQuestionCb {
    let previous_callback = alpm.take_raw_question_cb();
    alpm.set_question_cb(Rc::clone(answers), |question, answers| {
        if let alpm::Question::SelectProvider(mut question) = question.question() {
            let mut answers = answers.borrow_mut();
            let dependency = question.depend().to_string();
            let providers = question
                .providers()
                .iter()
                .map(package_id)
                .collect::<Vec<_>>();
            if let Some((_, provider)) = answers
                .overrides
                .iter()
                .find(|(overridden, _)| overridden == &dependency)
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
    previous_callback
}

fn probe(
    alpm: &mut Alpm,
    root: &NativeParityPackageV1,
    limits: ResolutionExplanationLimits,
    answers: &Rc<RefCell<ProviderAnswers>>,
    budget: &mut CheckBudget<'_>,
    project_packages: &ProjectPackages<'_>,
) -> ProbeResult<Preparation> {
    let baseline = prepare_once(alpm, root, limits, budget, answers, project_packages)?;
    let Preparation::Conflicting(conflict) = baseline else {
        return Ok(baseline);
    };
    let choices = std::mem::take(&mut answers.borrow_mut().choices);
    let mut stack = vec![QuestionFrame::from_failure(
        alpm,
        root,
        choices,
        &conflict,
        BTreeSet::new(),
        answers,
    )?];
    // Native question/provider order defines the first conflict-free missing
    // fallback across this depth-first walk. A prepared path always wins.
    let mut missing_fallback = None;
    while let Some(frame) = stack.last_mut() {
        let Some(answer) = frame.alternatives.next() else {
            stack.pop();
            continue;
        };
        frame.answer = Some(answer);
        {
            let mut answers = answers.borrow_mut();
            answers.overrides = stack
                .iter()
                .filter_map(|frame| frame.answer.clone())
                .collect();
            answers.choices.clear();
        }
        restart_transaction(alpm, root)?;
        // Every depth shares this budget. Exhaustion propagates as a typed
        // producer failure before any fallback closure classification.
        let current_conflict =
            match prepare_once(alpm, root, limits, budget, answers, project_packages)? {
                Preparation::Prepared => return Ok(Preparation::Prepared),
                Preparation::Unsatisfied(missing) => {
                    missing_fallback.get_or_insert(missing);
                    continue;
                }
                Preparation::Conflicting(conflict) => conflict,
            };
        // Derive suppression only after the active answers were actually
        // probed. Pending alternatives and merely visible questions are not
        // explored. Popping a frame automatically removes its path-local state
        // so a sibling may explore that question once on its own path.
        let explored_questions = stack
            .iter()
            .filter_map(|frame| frame.answer.as_ref().map(|(question, _)| question.clone()))
            .collect();
        let choices = std::mem::take(&mut answers.borrow_mut().choices);
        stack.push(QuestionFrame::from_failure(
            alpm,
            root,
            choices,
            &current_conflict,
            explored_questions,
            answers,
        )?);
    }
    Ok(missing_fallback.map_or(Preparation::Conflicting(conflict), Preparation::Unsatisfied))
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
