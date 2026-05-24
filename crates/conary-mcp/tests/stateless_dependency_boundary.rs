// crates/conary-mcp/tests/stateless_dependency_boundary.rs
//! Guard tests for the stateless MCP compliance harness boundary.

use std::{fs, path::PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("conary-mcp should live under crates/")
        .to_path_buf()
}

#[test]
fn stateless_modules_do_not_use_rmcp_or_live_http_framework_types() {
    for module_path in ["src/stateless.rs", "src/stateless_http.rs"] {
        let source =
            fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(module_path))
                .expect("stateless module should be readable");

        for forbidden in [
            "use rmcp",
            "rmcp::",
            "RoleServer",
            "ServerHandler",
            "StreamableHttpService",
            "LocalSessionManager",
            "Mcp-Session-Id",
            "InitializeResult",
            "use axum",
            "axum::",
        ] {
            assert!(
                !source.contains(forbidden),
                "{module_path} must not depend on legacy/session/live HTTP type {forbidden}"
            );
        }
    }
}

#[test]
fn live_mcp_server_files_do_not_contain_draft_stateless_identifiers() {
    let root = repo_root();
    for path in [
        "apps/remi/src/server/mcp.rs",
        "apps/remi/src/server/routes/mcp.rs",
        "apps/conary-test/src/server/mcp.rs",
        "apps/conary-test/src/server/routes.rs",
    ] {
        let source =
            fs::read_to_string(root.join(path)).expect("live MCP server file should be readable");
        for forbidden in [
            "Mcp-Method",
            "Mcp-Name",
            "server/discover",
            "MCP-Protocol-Version",
            "DRAFT-2026-v1",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} must not contain draft stateless identifier '{forbidden}' until a live adapter slice adds it"
            );
        }
    }
}
