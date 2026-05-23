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
fn stateless_module_does_not_use_rmcp_or_session_types() {
    let source =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/stateless.rs"))
            .expect("stateless module should be readable");

    for forbidden in [
        "use rmcp",
        "rmcp::",
        "RoleServer",
        "ServerHandler",
        "LocalSessionManager",
        "Mcp-Session-Id",
        "InitializeResult",
    ] {
        assert!(
            !source.contains(forbidden),
            "stateless module must not depend on legacy/session MCP type {forbidden}"
        );
    }
}

#[test]
fn live_mcp_route_files_do_not_contain_draft_stateless_identifiers() {
    let root = repo_root();
    for path in [
        "apps/remi/src/server/routes/mcp.rs",
        "apps/conary-test/src/server/routes.rs",
    ] {
        let source =
            fs::read_to_string(root.join(path)).expect("live MCP route file should be readable");
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
