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
}
