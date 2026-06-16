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

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublishModeInput {
    Auto,
    ArtifactStatic,
    ProjectStatic,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct PublishPlanInput {
    pub artifact_or_project_path: String,
    pub target: String,
    #[serde(default)]
    pub recipe: Option<String>,
    #[serde(default)]
    pub key_dir: Option<String>,
    #[serde(default)]
    pub state_file: Option<String>,
    #[serde(default = "default_publish_mode")]
    pub mode: PublishModeInput,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct PublishApplyInput {
    pub plan_id: String,
    pub fingerprint: String,
    pub confirmation: String,
}

fn default_publish_mode() -> PublishModeInput {
    PublishModeInput::Auto
}
