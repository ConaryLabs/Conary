// apps/conary/src/app.rs
//! Conary application bootstrap and top-level error presentation.

use anyhow::Result;
use clap::Parser;

use crate::cli::Cli;
use crate::dispatch;

pub async fn run() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    conary_core::scriptlet::set_seccomp_warn_override(cli.seccomp_warn);

    if let Err(err) = dispatch::dispatch(cli).await {
        report_error(&err);
        std::process::exit(1);
    }

    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

fn report_error(err: &anyhow::Error) {
    if let Some(core_err) = err.downcast_ref::<conary_core::Error>() {
        match core_err {
            conary_core::Error::DatabaseNotFound(_) => {
                eprintln!("Error: Database not initialized.");
                eprintln!("Run 'conary system init' to set up the package database.");
            }
            conary_core::Error::NotFound(detail) => {
                eprintln!("Error: {detail}");
            }
            conary_core::Error::ConflictError(detail) => {
                eprintln!("Error: Conflict -- {detail}");
                eprintln!("Try 'conary remove' first or use '--force' if available.");
            }
            conary_core::Error::PathTraversal(detail) => {
                eprintln!("Error: Path safety violation -- {detail}");
                eprintln!("This may indicate a malicious or corrupt package.");
            }
            other => {
                eprintln!("Error: {other}");
            }
        }
    } else {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| format!("{err:#}"))) {
            Ok(msg) => eprintln!("Error: {msg}"),
            Err(_) => eprintln!("Error: {err}"),
        }
    }
}
