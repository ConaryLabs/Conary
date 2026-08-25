// apps/remi/src/bin/remi.rs
//! Standalone Remi package server binary.

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use remi::server::{
    ConversionCrawlConfig, IndexGenConfig, PrewarmConfig, ProfileRevisionSelection, ProxyConfig,
    RemiConfig, generate_indices, run_conversion_crawl, run_prewarm, run_proxy,
    run_server_from_config,
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
    /// Convert every exact package variant in every public profile.
    ConversionCrawl(ConversionCrawlArgs),
    /// Atomically activate one completely proven public candidate universe.
    PromotionActivate(PromotionActivateArgs),
    /// Record reproducible conversion latency and work evidence.
    ConversionBenchmark(ConversionBenchmarkArgs),
    /// Remi-owned trust admin commands.
    Trust {
        #[command(subcommand)]
        command: TrustCommand,
    },
    /// Prepare or roll back an atomic service deployment transition.
    Deployment {
        #[command(subcommand)]
        command: DeploymentCommand,
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

    /// Root containing immutable activated source and profile catalogs
    #[arg(long, default_value = "/var/lib/conary/data/catalogs")]
    catalog_dir: String,

    /// Output directory for generated index files
    #[arg(short, long, default_value = "/var/lib/conary/data/repo")]
    output_dir: String,

    /// Exact source profile to generate (fedora-44, ubuntu-26.04, arch)
    #[arg(long)]
    source_profile: Option<String>,

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

    /// Directory containing per-distro TUF authority keys
    #[arg(long)]
    repository_keys_dir: Option<PathBuf>,

    /// Distribution to pre-warm (fedora-44, ubuntu-26.04, arch)
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

#[derive(Args)]
struct ConversionCrawlArgs {
    /// Database containing durable registered profile revisions and repository keys.
    #[arg(long, default_value = "/var/lib/conary/conary.db")]
    db: PathBuf,

    /// Root containing immutable source and profile catalogs.
    #[arg(long, default_value = "/var/lib/conary/data/catalogs")]
    catalog_dir: PathBuf,

    /// Path to chunk storage directory.
    #[arg(long, default_value = "/var/lib/conary/data/chunks")]
    chunk_dir: PathBuf,

    /// Path to cache and conversion scratch storage.
    #[arg(long, default_value = "/var/lib/conary/data/cache")]
    cache_dir: PathBuf,

    /// Directory containing exact per-profile TUF signing authority.
    #[arg(long)]
    repository_keys_dir: PathBuf,

    /// Exact public candidate as PROFILE=REVISION; repeat in canonical order.
    #[arg(
        long = "candidate",
        value_name = "PROFILE=SHA256",
        required = true,
        value_parser = parse_candidate
    )]
    candidates: Vec<ProfileRevisionSelection>,

    /// Canonical crawl evidence output path.
    #[arg(long)]
    output: PathBuf,

    /// Maximum number of conversions running concurrently; scope is unchanged.
    #[arg(long, default_value = "4")]
    concurrency: usize,
}

#[derive(Args)]
struct PromotionActivateArgs {
    /// Current Remi service configuration; the runtime must be stopped.
    #[arg(long, default_value = "/etc/conary/remi.toml")]
    config: PathBuf,

    /// Canonical RemiPromotionEvidenceV1 artifact.
    #[arg(long)]
    promotion_evidence: PathBuf,

    /// Exact complete RemiConversionCrawlV4 artifact bound by the evidence.
    #[arg(long)]
    conversion_crawl: PathBuf,
}

#[derive(Args)]
struct ConversionBenchmarkArgs {
    /// Database path
    #[arg(long, default_value = "/var/lib/conary/conary.db")]
    db: String,

    /// Path to chunk storage directory
    #[arg(long, default_value = "/var/lib/conary/data/chunks")]
    chunk_dir: String,

    /// Path to cache/scratch directory
    #[arg(long, default_value = "/var/lib/conary/data/cache")]
    cache_dir: String,

    /// Directory containing per-distro TUF authority keys
    #[arg(long)]
    repository_keys_dir: Option<PathBuf>,

    /// Distribution to benchmark (fedora-44, ubuntu-26.04, arch)
    #[arg(long)]
    distro: String,

    /// Package names to benchmark. Repeat the flag for multiple packages.
    #[arg(long = "package")]
    packages: Vec<String>,

    /// Stable operator-defined identity for the benchmark hardware.
    #[arg(long)]
    hardware_label: String,

    /// Runs per exact package subject. Two records cold and warm behavior.
    #[arg(long, default_value = "2")]
    iterations: usize,

    /// Emit JSON lines instead of pretty JSON.
    #[arg(long)]
    jsonl: bool,

    /// Optional R2 endpoint. When omitted, R2 write-through timing is recorded as skipped.
    #[arg(long)]
    r2_endpoint: Option<String>,

    /// Optional R2 bucket name.
    #[arg(long, default_value = "conary-chunks")]
    r2_bucket: String,

    /// Optional R2 key prefix.
    #[arg(long, default_value = "chunks/")]
    r2_prefix: String,

    /// Optional R2 region string.
    #[arg(long, default_value = "auto")]
    r2_region: String,
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

#[derive(Subcommand)]
enum DeploymentCommand {
    /// Initialize the dedicated universe keys and durable public metadata root.
    InitializeUniverseAuthority(DeploymentUniverseAuthorityArgs),
    /// Back up current state and prepare current config/schema authority.
    Prepare(DeploymentPrepareArgs),
    /// Restore a prepared deployment transition.
    Rollback(DeploymentRollbackArgs),
    /// Verify current schema, source authority, and repopulation state.
    Inspect(DeploymentInspectArgs),
}

#[derive(Args)]
struct DeploymentUniverseAuthorityArgs {
    /// Durable exact-profile and universe signing authority root.
    #[arg(long, default_value = "/conary/repository-keys")]
    repository_keys_dir: PathBuf,
}

#[derive(Args)]
struct DeploymentPrepareArgs {
    /// Current Remi service configuration.
    #[arg(long, default_value = "/etc/conary/remi.toml")]
    config: PathBuf,

    /// Staged typed repository manifest.
    #[arg(long)]
    repository_manifest: PathBuf,

    /// Installed typed repository manifest path.
    #[arg(long, default_value = "/etc/conary/remi-repositories.toml")]
    repository_manifest_target: PathBuf,

    /// Durable exact-profile repository signing authority.
    #[arg(long, default_value = "/conary/repository-keys")]
    repository_keys_dir: PathBuf,

    /// Stable deployment identity used in the recoverable backup name.
    #[arg(long)]
    deployment_id: String,

    /// Maximum concurrent package conversions.
    #[arg(long)]
    max_concurrent: usize,
}

#[derive(Args)]
struct DeploymentRollbackArgs {
    /// Transition manifest emitted by `deployment prepare`.
    #[arg(long)]
    manifest: PathBuf,
}

#[derive(Args)]
struct DeploymentInspectArgs {
    /// Current Remi service configuration.
    #[arg(long, default_value = "/etc/conary/remi.toml")]
    config: PathBuf,

    /// Fail until every source has metadata and converted artifacts.
    #[arg(long)]
    require_repopulated: bool,
}

fn main() {
    conary_bootstrap::init_server_tracing();

    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Proxy(args)) => run_proxy_command(args),
        Some(Command::IndexGen(args)) => run_index_gen_command(args),
        Some(Command::Prewarm(args)) => run_prewarm_command(args),
        Some(Command::ConversionCrawl(args)) => run_conversion_crawl_command(args),
        Some(Command::PromotionActivate(args)) => run_promotion_activate_command(args),
        Some(Command::ConversionBenchmark(args)) => run_conversion_benchmark_command(args),
        Some(Command::Trust { command }) => run_trust_command(command),
        Some(Command::Deployment { command }) => run_deployment_command(command),
        None => run_server_command(cli.serve),
    };

    let code = finish_main(result);
    if code != 0 {
        std::process::exit(code);
    }
}

fn run_deployment_command(command: DeploymentCommand) -> Result<()> {
    match command {
        DeploymentCommand::InitializeUniverseAuthority(args) => {
            let root = remi::deployment::initialize_universe_authority(&args.repository_keys_dir)?;
            println!("{}", root.display());
        }
        DeploymentCommand::Prepare(args) => {
            let manifest = remi::deployment::prepare(&remi::deployment::PrepareOptions {
                config_path: args.config,
                repository_manifest_source: args.repository_manifest,
                repository_manifest_target: args.repository_manifest_target,
                repository_keys_dir: args.repository_keys_dir,
                deployment_id: args.deployment_id,
                max_concurrent: args.max_concurrent,
            })?;
            println!("{}", manifest.display());
        }
        DeploymentCommand::Rollback(args) => {
            remi::deployment::rollback(&args.manifest)?;
        }
        DeploymentCommand::Inspect(args) => {
            let state = remi::deployment::inspect_state(&args.config)?;
            println!("{}", serde_json::to_string_pretty(&state)?);
            if args.require_repopulated && !state.repopulation_complete() {
                anyhow::bail!("Remi immutable profile universe and conversions are not populated");
            }
        }
    }
    Ok(())
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

    if only_init {
        println!("Initializing Remi storage directories...");
        remi::server::initialize_storage_directories(&remi_config)?;
        println!("Storage directories initialized.");
        return Ok(());
    }

    run_server_from_config(&remi_config)
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
        catalog_dir: args.catalog_dir,
        output_dir: args.output_dir,
        source_profile: args.source_profile,
        sign_key: args.sign_key,
    };

    let results = generate_indices(&config)?;
    if results.is_empty() {
        println!("No indices generated.");
    } else {
        for result in results {
            println!(
                "{}: {} packages ({} versions) -> {}{}",
                result.source_profile,
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
        repository_keys_dir: args.repository_keys_dir,
        distro: args.distro,
        max_packages: args.max_packages,
        popularity_file: args.popularity_file,
        pattern: args.pattern,
        dry_run: args.dry_run,
    };

    conary_bootstrap::run_with_runtime(move || async move {
        let result = run_prewarm(&config).await?;
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
            for entry in &result.failed {
                println!(
                    "  {} [{}]: {}",
                    entry.package,
                    entry.failure.kind().as_str(),
                    entry.failure.detail()
                );
            }
        }

        Ok(())
    })
}

fn run_conversion_crawl_command(args: ConversionCrawlArgs) -> Result<()> {
    let config = ConversionCrawlConfig {
        db_path: args.db,
        catalog_dir: args.catalog_dir,
        chunk_dir: args.chunk_dir,
        cache_dir: args.cache_dir,
        repository_keys_dir: args.repository_keys_dir,
        output_path: args.output,
        concurrency: args.concurrency,
        candidates: args.candidates,
    };
    conary_bootstrap::run_with_runtime(move || async move {
        let report = run_conversion_crawl(&config).await?;
        let packages = report
            .profiles
            .iter()
            .map(|profile| profile.expected_packages)
            .sum::<u64>();
        println!(
            "Conversion crawl complete: {} public profiles, {} exact packages",
            report.profiles.len(),
            packages
        );
        println!("Evidence: {}", config.output_path.display());
        Ok(())
    })
}

fn run_promotion_activate_command(args: PromotionActivateArgs) -> Result<()> {
    let config = RemiConfig::load(&args.config)?;
    conary_bootstrap::run_with_runtime(move || async move {
        let outcome = remi::server::run_promotion_activation_from_config(
            &config,
            args.promotion_evidence,
            args.conversion_crawl,
        )
        .await?;
        println!("{}", serde_json::to_string_pretty(&outcome)?);
        Ok(())
    })
}

fn parse_candidate(value: &str) -> std::result::Result<ProfileRevisionSelection, String> {
    let (source_profile, profile_revision_sha256) = value
        .split_once('=')
        .ok_or_else(|| "candidate must be PROFILE=SHA256".to_string())?;
    if source_profile.is_empty() {
        return Err("candidate profile must not be empty".to_string());
    }
    if profile_revision_sha256.len() != 64
        || !profile_revision_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || profile_revision_sha256 != profile_revision_sha256.to_ascii_lowercase()
    {
        return Err("candidate revision must be an exact lowercase SHA-256 digest".to_string());
    }
    Ok(ProfileRevisionSelection {
        source_profile: source_profile.to_string(),
        profile_revision_sha256: profile_revision_sha256.to_string(),
    })
}

fn run_conversion_benchmark_command(args: ConversionBenchmarkArgs) -> Result<()> {
    anyhow::ensure!(args.iterations > 0, "--iterations must be at least 1");
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let r2_store = if let Some(endpoint) = args.r2_endpoint.clone() {
            let config = remi::server::r2::R2Config {
                endpoint,
                bucket: args.r2_bucket.clone(),
                prefix: args.r2_prefix.clone(),
                region: args.r2_region.clone(),
            };
            Some(std::sync::Arc::new(remi::server::R2Store::new(&config)?))
        } else {
            None
        };

        let service = remi::server::ConversionService::new(
            PathBuf::from(args.chunk_dir),
            PathBuf::from(args.cache_dir),
            PathBuf::from(args.db),
            r2_store,
        )
        .with_repository_keys_dir(args.repository_keys_dir);

        let samples = if args.packages.is_empty() {
            service.benchmark_size_class_samples(&args.distro).await?
        } else {
            let mut samples = Vec::with_capacity(args.packages.len());
            for package in &args.packages {
                samples.push(
                    service
                        .benchmark_explicit_sample(&args.distro, package)
                        .await?,
                );
            }
            samples
        };

        let environment =
            remi::server::ConversionBenchmarkEnvironment::capture(args.hardware_label.clone());

        for sample in samples {
            for iteration in 1..=args.iterations {
                let result = service
                    .benchmark_package_conversion(&args.distro, &sample, iteration, &environment)
                    .await;

                match result {
                    Ok(evidence) => {
                        if args.jsonl {
                            println!("{}", serde_json::to_string(&evidence)?);
                        } else {
                            println!("{}", serde_json::to_string_pretty(&evidence)?);
                        }
                    }
                    Err(err) => {
                        let evidence = serde_json::json!({
                            "schema_version": 1,
                            "environment": &environment,
                            "sample": &sample,
                            "iteration": iteration,
                            "distro": &args.distro,
                            "converted": false,
                            "error": err.to_string(),
                        });
                        if args.jsonl {
                            println!("{}", serde_json::to_string(&evidence)?);
                        } else {
                            println!("{}", serde_json::to_string_pretty(&evidence)?);
                        }
                    }
                }
            }
        }

        Ok(())
    })
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

    #[test]
    fn conversion_crawl_candidate_parser_requires_exact_identity() {
        let digest = "a".repeat(64);
        assert_eq!(
            parse_candidate(&format!("fedora-44={digest}")).unwrap(),
            ProfileRevisionSelection {
                source_profile: "fedora-44".to_string(),
                profile_revision_sha256: digest,
            }
        );
        assert!(parse_candidate("fedora-44").is_err());
        assert!(parse_candidate(&format!("fedora-44={}", "A".repeat(64))).is_err());
        assert!(
            parse_candidate("=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .is_err()
        );
    }
}
