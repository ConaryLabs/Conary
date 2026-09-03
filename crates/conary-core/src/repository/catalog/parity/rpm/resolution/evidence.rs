// crates/conary-core/src/repository/catalog/parity/rpm/resolution/evidence.rs

//! Byte-bounded projection of libsolv problem evidence.

use super::{
    PackageResolutionIndexReader, SOLVER_RULE_INFARCH, SOLVER_RULE_JOB,
    SOLVER_RULE_JOB_UNSUPPORTED, SOLVER_RULE_PKG_NOT_INSTALLABLE,
    SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP, SOLVER_RULE_PKG_REQUIRES,
    SOLVER_RULE_STRICT_REPO_PRIORITY,
};
use crate::error::Result;
use crate::repository::catalog::parity::resolution_survey::NativeExplanationBudget;
use crate::repository::catalog::parity::{
    NativeResolutionSurveyEvidenceWithheldReasonV1, NativeResolutionSurveyNativeExplanationV1,
    NativeResolutionSurveyRpmPackageV1, NativeResolutionSurveyRpmProblemV1,
    NativeResolutionSurveyRpmRuleV1,
};

use super::super::SolvPool;
use super::super::ffi::{SolvProblem, SolvProblemRule};

pub(super) fn rpm_explanation(
    pool: &SolvPool,
    package_index: &PackageResolutionIndexReader,
    source_problems: &[SolvProblem],
    byte_limit: u64,
) -> NativeResolutionSurveyNativeExplanationV1 {
    record_explanation_build();
    let mut explanation = NativeResolutionSurveyNativeExplanationV1::Rpm {
        problems: Vec::new(),
    };
    let Some(mut budget) = NativeExplanationBudget::for_explanation(&explanation, byte_limit)
    else {
        return withheld();
    };
    let NativeResolutionSurveyNativeExplanationV1::Rpm { problems } = &mut explanation else {
        unreachable!("new RPM explanation has the wrong ecosystem")
    };
    if !append_problems(pool, package_index, problems, source_problems, &mut budget) {
        return withheld();
    }
    explanation
}

#[cfg(test)]
thread_local! {
    static EXPLANATION_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn record_explanation_build() {
    #[cfg(test)]
    EXPLANATION_BUILDS.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
pub(super) fn reset_explanation_builds() {
    EXPLANATION_BUILDS.with(|count| count.set(0));
}

#[cfg(test)]
pub(super) fn explanation_builds() -> usize {
    EXPLANATION_BUILDS.with(std::cell::Cell::get)
}

fn append_problems(
    pool: &SolvPool,
    package_index: &PackageResolutionIndexReader,
    problems: &mut Vec<NativeResolutionSurveyRpmProblemV1>,
    source_problems: &[SolvProblem],
    budget: &mut NativeExplanationBudget,
) -> bool {
    for source_problem in source_problems {
        let mut problem = NativeResolutionSurveyRpmProblemV1 {
            problem: source_problem.problem,
            rules: Vec::new(),
        };
        if !budget.retain(&problem, !problems.is_empty()) {
            return false;
        }
        for source_rule in &source_problem.rules {
            let rule = project_rule(pool, package_index, source_rule);
            if !budget.retain(&rule, !problem.rules.is_empty()) {
                return false;
            }
            problem.rules.push(rule);
        }
        problems.push(problem);
    }
    true
}

fn project_rule(
    pool: &SolvPool,
    package_index: &PackageResolutionIndexReader,
    rule: &SolvProblemRule,
) -> NativeResolutionSurveyRpmRuleV1 {
    let (from, from_unavailable_reason) = rpm_package_field(pool, package_index, rule.from_index);
    let (to, to_unavailable_reason) = rpm_package_field(pool, package_index, rule.to_index);
    let (dependency_id, dependency, dependency_unavailable_reason) =
        rpm_rule_dependency_fields(pool, rule);
    NativeResolutionSurveyRpmRuleV1 {
        rule_type_numeric: rule.rule_type,
        rule_type_symbolic: rpm_rule_type_symbolic(rule.rule_type).to_string(),
        from_native_index: rule.from_index.and_then(|index| index.try_into().ok()),
        from,
        from_unavailable_reason,
        to_native_index: rule.to_index.and_then(|index| index.try_into().ok()),
        to,
        to_unavailable_reason,
        dependency_id,
        dependency,
        dependency_unavailable_reason,
    }
}

fn withheld() -> NativeResolutionSurveyNativeExplanationV1 {
    NativeResolutionSurveyNativeExplanationV1::Withheld {
        reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted,
    }
}

fn rpm_package_field(
    pool: &SolvPool,
    package_index: &PackageResolutionIndexReader,
    index: Option<usize>,
) -> (Option<NativeResolutionSurveyRpmPackageV1>, Option<String>) {
    let Some(index) = index else {
        return (None, None);
    };
    let result: Result<NativeResolutionSurveyRpmPackageV1> = (|| {
        let package = pool.package(index)?;
        Ok(NativeResolutionSurveyRpmPackageV1 {
            package_key_sha256: package_index.package_key(index)?,
            name: package.name()?,
            evr: package.evr()?,
            architecture: package.arch()?,
        })
    })();
    match result {
        Ok(package) => (Some(package), None),
        Err(_) => (
            None,
            Some("native_package_projection_unavailable".to_string()),
        ),
    }
}

fn rpm_dependency_field(pool: &SolvPool, dependency: i32) -> (Option<String>, Option<String>) {
    if dependency == 0 {
        return (None, None);
    }
    match pool.dependency(dependency).and_then(|value| value.text()) {
        Ok(text) => (Some(text), None),
        Err(_) => (None, Some("native_dependency_text_unavailable".to_string())),
    }
}

fn rpm_rule_dependency_fields(
    pool: &SolvPool,
    rule: &SolvProblemRule,
) -> (Option<i32>, Option<String>, Option<String>) {
    let unavailable_reason = match rule.rule_type {
        SOLVER_RULE_JOB => Some("solver_rule_job_dep_is_job_index"),
        SOLVER_RULE_JOB_UNSUPPORTED => Some("solver_rule_job_unsupported_dep_is_job_index"),
        _ => None,
    };
    if let Some(unavailable_reason) = unavailable_reason {
        return (None, None, Some(unavailable_reason.to_string()));
    }
    let (dependency, dependency_unavailable_reason) = rpm_dependency_field(pool, rule.dependency);
    (
        (rule.dependency != 0).then_some(rule.dependency),
        dependency,
        dependency_unavailable_reason,
    )
}

fn rpm_rule_type_symbolic(rule_type: i32) -> &'static str {
    match rule_type {
        0 => "SOLVER_RULE_UNKNOWN",
        0x100 => "SOLVER_RULE_PKG",
        SOLVER_RULE_PKG_NOT_INSTALLABLE => "SOLVER_RULE_PKG_NOT_INSTALLABLE",
        SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP => "SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP",
        SOLVER_RULE_PKG_REQUIRES => "SOLVER_RULE_PKG_REQUIRES",
        0x104 => "SOLVER_RULE_PKG_SELF_CONFLICT",
        0x105 => "SOLVER_RULE_PKG_CONFLICTS",
        0x106 => "SOLVER_RULE_PKG_SAME_NAME",
        0x107 => "SOLVER_RULE_PKG_OBSOLETES",
        0x108 => "SOLVER_RULE_PKG_IMPLICIT_OBSOLETES",
        0x109 => "SOLVER_RULE_PKG_INSTALLED_OBSOLETES",
        0x10a => "SOLVER_RULE_PKG_RECOMMENDS",
        0x10b => "SOLVER_RULE_PKG_CONSTRAINS",
        0x10c => "SOLVER_RULE_PKG_SUPPLEMENTS",
        0x200 => "SOLVER_RULE_UPDATE",
        0x300 => "SOLVER_RULE_FEATURE",
        SOLVER_RULE_JOB => "SOLVER_RULE_JOB",
        0x401 => "SOLVER_RULE_JOB_NOTHING_PROVIDES_DEP",
        0x402 => "SOLVER_RULE_JOB_PROVIDED_BY_SYSTEM",
        0x403 => "SOLVER_RULE_JOB_UNKNOWN_PACKAGE",
        SOLVER_RULE_JOB_UNSUPPORTED => "SOLVER_RULE_JOB_UNSUPPORTED",
        0x500 => "SOLVER_RULE_DISTUPGRADE",
        SOLVER_RULE_INFARCH => "SOLVER_RULE_INFARCH",
        0x700 => "SOLVER_RULE_CHOICE",
        0x800 => "SOLVER_RULE_LEARNT",
        0x900 => "SOLVER_RULE_BEST",
        0xa00 => "SOLVER_RULE_YUMOBS",
        0xb00 => "SOLVER_RULE_RECOMMENDS",
        0xc00 => "SOLVER_RULE_BLACK",
        SOLVER_RULE_STRICT_REPO_PRIORITY => "SOLVER_RULE_STRICT_REPO_PRIORITY",
        _ => "SOLVER_RULE_UNRECOGNIZED",
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    #[test]
    fn rpm_explanation_withholds_multi_problem_root_when_budget_expires() {
        let pool = SolvPool::create().unwrap();
        let package_index = PackageResolutionIndexReader {
            connection: Connection::open_in_memory().unwrap(),
        };
        let problems = (1..=3)
            .map(|problem| SolvProblem {
                problem,
                rules: vec![
                    SolvProblemRule {
                        rule_type: SOLVER_RULE_JOB,
                        from_index: None,
                        to_index: None,
                        dependency: 1,
                    },
                    SolvProblemRule {
                        rule_type: SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP,
                        from_index: None,
                        to_index: None,
                        dependency: 0,
                    },
                ],
            })
            .collect::<Vec<_>>();

        let full = rpm_explanation(&pool, &package_index, &problems, u64::MAX);
        let NativeResolutionSurveyNativeExplanationV1::Rpm { problems: retained } = full else {
            panic!("unbounded RPM explanation must retain native problems")
        };
        assert_eq!(retained.len(), 3);

        let full = rpm_explanation(&pool, &package_index, &problems, u64::MAX);
        let full_bytes = crate::json::canonical_json(&full).unwrap().len() as u64;
        assert_eq!(
            rpm_explanation(&pool, &package_index, &problems, full_bytes),
            full
        );

        assert!(matches!(
            rpm_explanation(&pool, &package_index, &problems, full_bytes - 1),
            NativeResolutionSurveyNativeExplanationV1::Withheld {
                reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted
            }
        ));
    }
}
