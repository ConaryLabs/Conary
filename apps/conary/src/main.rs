// apps/conary/src/main.rs
//! Conary Package Manager entrypoint.

mod app;
mod cli;
mod commands;
mod dispatch;
mod live_host_safety;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    app::run().await
}
