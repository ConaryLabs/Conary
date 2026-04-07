// apps/conary/src/main.rs
//! Conary Package Manager entrypoint.

mod app;
mod cli;
mod commands;
mod dispatch;
mod live_host_safety;

#[tokio::main]
async fn main() {
    let code = conary_bootstrap::finish(app::run().await, app::report_error, 1);
    if code != 0 {
        std::process::exit(code);
    }
}
