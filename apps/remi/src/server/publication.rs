// apps/remi/src/server/publication.rs
//! Publication policy for legacy scriptlet conversion results.

use crate::server::conversion::ScriptletPackageMetadata;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::db::models::{ConvertedPackage, ScriptletSummaryForPublication};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationDecision {
    Ready,
    ReviewRequired(PublicationGateReport),
    Blocked(PublicationGateReport),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationRefusal {
    ReviewRequired(PublicationGateReport),
    Blocked(PublicationGateReport),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicationGateReport {
    pub publication_status: String,
    pub scriptlet_fidelity: String,
    pub target_compatibility: String,
    pub summary_valid: bool,
    pub message: String,
    pub reason_codes: Vec<String>,
    pub blocked_reason_codes: Vec<String>,
    pub review_reason_codes: Vec<String>,
    pub unknown_commands: Vec<String>,
    pub blocked_classes: Vec<String>,
    pub evidence_digest: Option<String>,
    pub curation_evidence_digest: Option<String>,
    pub review_artifact_available: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PublicationRefusalResponse {
    pub status: &'static str,
    pub message: String,
    pub distro: String,
    pub package: String,
    pub version: Option<String>,
    pub scriptlets: PublicationGateReport,
}

pub fn classify_converted_package(converted: &ConvertedPackage) -> PublicationDecision {
    classify_summary(converted.scriptlet_summary_for_publication())
}

pub fn classify_summary(publication: ScriptletSummaryForPublication) -> PublicationDecision {
    if publication.valid && publication.summary.publication_status == "public" {
        return PublicationDecision::Ready;
    }

    let report = report_from_summary(&publication.summary, publication.valid);
    if publication.summary.publication_status == "blocked" {
        PublicationDecision::Blocked(report)
    } else {
        PublicationDecision::ReviewRequired(report)
    }
}

pub fn refusal_response(
    refusal: PublicationRefusal,
    distro: &str,
    package: &str,
    version: Option<&str>,
) -> Response {
    let (status, status_text, report) = match refusal {
        PublicationRefusal::ReviewRequired(report) => {
            (StatusCode::CONFLICT, "review-required", report)
        }
        PublicationRefusal::Blocked(report) => (StatusCode::FORBIDDEN, "blocked", report),
    };

    (
        status,
        Json(PublicationRefusalResponse {
            status: status_text,
            message: report.message.clone(),
            distro: distro.to_string(),
            package: package.to_string(),
            version: version.map(str::to_string),
            scriptlets: report,
        }),
    )
        .into_response()
}

pub fn decision_refusal(decision: PublicationDecision) -> Option<PublicationRefusal> {
    match decision {
        PublicationDecision::Ready => None,
        PublicationDecision::ReviewRequired(report) => {
            Some(PublicationRefusal::ReviewRequired(report))
        }
        PublicationDecision::Blocked(report) => Some(PublicationRefusal::Blocked(report)),
    }
}

pub fn report_from_summary(
    summary: &ScriptletBundleSummary,
    summary_valid: bool,
) -> PublicationGateReport {
    let mut reason_codes = Vec::new();
    let mut seen = BTreeSet::new();
    for code in &summary.blocked_reason_codes {
        push_reason(&mut reason_codes, &mut seen, code.clone());
    }
    for code in &summary.review_reason_codes {
        push_reason(&mut reason_codes, &mut seen, code.clone());
    }
    for command in sorted(&summary.unknown_commands) {
        push_reason(
            &mut reason_codes,
            &mut seen,
            format!("unknown-command:{command}"),
        );
    }
    for class_id in sorted(&summary.blocked_classes) {
        push_reason(&mut reason_codes, &mut seen, class_id);
    }
    if !summary_valid {
        push_reason(
            &mut reason_codes,
            &mut seen,
            "publication-gate-malformed-summary".to_string(),
        );
    }

    PublicationGateReport {
        publication_status: summary.publication_status.clone(),
        scriptlet_fidelity: summary.scriptlet_fidelity.clone(),
        target_compatibility: summary.target_compatibility.clone(),
        summary_valid,
        message: message_for_status(&summary.publication_status, summary_valid).to_string(),
        reason_codes,
        blocked_reason_codes: summary.blocked_reason_codes.clone(),
        review_reason_codes: summary.review_reason_codes.clone(),
        unknown_commands: sorted(&summary.unknown_commands),
        blocked_classes: sorted(&summary.blocked_classes),
        evidence_digest: summary.evidence_digest.clone(),
        curation_evidence_digest: summary.curation_evidence_digest.clone(),
        review_artifact_available: summary.review_artifact_path.is_some(),
    }
}

pub fn public_metadata(summary: &ScriptletBundleSummary) -> ScriptletPackageMetadata {
    ScriptletPackageMetadata::from(summary)
}

fn push_reason(reasons: &mut Vec<String>, seen: &mut BTreeSet<String>, reason: String) {
    if seen.insert(reason.clone()) {
        reasons.push(reason);
    }
}

fn sorted(values: &[String]) -> Vec<String> {
    values
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn message_for_status(status: &str, valid: bool) -> &'static str {
    if !valid {
        return "Converted package has malformed scriptlet publication metadata";
    }
    match status {
        "blocked" => "Converted package is blocked by legacy scriptlet policy",
        "local-only" => "Converted package is local-only and cannot be served publicly",
        "private-review" => "Converted package requires scriptlet review before public serving",
        _ => "Converted package is not public-ready",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::convert::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
    use conary_core::db::models::ScriptletSummaryForPublication;

    fn summary(status: &str) -> ScriptletBundleSummary {
        ScriptletBundleSummary {
            publication_status: status.to_string(),
            scriptlet_fidelity: status.to_string(),
            target_compatibility: status.to_string(),
            ..ScriptletBundleSummary::default()
        }
    }

    #[test]
    fn publication_policy_maps_statuses_to_decisions() {
        assert!(matches!(
            classify_summary(ScriptletSummaryForPublication {
                summary: summary("public"),
                valid: true,
            }),
            PublicationDecision::Ready
        ));
        assert!(matches!(
            classify_summary(ScriptletSummaryForPublication {
                summary: summary("private-review"),
                valid: true,
            }),
            PublicationDecision::ReviewRequired(_)
        ));
        assert!(matches!(
            classify_summary(ScriptletSummaryForPublication {
                summary: summary("blocked"),
                valid: true,
            }),
            PublicationDecision::Blocked(_)
        ));
        assert!(matches!(
            classify_summary(ScriptletSummaryForPublication {
                summary: summary("public"),
                valid: false,
            }),
            PublicationDecision::ReviewRequired(_)
        ));
    }

    #[test]
    fn publication_report_reasons_are_deterministic_and_deduplicated() {
        let summary = ScriptletBundleSummary {
            publication_status: "private-review".to_string(),
            decision_counts: ScriptletDecisionCountsSummary {
                review: 2,
                ..ScriptletDecisionCountsSummary::default()
            },
            blocked_reason_codes: vec!["blocked-b".to_string(), "blocked-a".to_string()],
            review_reason_codes: vec!["review-a".to_string(), "review-a".to_string()],
            unknown_commands: vec!["zz".to_string(), "aa".to_string()],
            blocked_classes: vec!["class-b".to_string(), "class-a".to_string()],
            ..ScriptletBundleSummary::default()
        };

        let report = report_from_summary(&summary, true);

        assert_eq!(
            report.reason_codes,
            vec![
                "blocked-b",
                "blocked-a",
                "review-a",
                "unknown-command:aa",
                "unknown-command:zz",
                "class-a",
                "class-b",
            ]
        );
    }
}
