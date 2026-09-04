// apps/conary/src/commands/packaging_mcp/server.rs
//! Local stdio MCP server for packaging agent tools.

use std::future::Future;

use conary_mcp::{contract_tool_result, map_internal, server_info};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::tool::{ToolCallContext, ToolRouter},
    handler::server::wrapper::Parameters,
    model::*,
    service::RequestContext,
    tool, tool_router,
};

use super::service::PackagingAgentService;
use super::types::{
    DiagnoseLatestFailureInput, InspectProjectInput, OperationRecordsListInput,
    OperationRecordsReadInput, PublishApplyInput, PublishPlanInput,
};

#[derive(Clone)]
pub(crate) struct PackagingMcpServer {
    service: PackagingAgentService,
    tool_router: ToolRouter<Self>,
}

impl PackagingMcpServer {
    pub(crate) fn new(service: PackagingAgentService) -> Self {
        Self {
            service,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl PackagingMcpServer {
    #[tool(
        name = "conary.packaging.inspect_project",
        description = "Inspect local packaging project or artifact facts without building.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn inspect_project(
        &self,
        Parameters(input): Parameters<InspectProjectInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self.service.inspect_project(input).map_err(map_internal)?;
        contract_tool_result(&result)
    }

    #[tool(
        name = "conary.packaging.diagnose_latest_failure",
        description = "Diagnose the newest failed packaging operation record.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn diagnose_latest_failure(
        &self,
        Parameters(input): Parameters<DiagnoseLatestFailureInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .service
            .diagnose_latest_failure(input)
            .map_err(map_internal)?;
        contract_tool_result(&result)
    }

    #[tool(
        name = "conary.packaging.operation_records.list",
        description = "List recent redacted packaging operation records.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn operation_records_list(
        &self,
        Parameters(input): Parameters<OperationRecordsListInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .service
            .list_operation_records(input)
            .map_err(map_internal)?;
        contract_tool_result(&result)
    }

    #[tool(
        name = "conary.packaging.operation_records.read",
        description = "Read one redacted packaging operation record.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn operation_records_read(
        &self,
        Parameters(input): Parameters<OperationRecordsReadInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .service
            .read_operation_record(input)
            .map_err(map_internal)?;
        contract_tool_result(&result)
    }

    #[tool(
        name = "conary.packaging.publish.plan",
        description = "Plan static artifact publish and return confirmation material.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn publish_plan(
        &self,
        Parameters(input): Parameters<PublishPlanInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self.service.plan_publish(input).map_err(map_internal)?;
        contract_tool_result(&result)
    }

    #[tool(
        name = "conary.packaging.publish.apply",
        description = "Apply a confirmed static artifact publish plan.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn publish_apply(
        &self,
        Parameters(input): Parameters<PublishApplyInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self.service.apply_publish(input).map_err(map_internal)?;
        contract_tool_result(&result)
    }
}

impl ServerHandler for PackagingMcpServer {
    fn get_info(&self) -> ServerInfo {
        server_info(
            "conary-packaging-mcp",
            env!("CARGO_PKG_VERSION"),
            "Local-only Conary packaging MCP server for explicit-recipe inspection, \
             operation-record lookup, and packaging failure diagnosis.",
        )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            ..Default::default()
        }))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, McpError>> + Send + '_ {
        let tool_context = ToolCallContext::new(self, request, context);
        async move { self.tool_router.call(tool_context).await }
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_agent_contract::{RiskLevel, packaging_tools};

    #[test]
    fn packaging_server_catalog_matches_contract_metadata() {
        let mut adapter_tools = PackagingMcpServer::tool_router().list_all();
        adapter_tools.sort_by(|left, right| left.name.cmp(&right.name));
        let mut contract_tools = packaging_tools();
        contract_tools.sort_by(|left, right| left.name.cmp(&right.name));

        assert_eq!(adapter_tools.len(), contract_tools.len());
        for (adapter, contract) in adapter_tools.iter().zip(&contract_tools) {
            assert_eq!(adapter.name.as_ref(), contract.name);
            assert_eq!(
                adapter.description.as_deref(),
                Some(contract.description.as_str())
            );
            let adapter_risk = if adapter
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.read_only_hint)
                == Some(true)
            {
                RiskLevel::ReadOnly
            } else {
                RiskLevel::High
            };
            assert_eq!(
                adapter_risk, contract.risk,
                "risk drift for {}",
                contract.name
            );
        }
    }

    #[tokio::test]
    async fn inspect_project_tool_returns_contract_json() {
        let temp = tempfile::TempDir::new().unwrap();
        let recipe = temp.path().join("recipe.toml");
        std::fs::write(
            &recipe,
            r#"
[package]
name = "demo"
version = "0.1.0"
description = "demo"
license = "MIT"

[source]
path = "."

[build]
install = "true"
"#,
        )
        .unwrap();
        let service = super::super::service::PackagingAgentService::with_operations_dir(
            temp.path().join("ops"),
        );
        let server = PackagingMcpServer::new(service);

        let result = server
            .inspect_project(Parameters(super::super::types::InspectProjectInput {
                target: recipe.display().to_string(),
                recipe: None,
            }))
            .await
            .unwrap();

        let text = result.content[0].as_text().expect("text content");
        assert!(
            text.text
                .contains("\"operation\": \"conary.packaging.inspect_project\"")
        );
        assert!(text.text.contains("\"risk\": \"read_only\""));
    }
}
