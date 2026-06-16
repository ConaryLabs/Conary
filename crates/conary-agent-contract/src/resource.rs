// conary-agent-contract/src/resource.rs
//! Canonical Conary agent resource URI helpers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceRef {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ResourceRef {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            name: None,
        }
    }

    pub fn named(uri: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            name: Some(name.into()),
        }
    }
}

pub fn remi_health() -> ResourceRef {
    ResourceRef::new("conary://remi/health")
}

pub fn remi_repository(name: &str) -> ResourceRef {
    ResourceRef::named(
        format!("conary://remi/repositories/{}", encode_segment(name)),
        name,
    )
}

pub fn remi_federation_peer(peer_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!("conary://remi/federation/peers/{}", encode_segment(peer_id)),
        peer_id,
    )
}

pub fn remi_audit_summary() -> ResourceRef {
    ResourceRef::new("conary://remi/audit/summary")
}

pub fn remi_chunk_stats() -> ResourceRef {
    ResourceRef::new("conary://remi/chunks/stats")
}

pub fn test_suites() -> ResourceRef {
    ResourceRef::new("conary-test://suites")
}

pub fn test_suite(suite_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!("conary-test://suites/{}", encode_segment(suite_id)),
        suite_id,
    )
}

pub fn test_run(run_id: u64) -> ResourceRef {
    ResourceRef::named(format!("conary-test://runs/{run_id}"), run_id.to_string())
}

pub fn test_run_artifact(run_id: u64, artifact_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!(
            "conary-test://runs/{run_id}/artifacts/{}",
            encode_segment(artifact_id)
        ),
        artifact_id,
    )
}

pub fn local_bootstrap_status() -> ResourceRef {
    ResourceRef::new("conary-local://bootstrap/status")
}

pub fn packaging_operations_recent() -> ResourceRef {
    ResourceRef::new("conary-packaging://operations/recent")
}

pub fn packaging_operation(operation_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!(
            "conary-packaging://operations/{}",
            encode_segment(operation_id)
        ),
        operation_id,
    )
}

pub fn packaging_operation_events(operation_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!(
            "conary-packaging://operations/{}/events",
            encode_segment(operation_id)
        ),
        operation_id,
    )
}

pub fn packaging_project(project_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!("conary-packaging://projects/{}", encode_segment(project_id)),
        project_id,
    )
}

pub fn packaging_artifact(artifact_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!(
            "conary-packaging://artifacts/{}",
            encode_segment(artifact_id)
        ),
        artifact_id,
    )
}

fn encode_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_helpers_emit_stable_uris() {
        assert_eq!(remi_health().uri, "conary://remi/health");
        assert_eq!(
            remi_repository("fedora44").uri,
            "conary://remi/repositories/fedora44"
        );
        assert_eq!(test_run(42).uri, "conary-test://runs/42");
        assert_eq!(
            test_run_artifact(42, "logs").uri,
            "conary-test://runs/42/artifacts/logs"
        );
        assert_eq!(
            local_bootstrap_status().uri,
            "conary-local://bootstrap/status"
        );
    }

    #[test]
    fn test_suites_resource_helper_emits_static_index_uri() {
        let resource = test_suites();

        assert_eq!(resource.uri, "conary-test://suites");
        assert!(resource.name.is_none());
    }

    #[test]
    fn resource_path_segments_are_percent_encoded() {
        assert_eq!(
            remi_repository("fedora/44 beta").uri,
            "conary://remi/repositories/fedora%2F44%20beta"
        );
        assert_eq!(
            test_run_artifact(42, "logs/stderr").uri,
            "conary-test://runs/42/artifacts/logs%2Fstderr"
        );
    }

    #[test]
    fn packaging_resource_helpers_emit_stable_uris() {
        assert_eq!(
            packaging_operations_recent().uri,
            "conary-packaging://operations/recent"
        );
        assert_eq!(
            packaging_operation("publish-1700000000000-42").uri,
            "conary-packaging://operations/publish-1700000000000-42"
        );
        assert_eq!(
            packaging_operation_events("cook-1").uri,
            "conary-packaging://operations/cook-1/events"
        );
        assert_eq!(
            packaging_project("recipe path").uri,
            "conary-packaging://projects/recipe%20path"
        );
        assert_eq!(
            packaging_artifact("sha256:abc123").uri,
            "conary-packaging://artifacts/sha256%3Aabc123"
        );
    }
}
