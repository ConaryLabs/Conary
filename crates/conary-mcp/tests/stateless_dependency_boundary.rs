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
fn remi_and_legacy_mcp_files_do_not_contain_draft_stateless_identifiers() {
    let root = repo_root();
    for path in [
        "apps/remi/src/server/mcp.rs",
        "apps/remi/src/server/routes/mcp.rs",
        "apps/conary-test/src/server/mcp.rs",
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
                "{path} must not contain draft stateless identifier '{forbidden}'"
            );
        }
    }
}

#[test]
fn conary_test_routes_only_mounts_stateless_adapter() {
    let root = repo_root();
    let path = "apps/conary-test/src/server/routes.rs";
    let source = fs::read_to_string(root.join(path)).expect("routes file should be readable");

    assert!(
        source.contains("\"/mcp/stateless\""),
        "{path} should mount the stateless discovery route"
    );
    assert!(
        source.contains("stateless_mcp::handle"),
        "{path} should delegate stateless protocol handling to stateless_mcp"
    );

    for forbidden in [
        "MCP-Protocol-Version",
        "Mcp-Method",
        "Mcp-Name",
        "DRAFT-2026-v1",
        "io.modelcontextprotocol/",
        "handle_stateless_http_request",
        "handle_stateless_http_bytes",
        "server/discover",
    ] {
        assert!(
            !source.contains(forbidden),
            "{path} must only mount the stateless adapter, not contain protocol logic '{forbidden}'"
        );
    }
}

#[test]
fn conary_test_stateless_adapter_does_not_use_rmcp_session_types() {
    let root = repo_root();
    let path = "apps/conary-test/src/server/stateless_mcp.rs";
    let source =
        fs::read_to_string(root.join(path)).expect("stateless MCP adapter file should be readable");

    for forbidden in [
        "use rmcp",
        "rmcp::",
        "RoleServer",
        "ServerHandler",
        "StreamableHttpService",
        "LocalSessionManager",
        "Mcp-Session-Id",
        "InitializeResult",
    ] {
        assert!(
            !source.contains(forbidden),
            "{path} must not depend on legacy/session type {forbidden}"
        );
    }
}
