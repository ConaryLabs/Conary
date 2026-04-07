// apps/conaryd/src/bin/conaryd.rs
//! Standalone conaryd daemon binary.

use anyhow::Result;
use clap::Parser;
use conaryd::daemon::{DaemonConfig, run_daemon};
use std::path::PathBuf;

/// conaryd — Conary system daemon
///
/// Provides a REST API for package operations with SSE progress
/// streaming and job queue management.
#[derive(Parser)]
#[command(name = "conaryd", version, about)]
struct Args {
    /// Database path
    #[arg(long, default_value = DaemonConfig::DEFAULT_DB_PATH)]
    db: String,

    /// Unix socket path
    #[arg(long, default_value = DaemonConfig::DEFAULT_SOCKET_PATH)]
    socket: String,

    /// Optional TCP bind address (e.g., 127.0.0.1:7890)
    #[arg(long)]
    tcp: Option<String>,

    /// Run in foreground (don't daemonize)
    #[arg(long)]
    foreground: bool,
}

fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let config = DaemonConfig {
        db_path: PathBuf::from(args.db),
        socket_path: PathBuf::from(args.socket),
        enable_tcp: args.tcp.is_some(),
        tcp_bind: args.tcp,
        ..Default::default()
    };

    tokio::runtime::Runtime::new()
        .expect("Failed to create Tokio runtime")
        .block_on(async {
            run_daemon(config)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_defaults_match_canonical_daemon_defaults() {
        let args = Args::parse_from(["conaryd"]);
        assert_eq!(args.db, DaemonConfig::DEFAULT_DB_PATH);
        assert_eq!(args.socket, DaemonConfig::DEFAULT_SOCKET_PATH);
        assert_eq!(args.tcp, None);
    }
}
