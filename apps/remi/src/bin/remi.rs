// apps/remi/src/bin/remi.rs
//! Standalone Remi package server binary.

use anyhow::Result;
use clap::Parser;
use remi::server::{RemiConfig, run_server_from_config};
use std::path::PathBuf;

/// Remi — CCS conversion proxy and package server
///
/// Proxies upstream package repositories and converts legacy packages
/// (RPM/DEB/Arch) to CCS format on-demand.
#[derive(Parser)]
#[command(name = "remi", version, about)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<String>,

    /// Override bind address (default from config or 0.0.0.0:8080)
    #[arg(long)]
    bind: Option<String>,

    /// Override admin bind address (default from config or 127.0.0.1:8081)
    #[arg(long)]
    admin_bind: Option<String>,

    /// Storage root directory (default from config or /conary)
    #[arg(long)]
    storage: Option<String>,

    /// Initialize storage directories if they don't exist
    #[arg(long)]
    init: bool,

    /// Validate configuration and exit
    #[arg(long)]
    validate: bool,
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

    let only_init = args.init
        && args.bind.is_none()
        && args.admin_bind.is_none()
        && args.storage.is_none()
        && args.config.is_none();

    // Load or create configuration
    let mut remi_config = if let Some(config_path) = args.config {
        RemiConfig::load(&PathBuf::from(&config_path))?
    } else {
        // Look for default config locations
        let default_paths = [
            PathBuf::from("/etc/conary/remi.toml"),
            PathBuf::from("remi.toml"),
        ];

        let mut found_config = None;
        for path in &default_paths {
            if path.exists() {
                println!("Using config: {}", path.display());
                found_config = Some(RemiConfig::load(path)?);
                break;
            }
        }
        found_config.unwrap_or_else(RemiConfig::new)
    };

    // Apply CLI overrides
    if let Some(bind_addr) = args.bind {
        remi_config.server.bind = bind_addr;
    }
    if let Some(admin_addr) = args.admin_bind {
        remi_config.server.admin_bind = admin_addr;
    }
    if let Some(storage_path) = args.storage {
        remi_config.storage.root = PathBuf::from(storage_path);
    }

    // Validate configuration
    if let Err(e) = remi_config.validate() {
        eprintln!("Configuration error: {}", e);
        std::process::exit(1);
    }

    if args.validate {
        println!("Configuration is valid.");
        println!("  Public API:   {}", remi_config.server.bind);
        println!("  Admin API:    {}", remi_config.server.admin_bind);
        println!("  Storage root: {}", remi_config.storage.root.display());
        return Ok(());
    }

    // Initialize directories if requested
    if args.init {
        println!("Initializing Remi storage directories...");
        for dir in remi_config.storage_dirs() {
            if !dir.exists() {
                println!("  Creating: {}", dir.display());
                std::fs::create_dir_all(&dir)?;
            }
        }
        println!("Storage directories initialized.");

        // If only init was requested, exit
        if only_init {
            return Ok(());
        }
    }

    // Run the async server
    tokio::runtime::Runtime::new()
        .expect("Failed to create Tokio runtime")
        .block_on(run_server_from_config(&remi_config))
}
