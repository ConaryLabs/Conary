// conary-test/src/cli.rs

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "conary-test", version, about = "Conary test infrastructure")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a test suite
    Run {
        /// Distro to test against
        #[arg(long, required_unless_present = "all_distros")]
        distro: Option<String>,

        /// Test phase (1, 2, or 3)
        #[arg(long, default_value = "1")]
        phase: u32,

        /// Path to test suite TOML
        #[arg(long)]
        suite: Option<String>,

        /// Run all distros
        #[arg(long)]
        all_distros: bool,
    },

    /// Start the HTTP + MCP server
    Serve {
        /// Port to listen on
        #[arg(long, default_value = "9090")]
        port: u16,
    },

    /// List available test suites
    List,

    /// Manage container images
    Images {
        #[command(subcommand)]
        command: ImageCommands,
    },
}

#[derive(Subcommand)]
enum ImageCommands {
    /// Build a distro image
    Build {
        /// Distro to build
        #[arg(long)]
        distro: String,
    },

    /// List built images
    List,
}

/// Load global config from `$CONARY_TEST_CONFIG` or default path.
fn load_config() -> Result<conary_test::config::distro::GlobalConfig> {
    let path = std::env::var("CONARY_TEST_CONFIG")
        .unwrap_or_else(|_| "tests/integration/remi/config.toml".into());
    conary_test::config::load_global_config(Path::new(&path))
}

/// Return manifest directory from `$CONARY_TEST_MANIFESTS` or default.
fn manifest_dir() -> String {
    std::env::var("CONARY_TEST_MANIFESTS")
        .unwrap_or_else(|_| "tests/integration/remi/manifests".into())
}

/// Discover manifests matching a requested phase.
fn manifests_for_phase(phase: u32) -> Result<Vec<PathBuf>> {
    let dir = manifest_dir();
    let dir_path = Path::new(&dir);
    if !dir_path.is_dir() {
        bail!("manifest directory not found: {}", dir_path.display());
    }

    let mut manifests = Vec::new();
    for entry in std::fs::read_dir(dir_path)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }

        let manifest = conary_test::config::load_manifest(&path)
            .with_context(|| format!("failed to parse manifest: {}", path.display()))?;
        if manifest.suite.phase == phase {
            manifests.push(path);
        }
    }

    manifests.sort();
    if manifests.is_empty() {
        bail!(
            "no manifests found for phase {phase} in {}",
            dir_path.display()
        );
    }

    Ok(manifests)
}

/// Resolve the containerfile path for a distro.
fn containerfile_path(
    config: &conary_test::config::distro::GlobalConfig,
    distro: &str,
) -> Result<PathBuf> {
    let dc = config
        .distros
        .get(distro)
        .with_context(|| format!("unknown distro: {distro}"))?;

    let default_name = format!("Containerfile.{distro}");
    let filename = dc.containerfile.as_deref().unwrap_or(&default_name);

    let path = PathBuf::from("tests/integration/remi/containers").join(filename);
    if !path.exists() {
        bail!("containerfile not found: {}", path.display());
    }
    Ok(path)
}

fn host_results_dir() -> PathBuf {
    std::env::var("CONARY_TEST_RESULTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("tests/integration/remi/results"))
}

async fn initialize_container_state(
    config: &conary_test::config::distro::GlobalConfig,
    backend: &conary_test::container::BollardBackend,
    container_id: &conary_test::container::ContainerId,
) -> Result<()> {
    use conary_test::container::ContainerBackend;

    let db_parent = Path::new(&config.paths.db)
        .parent()
        .context("db path has no parent directory")?
        .display()
        .to_string();
    let init_cmd = format!(
        "mkdir -p {db_parent} && {} system init --db-path {}",
        config.paths.conary_bin, config.paths.db
    );
    let init_result = backend
        .exec(container_id, &["sh", "-c", &init_cmd], Duration::from_secs(120))
        .await?;
    if init_result.exit_code != 0 {
        bail!(
            "failed to initialize conary database: {}{}",
            init_result.stdout,
            init_result.stderr
        );
    }

    for repo in &config.setup.remove_default_repos {
        let remove_cmd = format!(
            "{} repo remove {} --db-path {} >/dev/null 2>&1 || true",
            config.paths.conary_bin, repo, config.paths.db
        );
        backend
            .exec(
                container_id,
                &["sh", "-c", &remove_cmd],
                Duration::from_secs(30),
            )
            .await?;
    }

    Ok(())
}

/// Run tests for a single distro.
fn run_single_distro(
    config: &conary_test::config::distro::GlobalConfig,
    distro: &str,
    phase: u32,
    suite_path: Option<&str>,
) -> Result<bool> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let backend = conary_test::container::BollardBackend::new()?;
        let host_results_dir = host_results_dir();
        std::fs::create_dir_all(&host_results_dir).ok();

        // Resolve and build the image.
        let cf_path = containerfile_path(config, distro)?;
        tracing::info!(distro, containerfile = %cf_path.display(), "Building image");
        let image_tag =
            conary_test::container::build_distro_image(&backend, &cf_path, distro).await?;
        tracing::info!(distro, image = %image_tag, "Image built");

        // Create and start the container.
        let container_config = conary_test::container::ContainerConfig {
            image: image_tag,
            privileged: true,
            volumes: vec![conary_test::container::VolumeMount {
                host_path: host_results_dir.display().to_string(),
                container_path: config.paths.results_dir.clone(),
                read_only: false,
            }],
            ..Default::default()
        };
        let container_id = backend.create(container_config).await?;
        tracing::info!(distro, id = %container_id, "Container created");

        use conary_test::container::ContainerBackend;
        backend.start(&container_id).await?;
        tracing::info!(distro, id = %container_id, "Container started");
        initialize_container_state(config, &backend, &container_id).await?;

        let manifest_paths = match suite_path {
            Some(p) => vec![PathBuf::from(p)],
            None => manifests_for_phase(phase)?,
        };

        let mut aggregate_suite =
            conary_test::engine::suite::TestSuite::new(&format!("phase-{phase}"), phase);
        aggregate_suite.status = conary_test::engine::suite::RunStatus::Running;

        for manifest_path in &manifest_paths {
            let manifest = conary_test::config::load_manifest(manifest_path)
                .with_context(|| format!("failed to load manifest: {}", manifest_path.display()))?;

            let mut runner =
                conary_test::engine::runner::TestRunner::new(config.clone(), distro.to_string());
            let suite = runner.run(&manifest, &backend, &container_id).await?;
            for result in suite.results {
                aggregate_suite.record(result);
            }
        }
        aggregate_suite.finish();

        // Print JSON results.
        let json = conary_test::report::json::to_json_report(&aggregate_suite)?;
        println!("{json}");

        // Write results to file.
        let results_file = host_results_dir.join(format!("{distro}-phase{phase}.json"));
        conary_test::report::json::write_json_report(&aggregate_suite, &results_file)?;
        tracing::info!(path = %results_file.display(), "Results written");

        let has_failures = aggregate_suite.failed() > 0;

        // Cleanup container.
        if let Err(e) = backend.stop(&container_id).await {
            tracing::warn!(error = %e, "Failed to stop container");
        }
        if let Err(e) = backend.remove(&container_id).await {
            tracing::warn!(error = %e, "Failed to remove container");
        }

        Ok(!has_failures)
    })
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            distro,
            phase,
            suite,
            all_distros,
        } => {
            let config = load_config()?;

            let distros: Vec<String> = if all_distros {
                config.distros.keys().cloned().collect()
            } else {
                vec![distro.context("--distro is required when --all-distros is not set")?]
            };

            let mut all_passed = true;
            for d in &distros {
                tracing::info!(distro = %d, phase, "Starting test run");
                let passed = run_single_distro(&config, d, phase, suite.as_deref())?;
                if !passed {
                    all_passed = false;
                }
            }

            if !all_passed {
                std::process::exit(1);
            }
            Ok(())
        }

        Commands::Serve { port } => {
            let config = load_config()?;
            let state = conary_test::server::AppState::new(config, manifest_dir());
            tracing::info!(%port, "Starting server");
            tokio::runtime::Runtime::new()?.block_on(conary_test::server::run_server(state, port))
        }

        Commands::List => {
            let dir = manifest_dir();
            let dir_path = Path::new(&dir);

            if !dir_path.is_dir() {
                tracing::warn!(path = %dir, "Manifest directory not found");
                return Ok(());
            }

            let mut entries: Vec<_> = std::fs::read_dir(dir_path)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
                .collect();
            entries.sort_by_key(|e| e.file_name());

            if entries.is_empty() {
                println!("No test manifests found in {dir}");
                return Ok(());
            }

            println!("{:<30} {:<8} TESTS", "NAME", "PHASE");
            println!("{}", "-".repeat(50));
            for entry in entries {
                let path = entry.path();
                match conary_test::config::load_manifest(&path) {
                    Ok(manifest) => {
                        println!(
                            "{:<30} {:<8} {}",
                            manifest.suite.name,
                            manifest.suite.phase,
                            manifest.test.len()
                        );
                    }
                    Err(e) => {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        tracing::warn!(file = %name, error = %e, "Failed to parse manifest");
                    }
                }
            }
            Ok(())
        }

        Commands::Images { command } => match command {
            ImageCommands::Build { distro } => {
                let config = load_config()?;
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(async {
                    let backend = conary_test::container::BollardBackend::new()?;
                    let cf_path = containerfile_path(&config, &distro)?;
                    tracing::info!(%distro, containerfile = %cf_path.display(), "Building image");
                    let tag =
                        conary_test::container::build_distro_image(&backend, &cf_path, &distro)
                            .await?;
                    tracing::info!(%distro, image = %tag, "Image built successfully");
                    Ok(())
                })
            }
            ImageCommands::List => {
                println!("Image listing not yet implemented");
                Ok(())
            }
        },
    }
}
