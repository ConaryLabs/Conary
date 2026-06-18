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
    pub conversion_fidelity: String,
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
    pub conversion_fidelity: &'a str,
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
        schema: "conary.remi.scriptlet-review.v1",
        distro: input.distro.to_string(),
        package: input.package.to_string(),
        version: input.version.to_string(),
        architecture: input.architecture.map(str::to_string),
        original_format: input.original_format.to_string(),
        publication: input.publication,
        conversion_fidelity: input.conversion_fidelity.to_string(),
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
mod tests {
    use super::*;
    use conary_core::ccs::convert::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
    use conary_core::db::models::{
        ChunkPublicationState, ConvertedPackage, ScriptletSummaryForPublication,
    };

    fn summary(status: &str) -> ScriptletBundleSummary {
        ScriptletBundleSummary {
            publication_status: status.to_string(),
            scriptlet_fidelity: status.to_string(),
            target_compatibility: status.to_string(),
            ..ScriptletBundleSummary::default()
        }
    }

    fn golden_summary(
        scriptlet_fidelity: &str,
        target_compatibility: &str,
        publication_status: &str,
    ) -> ScriptletBundleSummary {
        ScriptletBundleSummary {
            scriptlet_fidelity: scriptlet_fidelity.to_string(),
            target_compatibility: target_compatibility.to_string(),
            publication_status: publication_status.to_string(),
            evidence_digest: Some(conary_core::hash::sha256_prefixed(
                format!("{scriptlet_fidelity}:{publication_status}").as_bytes(),
            )),
            ..ScriptletBundleSummary::default()
        }
    }

    fn insert_golden_converted(
        conn: &rusqlite::Connection,
        name: &str,
        chunk: &str,
        summary: &ScriptletBundleSummary,
    ) {
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            name.to_string(),
            "1.0".to_string(),
            "rpm".to_string(),
            format!("sha256:{name}-source"),
            "high".to_string(),
            &[chunk.to_string()],
            10,
            format!("sha256:{name}-content"),
            format!("/cache/{name}.ccs"),
        );
        converted.package_architecture = Some("x86_64".to_string());
        converted.set_scriptlet_metadata(summary).unwrap();
        converted.insert(conn).unwrap();
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
    fn publication_golden_outcomes_filter_public_listing_and_chunks() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("remi.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();

        let native_free = golden_summary("native-free", "source-native", "public");

        let mut fully_replaced = golden_summary("fully-replaced", "source-native", "public");
        fully_replaced.decision_counts = ScriptletDecisionCountsSummary {
            replaced: 1,
            ..ScriptletDecisionCountsSummary::default()
        };

        let mut legacy_replay = golden_summary("legacy-replay", "source-native", "private-review");
        legacy_replay.decision_counts = ScriptletDecisionCountsSummary {
            legacy: 1,
            ..ScriptletDecisionCountsSummary::default()
        };
        legacy_replay
            .review_reason_codes
            .push("legacy-replay-required".to_string());

        let mut review_required =
            golden_summary("review-required", "review-required", "private-review");
        review_required.decision_counts = ScriptletDecisionCountsSummary {
            review: 1,
            ..ScriptletDecisionCountsSummary::default()
        };
        review_required
            .review_reason_codes
            .push("review-class-deb-trigger".to_string());

        let mut blocked = golden_summary("blocked", "blocked", "blocked");
        blocked.decision_counts = ScriptletDecisionCountsSummary {
            blocked: 1,
            ..ScriptletDecisionCountsSummary::default()
        };
        blocked
            .blocked_reason_codes
            .push("blocked-class-package-manager-recursion".to_string());

        let cases = [
            ("goal8a-native-free", "native-free-chunk", native_free, true),
            (
                "goal8a-fully-replaced",
                "fully-replaced-chunk",
                fully_replaced,
                true,
            ),
            (
                "goal8a-legacy-replay",
                "legacy-replay-chunk",
                legacy_replay,
                false,
            ),
            (
                "goal8a-review-required",
                "review-required-chunk",
                review_required,
                false,
            ),
            ("goal8a-blocked", "blocked-chunk", blocked, false),
        ];

        for (name, chunk, summary, _public_ready) in &cases {
            insert_golden_converted(&conn, name, chunk, summary);
        }

        let public_ready_names: std::collections::BTreeSet<_> =
            ConvertedPackage::find_publication_candidates(&conn, "fedora", None)
                .unwrap()
                .into_iter()
                .filter(|converted| converted.is_scriptlet_public_ready())
                .map(|converted| converted.package_name.unwrap())
                .collect();
        assert_eq!(
            public_ready_names,
            std::collections::BTreeSet::from([
                "goal8a-fully-replaced".to_string(),
                "goal8a-native-free".to_string(),
            ])
        );

        for (_name, chunk, _summary, public_ready) in cases {
            let expected = if public_ready {
                ChunkPublicationState::PublicReady
            } else {
                ChunkPublicationState::NonPublicOnly
            };
            assert_eq!(
                local_chunk_servable_by_public_gate(&db_path, chunk).unwrap(),
                public_ready,
                "{chunk}"
            );
            assert_eq!(
                ConvertedPackage::chunk_publication_state(&conn, chunk).unwrap(),
                expected,
                "{chunk}"
            );
        }
    }

    #[test]
    fn publication_gate_does_not_promote_regex_like_signals_to_authority() {
        let mut summary = golden_summary("fully-replaced", "source-native", "public");
        summary
            .review_reason_codes
            .push("regex-advisory-review".to_string());
        summary.unknown_commands.push("systemctl".to_string());

        assert!(matches!(
            classify_summary(ScriptletSummaryForPublication {
                summary,
                valid: true,
            }),
            PublicationDecision::Ready
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
