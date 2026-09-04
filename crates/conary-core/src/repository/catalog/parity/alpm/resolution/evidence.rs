// crates/conary-core/src/repository/catalog/parity/alpm/resolution/evidence.rs

//! Bounded libalpm-native explanations retained by diagnostic surveys.

use alpm::{Alpm, Package};

use crate::repository::catalog::parity::resolution_survey::NativeExplanationBudget;
use crate::repository::catalog::parity::{
    NativeResolutionSurveyAlpmConflictV1, NativeResolutionSurveyAlpmMissingV1,
    NativeResolutionSurveyAlpmPackageV1, NativeResolutionSurveyAlpmResultV1,
    NativeResolutionSurveyEvidenceWithheldReasonV1, NativeResolutionSurveyNativeExplanationV1,
};

pub(super) fn alpm_prepared_explanation(
    alpm: &Alpm,
    byte_limit: u64,
) -> NativeResolutionSurveyNativeExplanationV1 {
    record_explanation_build();
    let mut explanation = NativeResolutionSurveyNativeExplanationV1::Alpm {
        result: NativeResolutionSurveyAlpmResultV1::Prepared {
            packages: Vec::new(),
        },
    };
    let Some(mut budget) = NativeExplanationBudget::for_explanation(&explanation, byte_limit)
    else {
        return evidence_withheld();
    };
    let NativeResolutionSurveyNativeExplanationV1::Alpm {
        result: NativeResolutionSurveyAlpmResultV1::Prepared { packages },
    } = &mut explanation
    else {
        unreachable!("new ALPM explanation has the wrong result")
    };
    for source_package in alpm.trans_add().iter() {
        let package = alpm_package_explanation(source_package);
        if !budget.retain(&package, !packages.is_empty()) {
            return evidence_withheld();
        }
        packages.push(package);
    }
    explanation
}

pub(super) fn alpm_unsatisfied_explanation<'a>(
    dependencies: impl IntoIterator<Item = &'a alpm::DepMissing>,
    byte_limit: u64,
) -> NativeResolutionSurveyNativeExplanationV1 {
    record_explanation_build();
    let mut explanation = NativeResolutionSurveyNativeExplanationV1::Alpm {
        result: NativeResolutionSurveyAlpmResultV1::Unsatisfied {
            missing: Vec::new(),
        },
    };
    let Some(mut budget) = NativeExplanationBudget::for_explanation(&explanation, byte_limit)
    else {
        return evidence_withheld();
    };
    let NativeResolutionSurveyNativeExplanationV1::Alpm {
        result: NativeResolutionSurveyAlpmResultV1::Unsatisfied { missing },
    } = &mut explanation
    else {
        unreachable!("new ALPM explanation has the wrong result")
    };
    for dependency in dependencies {
        let entry = NativeResolutionSurveyAlpmMissingV1 {
            target: dependency.target().to_string(),
            dependency: dependency.depend().to_string(),
            causing_package: dependency.causing_pkg().map(str::to_string),
        };
        if !budget.retain(&entry, !missing.is_empty()) {
            return evidence_withheld();
        }
        missing.push(entry);
    }
    explanation
}

pub(super) fn alpm_conflict_explanation<'a>(
    conflicts: impl IntoIterator<Item = &'a alpm::Conflict>,
    byte_limit: u64,
) -> NativeResolutionSurveyNativeExplanationV1 {
    record_explanation_build();
    let mut explanation = NativeResolutionSurveyNativeExplanationV1::Alpm {
        result: NativeResolutionSurveyAlpmResultV1::Conflicts {
            conflicts: Vec::new(),
        },
    };
    let Some(mut budget) = NativeExplanationBudget::for_explanation(&explanation, byte_limit)
    else {
        return evidence_withheld();
    };
    let NativeResolutionSurveyNativeExplanationV1::Alpm {
        result:
            NativeResolutionSurveyAlpmResultV1::Conflicts {
                conflicts: retained,
            },
    } = &mut explanation
    else {
        unreachable!("new ALPM explanation has the wrong result")
    };
    for conflict in conflicts {
        let entry = NativeResolutionSurveyAlpmConflictV1 {
            package1: alpm_package_explanation(conflict.package1()),
            package2: alpm_package_explanation(conflict.package2()),
            reason: conflict.reason().to_string(),
        };
        if !budget.retain(&entry, !retained.is_empty()) {
            return evidence_withheld();
        }
        retained.push(entry);
    }
    explanation
}

fn alpm_package_explanation(package: &Package) -> NativeResolutionSurveyAlpmPackageV1 {
    NativeResolutionSurveyAlpmPackageV1 {
        name: package.name().to_string(),
        version: package.version().to_string(),
        architecture: package.arch().map(str::to_string),
    }
}

pub(super) fn alpm_unavailable(reason: &str) -> NativeResolutionSurveyNativeExplanationV1 {
    NativeResolutionSurveyNativeExplanationV1::Alpm {
        result: NativeResolutionSurveyAlpmResultV1::Unavailable {
            reason: reason.to_string(),
        },
    }
}

fn evidence_withheld() -> NativeResolutionSurveyNativeExplanationV1 {
    NativeResolutionSurveyNativeExplanationV1::Withheld {
        reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted,
    }
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
pub(in crate::repository::catalog::parity::alpm) fn reset_explanation_builds() {
    EXPLANATION_BUILDS.with(|count| count.set(0));
}

#[cfg(test)]
pub(in crate::repository::catalog::parity::alpm) fn explanation_builds() -> usize {
    EXPLANATION_BUILDS.with(std::cell::Cell::get)
}
