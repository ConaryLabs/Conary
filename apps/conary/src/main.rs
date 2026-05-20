// apps/conary/src/main.rs
//! Conary Package Manager entrypoint.

#[tokio::main]
async fn main() {
    let code = conary_bootstrap::finish(conary::app::run().await, conary::app::report_error, 1);
    if code != 0 {
        std::process::exit(code);
    }
}
