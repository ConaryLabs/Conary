// apps/remi/src/server/publication.rs
//! Publication policy for legacy scriptlet conversion results.

use crate::server::conversion::{ScriptletPackageMetadata, ServerConversionResult};
use crate::server::jobs::JobStatus;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::ccs::legacy_scriptlets::UnknownCommandEvidence;
use conary_core::db::models::{
    ChunkPublicationState, ConvertedPackage, NativePackagePublication,
    ScriptletSummaryForPublication,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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
    pub unknown_command_evidence: Vec<UnknownCommandEvidence>,
    pub blocked_classes: Vec<String>,
    #[serde(default)]
    pub boot_security_intents: Vec<conary_core::ccs::legacy_scriptlets::BootSecurityIntentEvidence>,
    #[serde(default)]
    pub security_policy_intents: Vec<conary_core::ccs::security_policy::SecurityPolicyIntent>,
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

#[derive(Debug)]
pub enum ServerConversionOutcome {
    Ready(ServerConversionResult),
    ReviewRequired(ServerConversionResult),
    Blocked(ServerConversionResult),
}

impl ServerConversionOutcome {
    pub fn into_result(self) -> ServerConversionResult {
        match self {
            Self::Ready(result) | Self::ReviewRequired(result) | Self::Blocked(result) => result,
        }
    }

    pub fn result(&self) -> &ServerConversionResult {
        match self {
            Self::Ready(result) | Self::ReviewRequired(result) | Self::Blocked(result) => result,
        }
    }

    pub fn result_mut(&mut self) -> &mut ServerConversionResult {
        match self {
            Self::Ready(result) | Self::ReviewRequired(result) | Self::Blocked(result) => result,
        }
    }

    pub fn job_status(&self) -> JobStatus {
        match self {
            Self::Ready(_) => JobStatus::Ready,
            Self::ReviewRequired(_) => JobStatus::ReviewRequired,
            Self::Blocked(_) => JobStatus::Blocked,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScriptletReviewArtifact {
    pub schema: &'static str,
    pub distro: String,
    pub package: String,
    pub version: String,
    pub architecture: Option<String>,
    pub original_format: String,
    pub publication: PublicationGateReport,
    pub scriptlet_fidelity: String,
    pub conversion_version: i32,
    pub ccs_content_hash: String,
    pub ccs_total_size: u64,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ReviewArtifactInput<'a> {
    pub distro: &'a str,
    pub package: &'a str,
    pub version: &'a str,
    pub architecture: Option<&'a str>,
    pub original_format: &'a str,
    pub scriptlet_fidelity: &'a str,
    pub conversion_version: i32,
    pub ccs_content_hash: &'a str,
    pub ccs_total_size: u64,
    pub publication: PublicationGateReport,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportIntentVisibility {
    Sanitized,
    Raw,
}

pub fn report_from_summary(
    summary: &ScriptletBundleSummary,
    summary_valid: bool,
) -> PublicationGateReport {
    report_from_summary_with_intent_visibility(
        summary,
        summary_valid,
        ReportIntentVisibility::Sanitized,
    )
}

pub fn raw_report_from_summary(
    summary: &ScriptletBundleSummary,
    summary_valid: bool,
) -> PublicationGateReport {
    report_from_summary_with_intent_visibility(summary, summary_valid, ReportIntentVisibility::Raw)
}

fn report_from_summary_with_intent_visibility(
    summary: &ScriptletBundleSummary,
    summary_valid: bool,
    intent_visibility: ReportIntentVisibility,
) -> PublicationGateReport {
    let unknown_command_evidence = summary
        .unknown_command_evidence
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut reason_codes = Vec::new();
    let mut seen = BTreeSet::new();
    for code in &summary.blocked_reason_codes {
        push_reason(&mut reason_codes, &mut seen, code.clone());
    }
    for code in &summary.review_reason_codes {
        push_reason(&mut reason_codes, &mut seen, code.clone());
    }
    for evidence in &unknown_command_evidence {
        push_reason(
            &mut reason_codes,
            &mut seen,
            format!("unknown-command:{}", evidence.command),
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

    let (boot_security_intents, security_policy_intents) = match intent_visibility {
        ReportIntentVisibility::Sanitized => (
            crate::server::scriptlet_evidence_queue::normalization::sanitize_boot_security_intents(
                &summary.boot_security_intents,
            ),
            crate::server::scriptlet_evidence_queue::normalization::sanitize_security_policy_intents(
                &summary.security_policy_intents,
            ),
        ),
        ReportIntentVisibility::Raw => (
            summary.boot_security_intents.clone(),
            summary.security_policy_intents.clone(),
        ),
    };

    PublicationGateReport {
        publication_status: summary.publication_status.clone(),
        scriptlet_fidelity: summary.scriptlet_fidelity.clone(),
        target_compatibility: summary.target_compatibility.clone(),
        summary_valid,
        message: message_for_summary(summary, summary_valid),
        reason_codes,
        blocked_reason_codes: summary.blocked_reason_codes.clone(),
        review_reason_codes: summary.review_reason_codes.clone(),
        unknown_command_evidence,
        blocked_classes: sorted(&summary.blocked_classes),
        boot_security_intents,
        security_policy_intents,
        evidence_digest: summary.evidence_digest.clone(),
        curation_evidence_digest: summary.curation_evidence_digest.clone(),
        review_artifact_available: summary.review_artifact_path.is_some(),
    }
}

pub fn public_metadata(summary: &ScriptletBundleSummary) -> ScriptletPackageMetadata {
    ScriptletPackageMetadata::from(summary)
}

pub fn local_chunk_servable_by_public_gate(db_path: &Path, hash: &str) -> anyhow::Result<bool> {
    let conn = crate::server::open_runtime_db(db_path)?;
    if NativePackagePublication::active_by_content_hash(&conn, hash)?.is_some() {
        return Ok(true);
    }
    Ok(!matches!(
        ConvertedPackage::chunk_publication_state(&conn, hash)?,
        ChunkPublicationState::NonPublicOnly
    ))
}

pub fn review_artifact_root(cache_dir: &Path) -> PathBuf {
    cache_dir.join("scriptlet-review")
}

pub fn write_review_artifact(
    cache_dir: &Path,
    input: ReviewArtifactInput<'_>,
) -> anyhow::Result<PathBuf> {
    let digest = input
        .publication
        .evidence_digest
        .as_deref()
        .unwrap_or("missing-evidence-digest")
        .replace(':', "-");
    let dir = review_artifact_root(cache_dir)
        .join(sanitize_component(input.distro))
        .join(sanitize_component(input.package))
        .join(sanitize_component(input.version))
        .join(sanitize_component(input.architecture.unwrap_or("noarch")));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{digest}.json"));
    let temp_path = dir.join(format!("{digest}.json.tmp"));
    let artifact = ScriptletReviewArtifact {
        schema: "conary.remi.scriptlet-review.v2",
        distro: input.distro.to_string(),
        package: input.package.to_string(),
        version: input.version.to_string(),
        architecture: input.architecture.map(str::to_string),
        original_format: input.original_format.to_string(),
        publication: input.publication,
        scriptlet_fidelity: input.scriptlet_fidelity.to_string(),
        conversion_version: input.conversion_version,
        ccs_content_hash: input.ccs_content_hash.to_string(),
        ccs_total_size: input.ccs_total_size,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let bytes = serde_json::to_vec_pretty(&artifact)?;
    std::fs::write(&temp_path, bytes)?;
    std::fs::rename(&temp_path, &path)?;
    Ok(path)
}

pub fn validate_review_artifact_path(cache_dir: &Path, path: &Path) -> anyhow::Result<bool> {
    let canonical_root = review_artifact_root(cache_dir).canonicalize()?;
    let canonical_path = path.canonicalize()?;
    Ok(canonical_path.starts_with(canonical_root))
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

fn message_for_summary(summary: &ScriptletBundleSummary, valid: bool) -> String {
    if !valid {
        return "Converted package has malformed scriptlet publication metadata".to_string();
    }
    match summary.publication_status.as_str() {
        "blocked" => {
            if summary.blocked_classes.is_empty() {
                "Converted package uses unsupported legacy scriptlets and cannot be served by the Remi public preview".to_string()
            } else {
                let mut classes = summary.blocked_classes.clone();
                classes.sort();
                format!(
                    "Converted package uses unsupported legacy scriptlet classes for the Remi public preview: {}",
                    classes.join(", ")
                )
            }
        }
        "local-only" => "Converted package is local-only and cannot be served publicly".to_string(),
        "private-review" => {
            "Converted package requires scriptlet review before public serving".to_string()
        }
        _ => "Converted package is not public-ready".to_string(),
    }
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
