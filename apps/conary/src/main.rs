// apps/conary/src/main.rs
//! Conary Package Manager entrypoint.

mod app;
mod cli;
mod commands;
mod dispatch;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    app::run().await
}
