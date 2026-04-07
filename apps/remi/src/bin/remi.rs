// apps/remi/src/bin/remi.rs
//! Standalone Remi package server binary.

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use remi::server::{
    IndexGenConfig, PrewarmConfig, ProxyConfig, RemiConfig, generate_indices, run_prewarm,
    run_proxy, run_server_from_config,
};
use remi::trust;
use std::path::PathBuf;

/// Remi — CCS conversion proxy and package server.
///
/// With no subcommand, `remi` starts the main service. Use explicit subcommands
/// for proxying, cache prewarming, or repository-admin utilities.
#[derive(Parser)]
#[command(name = "remi", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Run a zero-config Remi LAN proxy.
    Proxy(ProxyArgs),
    /// Generate repository indices from the chunk store.
    IndexGen(IndexGenArgs),
    /// Pre-warm the chunk cache by converting popular packages.
    Prewarm(PrewarmArgs),
    /// Remi-owned trust admin commands.
    Trust {
        #[command(subcommand)]
        command: TrustCommand,
    },
}

#[derive(Args, Default)]
struct ServeArgs {
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

#[derive(Args)]
struct ProxyArgs {
    /// Port to listen on
    #[arg(long, default_value = "7891")]
    port: u16,

    /// Explicit upstream Remi URL (skips mDNS discovery)
    #[arg(long)]
    upstream: Option<String>,

    /// Disable mDNS auto-discovery
    #[arg(long)]
    no_mdns: bool,

    /// Local cache directory
    #[arg(long, default_value = "/var/cache/conary/proxy")]
    cache_dir: String,

    /// Serve only from cache (no upstream)
    #[arg(long)]
    offline: bool,

    /// Don't advertise via mDNS
    #[arg(long)]
    no_advertise: bool,
}

#[derive(Args)]
struct IndexGenArgs {
    /// Database path
    #[arg(long, default_value = "/var/lib/conary/conary.db")]
    db: String,

    /// Path to chunk storage directory
    #[arg(long, default_value = "/var/lib/conary/data/chunks")]
    chunk_dir: String,

    /// Output directory for generated index files
    #[arg(short, long, default_value = "/var/lib/conary/data/repo")]
    output_dir: String,

    /// Distribution to generate index for (arch, fedora, ubuntu, debian)
    #[arg(long)]
    distro: Option<String>,

    /// Sign the index with the specified key file
    #[arg(long)]
    sign_key: Option<String>,
}

#[derive(Args)]
struct PrewarmArgs {
    /// Database path
    #[arg(long, default_value = "/var/lib/conary/conary.db")]
    db: String,

    /// Path to chunk storage directory
    #[arg(long, default_value = "/var/lib/conary/data/chunks")]
    chunk_dir: String,

    /// Path to cache/scratch directory
    #[arg(long, default_value = "/var/lib/conary/data/cache")]
    cache_dir: String,

    /// Distribution to pre-warm (arch, fedora, ubuntu, debian)
    #[arg(long)]
    distro: String,

    /// Maximum number of packages to convert
    #[arg(long, default_value = "100")]
    max_packages: usize,

    /// Path to popularity data file (JSON with name/score pairs)
    #[arg(long)]
    popularity_file: Option<String>,

    /// Only convert packages matching this regex pattern
    #[arg(long)]
    pattern: Option<String>,

    /// Show what would be converted without actually converting
    #[arg(long)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum TrustCommand {
    /// Sign targets metadata for a repository.
    SignTargets(TrustSignTargetsArgs),
    /// Rotate a TUF role key.
    RotateKey(TrustRotateKeyArgs),
}

#[derive(Args)]
struct TrustSignTargetsArgs {
    /// Repository name
    repo: String,

    /// Path to signing key
    #[arg(long)]
    key: String,

    /// Path to the package database
    #[arg(long, default_value = "/var/lib/conary/conary.db")]
    db: String,
}

#[derive(Args)]
struct TrustRotateKeyArgs {
    /// Role to rotate (root, targets, snapshot, timestamp)
    role: String,

    /// Path to old key file
    #[arg(long)]
    old_key: String,

    /// Path to new key file
    #[arg(long)]
    new_key: String,

    /// Path to root key file (for signing the new root)
    #[arg(long)]
    root_key: String,

    /// Repository name
    repo: String,

    /// Path to the package database
    #[arg(long, default_value = "/var/lib/conary/conary.db")]
    db: String,
}

fn main() {
    conary_bootstrap::init_tracing();

    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Proxy(args)) => run_proxy_command(args),
        Some(Command::IndexGen(args)) => run_index_gen_command(args),
        Some(Command::Prewarm(args)) => run_prewarm_command(args),
        Some(Command::Trust { command }) => run_trust_command(command),
        None => run_server_command(cli.serve),
    };

    let code = finish_main(result);
    if code != 0 {
        std::process::exit(code);
    }
}

fn report_top_level_error(err: &anyhow::Error) {
    eprintln!("Error: {err:?}");
}

fn finish_main(result: anyhow::Result<()>) -> i32 {
    conary_bootstrap::finish(result, report_top_level_error, 101)
}

fn run_server_command(args: ServeArgs) -> Result<()> {
    let only_init = args.init
        && args.bind.is_none()
        && args.admin_bind.is_none()
        && args.storage.is_none()
        && args.config.is_none();

    let default_paths = [
        PathBuf::from("/etc/conary/remi.toml"),
        PathBuf::from("remi.toml"),
    ];
    let mut remi_config = load_remi_config(&args, &default_paths)?;
    apply_serve_overrides(&mut remi_config, &args);

    if let Err(err) = remi_config.validate() {
        eprintln!("Configuration error: {err}");
        std::process::exit(1);
    }

    if args.validate {
        println!("Configuration is valid.");
        println!("  Public API:   {}", remi_config.server.bind);
        println!("  Admin API:    {}", remi_config.server.admin_bind);
        println!("  Storage root: {}", remi_config.storage.root.display());
        return Ok(());
    }

    if args.init {
        println!("Initializing Remi storage directories...");
        for dir in remi_config.storage_dirs() {
            if !dir.exists() {
                println!("  Creating: {}", dir.display());
                std::fs::create_dir_all(&dir)?;
            }
        }
        println!("Storage directories initialized.");

        if only_init {
            return Ok(());
        }
    }

    conary_bootstrap::run_with_runtime(|| run_server_from_config(&remi_config))
}

fn load_remi_config(args: &ServeArgs, default_paths: &[PathBuf]) -> Result<RemiConfig> {
    if let Some(config_path) = args.config.as_ref() {
        return RemiConfig::load(&PathBuf::from(config_path));
    }

    for path in default_paths {
        if path.exists() {
            println!("Using config: {}", path.display());
            return RemiConfig::load(path);
        }
    }

    Ok(RemiConfig::new())
}

fn apply_serve_overrides(config: &mut RemiConfig, args: &ServeArgs) {
    if let Some(bind_addr) = args.bind.as_ref() {
        config.server.bind = bind_addr.clone();
    }
    if let Some(admin_addr) = args.admin_bind.as_ref() {
        config.server.admin_bind = admin_addr.clone();
    }
    if let Some(storage_path) = args.storage.as_ref() {
        config.storage.root = PathBuf::from(storage_path);
    }
}

fn run_proxy_command(args: ProxyArgs) -> Result<()> {
    let config = ProxyConfig {
        port: args.port,
        upstream_url: args.upstream,
        cache_dir: PathBuf::from(args.cache_dir),
        mdns_enabled: !args.no_mdns,
        mdns_scan_secs: 3,
        offline: args.offline,
        advertise: !args.no_advertise,
    };

    if let Some(parent) = config.cache_dir.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(&config.cache_dir)?;

    conary_bootstrap::run_with_runtime(move || run_proxy(config))
}

fn run_index_gen_command(args: IndexGenArgs) -> Result<()> {
    let config = IndexGenConfig {
        db_path: args.db,
        chunk_dir: args.chunk_dir,
        output_dir: args.output_dir,
        distro: args.distro,
        sign_key: args.sign_key,
    };

    let results = generate_indices(&config)?;
    if results.is_empty() {
        println!("No indices generated.");
    } else {
        for result in results {
            println!(
                "{}: {} packages ({} versions) -> {}{}",
                result.distro,
                result.package_count,
                result.version_count,
                result.index_path,
                if result.signed { " [signed]" } else { "" }
            );
        }
    }

    Ok(())
}

fn run_prewarm_command(args: PrewarmArgs) -> Result<()> {
    let config = PrewarmConfig {
        db_path: args.db,
        chunk_dir: args.chunk_dir,
        cache_dir: args.cache_dir,
        distro: args.distro,
        max_packages: args.max_packages,
        popularity_file: args.popularity_file,
        pattern: args.pattern,
        dry_run: args.dry_run,
    };

    let result = run_prewarm(&config)?;
    println!("Pre-warm complete:");
    println!("  Processed:  {}", result.packages_processed);
    println!("  Converted:  {}", result.packages_converted);
    println!("  Skipped:    {}", result.packages_skipped);
    println!("  Failed:     {}", result.packages_failed);
    println!("  Total size: {} bytes", result.total_bytes);

    if !result.converted.is_empty() {
        println!("\nConverted packages:");
        for package in &result.converted {
            println!("  {}", package);
        }
    }

    if !result.failed.is_empty() {
        println!("\nFailed packages:");
        for (package, error) in &result.failed {
            println!("  {}: {}", package, error);
        }
    }

    Ok(())
}

fn run_trust_command(command: TrustCommand) -> Result<()> {
    match command {
        TrustCommand::SignTargets(args) => trust::sign_targets(&args.repo, &args.key, &args.db),
        TrustCommand::RotateKey(args) => trust::rotate_key(
            &args.role,
            &args.old_key,
            &args.new_key,
            &args.root_key,
            &args.repo,
            &args.db,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_finish_main_returns_zero_on_success() {
        assert_eq!(finish_main(Ok(())), 0);
    }

    #[test]
    fn test_finish_main_preserves_101_on_top_level_failure() {
        assert_eq!(finish_main(Err(anyhow::anyhow!("boom"))), 101);
    }

    fn write_config(path: &std::path::Path, bind: &str, admin_bind: &str, storage_root: &str) {
        let config = format!(
            r#"
[server]
bind = "{bind}"
admin_bind = "{admin_bind}"

[storage]
root = "{storage_root}"
"#
        );
        std::fs::write(path, config).unwrap();
    }

    #[test]
    fn test_load_remi_config_prefers_explicit_config_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let explicit_path = temp_dir.path().join("explicit.toml");
        let fallback_path = temp_dir.path().join("fallback.toml");

        write_config(
            &explicit_path,
            "127.0.0.1:9001",
            "127.0.0.1:9002",
            "/explicit",
        );
        write_config(
            &fallback_path,
            "127.0.0.1:9101",
            "127.0.0.1:9102",
            "/fallback",
        );

        let args = ServeArgs {
            config: Some(explicit_path.display().to_string()),
            ..ServeArgs::default()
        };

        let config = load_remi_config(&args, &[fallback_path]).unwrap();

        assert_eq!(config.server.bind, "127.0.0.1:9001");
        assert_eq!(config.server.admin_bind, "127.0.0.1:9002");
        assert_eq!(config.storage.root, PathBuf::from("/explicit"));
    }

    #[test]
    fn test_load_remi_config_uses_first_existing_default_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let first_path = temp_dir.path().join("first.toml");
        let second_path = temp_dir.path().join("second.toml");

        write_config(&first_path, "127.0.0.1:9201", "127.0.0.1:9202", "/first");
        write_config(&second_path, "127.0.0.1:9301", "127.0.0.1:9302", "/second");

        let config = load_remi_config(&ServeArgs::default(), &[first_path, second_path]).unwrap();

        assert_eq!(config.server.bind, "127.0.0.1:9201");
        assert_eq!(config.server.admin_bind, "127.0.0.1:9202");
        assert_eq!(config.storage.root, PathBuf::from("/first"));
    }

    #[test]
    fn test_apply_serve_overrides_wins_over_file_values() {
        let mut config = RemiConfig::default();
        config.server.bind = "127.0.0.1:9401".to_string();
        config.server.admin_bind = "127.0.0.1:9402".to_string();
        config.storage.root = PathBuf::from("/from-config");

        let args = ServeArgs {
            bind: Some("0.0.0.0:9501".to_string()),
            admin_bind: Some("127.0.0.1:9502".to_string()),
            storage: Some("/from-cli".to_string()),
            ..ServeArgs::default()
        };

        apply_serve_overrides(&mut config, &args);

        assert_eq!(config.server.bind, "0.0.0.0:9501");
        assert_eq!(config.server.admin_bind, "127.0.0.1:9502");
        assert_eq!(config.storage.root, PathBuf::from("/from-cli"));
    }
}
