// apps/conary/src/commands/packaging_mcp/mod.rs
//! Local packaging MCP command surface.

use anyhow::Result;

pub(crate) mod projection;
pub(crate) mod publish_plan;
pub(crate) mod records;
mod server;
pub(crate) mod service;
pub(crate) mod types;

pub async fn cmd_mcp_packaging() -> Result<()> {
    use rmcp::ServiceExt;

    let service = service::PackagingAgentService::default();
    let server = server::PackagingMcpServer::new(service);
    server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| anyhow::anyhow!("start packaging MCP server: {error}"))?
        .waiting()
        .await
        .map_err(|error| anyhow::anyhow!("packaging MCP server stopped with error: {error}"))?;
    Ok(())
}
