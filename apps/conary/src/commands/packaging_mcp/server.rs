// apps/conary/src/commands/packaging_mcp/server.rs
//! Local stdio MCP server for packaging agent tools.

use std::future::Future;

use conary_mcp::tools::contract_tool_result;
use conary_mcp::{map_internal, server_info};
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
    DiagnoseLatestFailureInput, ExplainInferenceInput, InspectProjectInput,
    OperationRecordsListInput, OperationRecordsReadInput,
};

#[derive(Clone)]
pub(crate) struct PackagingMcpServer {
    service: PackagingAgentService,
    #[allow(dead_code)] // Read by rmcp's tool_router macro via generated code.
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
        name = "conary.packaging.explain_inference",
        description = "Explain recipe inference for a local source tree.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn explain_inference(
        &self,
        Parameters(input): Parameters<ExplainInferenceInput>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .service
            .explain_inference(input)
            .map_err(map_internal)?;
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
}

impl ServerHandler for PackagingMcpServer {
    fn get_info(&self) -> ServerInfo {
        server_info(
            "conary-packaging-mcp",
            env!("CARGO_PKG_VERSION"),
            "Local-only Conary packaging MCP server for read-only project inspection, \
             inference explanation, operation-record lookup, and packaging failure diagnosis.",
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
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
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
    use std::collections::BTreeSet;

    #[test]
    fn packaging_server_catalog_exposes_read_tools_with_contract_names() {
        let tools = PackagingMcpServer::tool_router().list_all();
        let names = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<BTreeSet<_>>();

        assert!(names.contains("conary.packaging.inspect_project"));
        assert!(names.contains("conary.packaging.explain_inference"));
        assert!(names.contains("conary.packaging.diagnose_latest_failure"));
        assert!(names.contains("conary.packaging.operation_records.list"));
        assert!(names.contains("conary.packaging.operation_records.read"));
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
