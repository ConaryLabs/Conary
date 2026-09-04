// crates/conary-agent-contract/src/resource.rs
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

pub fn test_suites() -> ResourceRef {
    ResourceRef::new("conary-test://suites")
}

pub fn test_run(run_id: u64) -> ResourceRef {
    ResourceRef::named(format!("conary-test://runs/{run_id}"), run_id.to_string())
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
        assert_eq!(test_run(42).uri, "conary-test://runs/42");
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
            packaging_project("recipe/path beta").uri,
            "conary-packaging://projects/recipe%2Fpath%20beta"
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
            packaging_project("recipe path").uri,
            "conary-packaging://projects/recipe%20path"
        );
        assert_eq!(
            packaging_artifact("sha256:abc123").uri,
            "conary-packaging://artifacts/sha256%3Aabc123"
        );
    }
}
