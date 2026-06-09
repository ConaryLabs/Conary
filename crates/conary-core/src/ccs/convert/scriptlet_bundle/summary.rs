// conary-core/src/ccs/convert/scriptlet_bundle/summary.rs

use super::types::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
use crate::ccs::legacy_scriptlets::{
    DecisionCounts, LegacyScriptletBundle, LegacyScriptletEntry, PublicationPolicy,
    PublicationStatus, ScriptletDecision, ScriptletFidelity, TargetCompatibility,
};
use std::collections::BTreeSet;

impl ScriptletBundleSummary {
    pub fn from_bundle(bundle: &LegacyScriptletBundle, evidence_digest: Option<String>) -> Self {
        summary_from_bundle(bundle, evidence_digest)
    }
}

pub(super) fn summary_from_bundle(
    bundle: &LegacyScriptletBundle,
    evidence_digest: Option<String>,
) -> ScriptletBundleSummary {
    let blocked_reason_codes = sorted_entry_reason_codes(bundle, "blocked");
    let review_reason_codes = sorted_entry_reason_codes(bundle, "review");
    let unknown_commands = bundle
        .entries
        .iter()
        .flat_map(|entry| entry.unknown_commands.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let blocked_classes = bundle
        .entries
        .iter()
        .flat_map(|entry| entry.blocked_classes.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    ScriptletBundleSummary {
        scriptlet_fidelity: bundle.scriptlet_fidelity.as_str().to_string(),
        target_compatibility: bundle.target_compatibility.as_str().to_string(),
        publication_status: bundle.publication_status.as_str().to_string(),
        evidence_digest,
        curation_evidence_digest: None,
        decision_counts: ScriptletDecisionCountsSummary {
            replaced: bundle.decision_counts.replaced,
            legacy: bundle.decision_counts.legacy,
            blocked: bundle.decision_counts.blocked,
            review: bundle.decision_counts.review,
        },
        blocked_reason_codes,
        review_reason_codes,
        unknown_commands,
        blocked_classes,
        review_artifact_path: None,
    }
}

fn sorted_entry_reason_codes(bundle: &LegacyScriptletBundle, decision: &str) -> Vec<String> {
    bundle
        .entries
        .iter()
        .filter(|entry| entry.decision.as_str() == decision)
        .map(|entry| entry.reason_code.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn decision_counts(entries: &[LegacyScriptletEntry]) -> DecisionCounts {
    let mut counts = DecisionCounts::default();
    for entry in entries {
        match entry.decision {
            ScriptletDecision::Replaced => counts.replaced += 1,
            ScriptletDecision::Legacy => counts.legacy += 1,
            ScriptletDecision::Blocked => counts.blocked += 1,
            ScriptletDecision::Review => counts.review += 1,
            ScriptletDecision::Unknown(_) => {}
        }
    }
    counts
}

pub(super) fn aggregate_status(
    entries: &[LegacyScriptletEntry],
    counts: &DecisionCounts,
) -> (
    ScriptletFidelity,
    TargetCompatibility,
    PublicationPolicy,
    PublicationStatus,
) {
    if entries.is_empty() {
        return (
            ScriptletFidelity::NativeFree,
            TargetCompatibility::ConaryPortable,
            PublicationPolicy::PublicIfNoBlocked,
            PublicationStatus::Public,
        );
    }
    if counts.blocked > 0 {
        return (
            ScriptletFidelity::Blocked,
            TargetCompatibility::Blocked,
            PublicationPolicy::Blocked,
            PublicationStatus::Blocked,
        );
    }
    if counts.review > 0 {
        return (
            ScriptletFidelity::ReviewRequired,
            TargetCompatibility::ReviewRequired,
            PublicationPolicy::PrivateReview,
            PublicationStatus::PrivateReview,
        );
    }
    if counts.legacy > 0 {
        return (
            ScriptletFidelity::LegacyReplay,
            TargetCompatibility::SourceNative,
            PublicationPolicy::LocalOnly,
            PublicationStatus::LocalOnly,
        );
    }
    (
        ScriptletFidelity::FullyReplaced,
        TargetCompatibility::ConaryPortable,
        PublicationPolicy::PublicIfNoBlocked,
        PublicationStatus::Public,
    )
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{bundle_for_metadata, package_metadata};
    use super::super::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
    use crate::ccs::convert::effects::ScriptletClassificationReport;

    #[test]
    fn scriptlet_bundle_summary_defaults_match_legacy_rows() {
        let summary = ScriptletBundleSummary::default();

        assert_eq!(summary.scriptlet_fidelity, "unknown");
        assert_eq!(summary.target_compatibility, "unknown");
        assert_eq!(summary.publication_status, "public");
        assert_eq!(summary.evidence_digest, None);
        assert_eq!(summary.curation_evidence_digest, None);
        assert_eq!(
            summary.decision_counts,
            ScriptletDecisionCountsSummary::default()
        );
        assert!(summary.blocked_reason_codes.is_empty());
        assert!(summary.review_reason_codes.is_empty());
        assert!(summary.unknown_commands.is_empty());
        assert!(summary.blocked_classes.is_empty());
        assert_eq!(summary.review_artifact_path, None);
    }

    #[test]
    fn scriptlet_bundle_summary_does_not_serialize_review_artifact_path() {
        let summary = ScriptletBundleSummary {
            review_artifact_path: Some("/tmp/private-review-secret".to_string()),
            ..ScriptletBundleSummary::default()
        };

        let json = serde_json::to_string(&summary).unwrap();

        assert!(!json.contains("review_artifact_path"));
        assert!(!json.contains("private-review-secret"));
    }

    #[test]
    fn scriptlet_bundle_summary_from_bundle_is_public_api() {
        let metadata = package_metadata("public-api", "1.0");
        let classification = ScriptletClassificationReport::default();
        let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();

        let summary = ScriptletBundleSummary::from_bundle(
            &build.bundle,
            Some(crate::hash::sha256_prefixed(b"x")),
        );

        assert_eq!(
            summary.publication_status,
            build.bundle.publication_status.as_str()
        );
        assert_eq!(
            summary.evidence_digest,
            Some(crate::hash::sha256_prefixed(b"x"))
        );
        assert_eq!(summary.review_artifact_path, None);
    }
}
