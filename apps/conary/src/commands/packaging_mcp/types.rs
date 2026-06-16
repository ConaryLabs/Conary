// apps/conary/src/commands/packaging_mcp/types.rs
//! Tool input and data DTOs for the packaging MCP service.

use conary_agent_contract::ResourceRef;

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct InspectProjectInput {
    pub target: String,
    #[serde(default)]
    pub recipe: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ExplainInferenceInput {
    pub target: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct DiagnoseLatestFailureInput {
    #[serde(default)]
    pub limit_events: Option<usize>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct OperationRecordsListInput {
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct OperationRecordsReadInput {
    pub operation_id: String,
    #[serde(default)]
    pub include_events: bool,
}

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct InspectProjectData {
    pub target_kind: String,
    pub subject: ResourceRef,
    pub recipe_path: Option<String>,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct OperationRecordData {
    pub operation_id: String,
    pub record: serde_json::Value,
}
