// apps/remi/src/server/routes/mcp.rs
//! External admin MCP endpoint wiring.

use super::*;

pub(super) fn create_mcp_router(
    state: Arc<RwLock<ServerState>>,
) -> Router<Arc<RwLock<ServerState>>> {
    let state_for_mcp = state;
    let mcp_service = rmcp::transport::streamable_http_server::StreamableHttpService::new(
        move || {
            Ok(crate::server::mcp::RemiMcpServer::new(
                state_for_mcp.clone(),
            ))
        },
        Arc::new(
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
        ),
        Default::default(),
    );
    let mcp_service = tower::service_fn(move |request: Request<Body>| {
        let mut service = mcp_service.clone();
        async move {
            if let Some(err) = mcp_scope_error(&request) {
                return Ok::<Response, Infallible>(err);
            }
            service
                .call(request)
                .await
                .map(|response| response.map(Body::new))
        }
    });

    Router::<Arc<RwLock<ServerState>>>::new()
        .nest_service("/mcp", mcp_service)
        .route_layer(middleware::from_fn(
            |request: Request<Body>, next: Next| async move {
                if let Some(err) = mcp_scope_error(&request) {
                    return err;
                }
                next.run(request).await
            },
        ))
}
