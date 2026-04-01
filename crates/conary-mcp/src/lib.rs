// crates/conary-mcp/src/lib.rs
//! Shared MCP (Model Context Protocol) helpers.
//!
//! Utility functions used by multiple workspace MCP server implementations.

use std::fmt::Display;

use rmcp::{ErrorData as McpError, model::*};

/// Serialize a value to pretty JSON, mapping failures to [`McpError`].
pub fn to_json_text<T: serde::Serialize>(value: &T) -> Result<String, McpError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))
}

/// Validate a path parameter against a safe pattern for URL interpolation.
///
/// Rejects values containing slashes, `..`, null bytes, or characters
/// outside `[a-zA-Z0-9._-]`.
pub fn validate_path_param(value: &str, param_name: &str) -> Result<(), McpError> {
    if value.is_empty()
        || value.contains('/')
        || value.contains("..")
        || value.contains('\0')
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        Err(McpError::invalid_params(
            format!("Invalid {param_name}: must match [a-zA-Z0-9._-]+"),
            None,
        ))
    } else {
        Ok(())
    }
}

/// Build an [`McpError`] for a resource-not-found condition.
pub fn map_not_found(entity: &str, id: impl Display) -> McpError {
    McpError::resource_not_found(format!("{entity} '{id}' not found"), None)
}

/// Build an [`McpError`] for an internal/unexpected failure.
pub fn map_internal(err: impl Display) -> McpError {
    McpError::internal_error(err.to_string(), None)
}

/// Build the [`ServerInfo`] boilerplate shared by all Conary MCP servers.
///
/// Creates an `InitializeResult` with tools enabled, the given server
/// name/version, and human-readable instructions.
pub fn server_info(name: &str, version: &str, instructions: &str) -> ServerInfo {
    InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
        .with_server_info(Implementation::new(name, version))
        .with_instructions(instructions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_json_text_serializes_pretty() {
        let value = serde_json::json!({"a": 1, "b": [2, 3]});
        let text = to_json_text(&value).expect("serialization should succeed");
        assert!(text.contains('\n'), "output should be pretty-printed");
        assert!(text.contains("\"a\": 1"));
    }

    #[test]
    fn validate_path_param_rejects_slash() {
        let result = validate_path_param("foo/bar", "test");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("Invalid test"));
    }

    #[test]
    fn validate_path_param_accepts_valid() {
        assert!(validate_path_param("ci.yaml", "workflow").is_ok());
        assert!(validate_path_param("my-workflow_v2.0", "workflow").is_ok());
    }
}
