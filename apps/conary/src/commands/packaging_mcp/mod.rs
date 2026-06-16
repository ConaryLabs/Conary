// apps/conary/src/commands/packaging_mcp/mod.rs
//! Local packaging MCP command surface.

use anyhow::Result;

pub(crate) mod projection;
pub(crate) mod records;

pub async fn cmd_mcp_packaging() -> Result<()> {
    anyhow::bail!("packaging MCP server wiring is added in the next task")
}
