// conary-agent-contract/src/result.rs
//! Shared result envelope for Conary agent-facing operations.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resource::ResourceRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Ok,
    Planned,
    Running,
    Unavailable,
    Failed,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    ReadOnly,
    Low,
    Medium,
    High,
    Destructive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorKind {
    MissingPrerequisite,
    NotSupported,
    Deferred,
    UnsafeWithoutConfirmation,
    RemoteUnavailable,
    ValidationFailed,
    PartialFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentError {
    pub kind: AgentErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Command,
    Resource,
    Artifact,
    Log,
    Check,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceItem {
    pub kind: EvidenceKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NextAction {
    pub label: String,
    pub description: String,
    pub risk: RiskLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub requires_confirmation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfirmationRequirement {
    pub plan_id: String,
    pub level: RiskLevel,
    pub reason: String,
    pub input_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OperationEnvelope {
    pub operation: String,
    pub status: OperationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<ResourceRef>,
    pub risk: RiskLevel,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<ResourceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_actions: Vec<NextAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<ConfirmationRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_logs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentError>,
}

impl OperationEnvelope {
    pub fn new(
        operation: impl Into<String>,
        status: OperationStatus,
        risk: RiskLevel,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            operation: operation.into(),
            status,
            subject: None,
            risk,
            summary: summary.into(),
            changed: Vec::new(),
            evidence: Vec::new(),
            warnings: Vec::new(),
            next_actions: Vec::new(),
            confirmation: None,
            raw_logs: None,
            error: None,
        }
    }
}

macro_rules! result_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        pub struct $name {
            #[serde(flatten)]
            pub envelope: OperationEnvelope,
            #[serde(default)]
            pub data: serde_json::Value,
        }

        impl $name {
            pub fn new(envelope: OperationEnvelope) -> Self {
                Self {
                    envelope,
                    data: serde_json::Value::Null,
                }
            }

            pub fn with_data(mut self, data: serde_json::Value) -> Self {
                self.data = data;
                self
            }
        }
    };
}

result_type!(InspectResult);
result_type!(PlanResult);
result_type!(VerifyResult);
result_type!(ApplyResult);
result_type!(ExplainResult);
result_type!(RecoverResult);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource;

    #[test]
    fn serializes_snake_case_status_and_risk() {
        let envelope = OperationEnvelope {
            subject: Some(resource::remi_health()),
            ..OperationEnvelope::new(
                "remi.health.inspect",
                OperationStatus::Ok,
                RiskLevel::ReadOnly,
                "Remi health is available",
            )
        };
        let value = serde_json::to_value(InspectResult::new(envelope)).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["risk"], "read_only");
        assert_eq!(value["subject"]["uri"], "conary://remi/health");
    }

    #[test]
    fn failed_result_uses_partial_failure_error_kind() {
        let mut envelope = OperationEnvelope::new(
            "conary-test.bootstrap.verify",
            OperationStatus::Partial,
            RiskLevel::ReadOnly,
            "Bootstrap check partially completed",
        );
        envelope.error = Some(AgentError {
            kind: AgentErrorKind::PartialFailure,
            message: "Container runtime is unavailable".to_string(),
            remediation: Some("Start Podman or Docker and rerun the check".to_string()),
        });
        let value = serde_json::to_value(VerifyResult::new(envelope)).unwrap();
        assert_eq!(value["status"], "partial");
        assert_eq!(value["error"]["kind"], "partial_failure");
    }

    #[test]
    fn confirmation_requirement_carries_plan_identity() {
        let confirmation = ConfirmationRequirement {
            plan_id: "plan-remi-audit-purge-1".to_string(),
            level: RiskLevel::Destructive,
            reason: "Purging audit rows removes evidence".to_string(),
            input_label: "Type the plan ID to confirm".to_string(),
            fingerprint: Some("audit-before-1700000000".to_string()),
            expires_at: None,
        };
        let value = serde_json::to_value(confirmation).unwrap();
        assert_eq!(value["level"], "destructive");
        assert_eq!(value["plan_id"], "plan-remi-audit-purge-1");
    }
}
