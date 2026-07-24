// apps/remi/src/server/scriptlet_evidence_queue/packet.rs

use conary_core::ccs::convert::support_matrix::{SupportMatrix, SupportOutcome};
use conary_core::db::models::ScriptletEvidenceClusterDetail;
use serde_json::{Value, json};

use super::normalization::{
    sanitize_boot_security_intents_value, sanitize_security_policy_intents_value,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketVisibility {
    Private,
    PublicSanitized,
}

impl PacketVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::PublicSanitized => "public-sanitized",
        }
    }
}

pub fn build_packet(detail: ScriptletEvidenceClusterDetail, visibility: PacketVisibility) -> Value {
    let support = support_matrix_context(&detail);
    let samples = detail
        .samples
        .iter()
        .map(|sample| {
            json!({
                "package": sample.package_name,
                "version": sample.package_version,
                "architecture": sample.package_architecture,
                "publication_status": sample.publication_status,
                "scriptlet_fidelity": sample.scriptlet_fidelity,
                "target_compatibility": sample.target_compatibility,
                "typed_evidence": sample.typed_evidence,
                "reason_codes": parse_json(&sample.reason_codes_json),
                "blocked_classes": parse_json(&sample.blocked_classes_json),
                "boot_security_intents": sanitize_boot_security_intents_value(&sample.boot_security_intents_json),
                "security_policy_intents": sanitize_security_policy_intents_value(&sample.security_policy_intents_json),
                "review_artifact_available": sample.review_artifact_path.is_some(),
                "review_artifact_stale": sample.review_artifact_stale,
                "evidence_digest": sample.evidence_digest,
                "curation_evidence_digest": sample.curation_evidence_digest,
                "observed_at": sample.observed_at,
            })
        })
        .collect::<Vec<_>>();

    let mut packet = json!({
        "schema": "conary.remi.scriptlet-evidence-packet.v1",
        "visibility": visibility.as_str(),
        "cluster": {
            "cluster_key": detail.cluster.cluster_key,
            "state": detail.cluster.state.as_str(),
            "distro": detail.cluster.distro,
            "target_profile": detail.cluster.target_profile,
            "blocked_class": detail.cluster.blocked_class,
            "command": detail.cluster.command,
            "normalized_command_shape": detail.cluster.normalized_command_shape,
            "normalized_command_shape_hash": detail.cluster.normalized_command_shape_hash,
            "lifecycle_phase": detail.cluster.lifecycle_phase,
            "first_seen": detail.cluster.first_seen,
            "last_seen": detail.cluster.last_seen,
            "updated_at": detail.cluster.updated_at,
        },
        "impact": {
            "attempt_count": samples.len(),
            "affected_packages": samples.iter().map(package_identity).collect::<Vec<_>>(),
        },
        "evidence": {
            "samples": samples,
        },
        "support_matrix": support,
        "adapter_work": {
            "suggested_fixture_names": suggested_fixture_names(&support),
        },
    });

    if visibility == PacketVisibility::Private {
        packet["maintainer_notes"] = json!(
            detail
                .notes
                .into_iter()
                .map(|note| json!({
                    "id": note.id,
                    "actor": note.actor,
                    "body": note.body,
                    "created_at": note.created_at,
                    "updated_at": note.updated_at,
                }))
                .collect::<Vec<_>>()
        );
        packet["review_artifacts"] = json!(
            detail
                .samples
                .into_iter()
                .filter(|sample| sample.review_artifact_path.is_some())
                .map(|sample| json!({
                    "sample_id": sample.id,
                    "package": sample.package_name,
                    "version": sample.package_version,
                    "architecture": sample.package_architecture,
                    "stale": sample.review_artifact_stale,
                    "reference": format!("sample-{}-review-artifact", sample.id),
                }))
                .collect::<Vec<_>>()
        );
    }

    packet
}

fn support_matrix_context(detail: &ScriptletEvidenceClusterDetail) -> Value {
    let entry = SupportMatrix::default()
        .entries()
        .iter()
        .find(|entry| entry.class_id == Some(detail.cluster.blocked_class.as_str()))
        .cloned();
    match entry {
        Some(entry) => json!({
            "id": entry.id,
            "command": entry.command,
            "class_id": entry.class_id,
            "adapter_id": entry.adapter_id,
            "outcome": support_outcome(entry.outcome),
            "reason_code": entry.reason_code,
            "source_families": entry.source_families,
            "lifecycle_notes": entry.lifecycle_notes,
            "fixture_names": entry.fixture_names,
        }),
        None => json!({
            "id": null,
            "class_id": detail.cluster.blocked_class,
            "fixture_names": [],
        }),
    }
}

fn support_outcome(outcome: SupportOutcome) -> &'static str {
    match outcome {
        SupportOutcome::Known => "known",
        SupportOutcome::Review => "review",
        SupportOutcome::Blocked => "blocked",
    }
}

fn suggested_fixture_names(support: &Value) -> Vec<String> {
    support
        .get("fixture_names")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn package_identity(sample: &Value) -> Value {
    json!({
        "package": sample.get("package").cloned().unwrap_or(Value::Null),
        "version": sample.get("version").cloned().unwrap_or(Value::Null),
        "architecture": sample.get("architecture").cloned().unwrap_or(Value::Null),
    })
}

fn parse_json(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::Array(Vec::new()))
}
