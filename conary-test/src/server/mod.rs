// conary-test/src/server/mod.rs

pub mod auth;
pub mod handlers;
pub mod mcp;
pub mod routes;
pub mod service;
pub mod state;

pub use routes::create_router;
pub use state::AppState;

pub async fn run_server(state: AppState, port: u16, token: Option<String>) -> anyhow::Result<()> {
    let app = create_router(state, token);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("conary-test server listening on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}
