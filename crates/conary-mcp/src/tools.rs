// crates/conary-mcp/src/tools.rs
//! Reusable MCP tool response helpers for Conary agent-contract results.

use rmcp::{ErrorData as McpError, model::*};

pub fn contract_tool_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let text = crate::contract_json_text(value)?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize)]
    struct Payload {
        status: &'static str,
    }

    #[test]
    fn contract_tool_result_serializes_json_text_content() {
        let result = contract_tool_result(&Payload { status: "ok" }).unwrap();
        assert_eq!(result.content.len(), 1);
        let text = result.content[0].as_text().expect("text content");
        assert!(text.text.contains("\"status\": \"ok\""));
    }
}
