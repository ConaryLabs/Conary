// conary-test/src/cli.rs

use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Parser, Subcommand};
use conary_test::engine::container_setup::initialize_container_state;
use conary_test::paths;
use handlers::{
    cmd_deploy_rebuild, cmd_deploy_restart, cmd_deploy_source, cmd_deploy_status,
    cmd_fixtures_build, cmd_fixtures_publish, cmd_health, cmd_images_info, cmd_images_prune,
    cmd_logs, cmd_manifests_reload,
};
use std::path::{Path, PathBuf};

mod handlers;

// ---------------------------------------------------------------------------
// ANSI color helpers
// ---------------------------------------------------------------------------

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

/// Return true if stdout is a TTY and `NO_COLOR` is not set.
fn use_color() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout()) && std::env::var_os("NO_COLOR").is_none()
}

/// Wrap text in an ANSI color code if color is enabled.
fn color(text: &str, code: &str) -> String {
    if use_color() {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "conary-test", version, about = "Conary test infrastructure")]
struct Cli {
    /// Output raw JSON instead of formatted text
    #[arg(long, global = true)]
    json: bool,

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
        /// Bearer token for authentication. If not set, reads CONARY_TEST_TOKEN env var.
        /// If neither is set, the server runs without auth.
        #[arg(long)]
        token: Option<String>,
        /// Maximum number of test runs that execute concurrently. Additional
        /// runs queue until a slot becomes available.
        #[arg(long, default_value = "2")]
        max_concurrent: usize,
    },

    /// List available test suites
    List,

    /// Manage container images
    Images {
        #[command(subcommand)]
        command: ImageCommands,
    },

    /// Deploy source, rebuild binaries, restart service
    Deploy {
        #[command(subcommand)]
        command: DeployCommands,
    },

    /// Build and publish test fixtures
    Fixtures {
        #[command(subcommand)]
        command: FixtureCommands,
    },

    /// Show test logs for a specific test
    Logs {
        /// Test identifier (e.g. "T01")
        test_id: String,

        /// Run ID to fetch logs from
        #[arg(long)]
        run: Option<u64>,

        /// Filter to a specific step index
        #[arg(long)]
        step: Option<u32>,

        /// Filter to stdout or stderr
        #[arg(long)]
        stream: Option<String>,
    },

    /// Check service health and deployment status
    Health {
        /// Local conary-test service port
        #[arg(long, env = "CONARY_TEST_PORT", default_value = "9090")]
        port: u16,
    },

    /// Reload test manifests from disk
    Manifests {
        #[command(subcommand)]
        command: ManifestCommands,
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

    /// Remove old images, keeping the N most recent per distro
    Prune {
        /// Number of images to keep per distro
        #[arg(long, default_value = "3")]
        keep: usize,
    },

    /// Show details about a container image
    Info {
        /// Image name or tag to inspect
        image: String,
    },
}

#[derive(Subcommand)]
enum DeployCommands {
    /// Pull source from git (optionally checkout a specific ref)
    Source {
        /// Git ref to checkout (branch, tag, or commit). Default: pull current branch.
        #[arg(long = "ref")]
        git_ref: Option<String>,
    },

    /// Rebuild binaries from current source
    Rebuild {
        /// Specific crate to build (conary, conary-test). Default: both.
        #[arg(long = "crate")]
        crate_name: Option<String>,
    },

    /// Restart the conary-test systemd user service
    Restart,

    /// Show deployment status (version, uptime, service state)
    Status {
        /// Local conary-test service port
        #[arg(long, env = "CONARY_TEST_PORT", default_value = "9090")]
        port: u16,
    },

    /// Perform a managed Forge rollout from a Git ref or explicit local snapshot
    #[command(
        group(ArgGroup::new("rollout_target").args(["unit", "group"]).required(true).multiple(false)),
        group(ArgGroup::new("rollout_source").args(["git_ref", "path"]).required(true).multiple(false))
    )]
    Rollout {
        /// Deploy a single manifest-defined rollout unit
        #[arg(long, group = "rollout_target")]
        unit: Option<String>,

        /// Deploy a manifest-defined rollout group
        #[arg(long, group = "rollout_target")]
        group: Option<String>,

        /// Resolve and deploy an exact Git ref on Forge
        #[arg(long = "ref", group = "rollout_source")]
        git_ref: Option<String>,

        /// Deploy an explicit local-snapshot path already synced to Forge
        #[arg(long, group = "rollout_source")]
        path: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum FixtureCommands {
    /// Build test fixture CCS packages
    Build {
        /// Fixture groups: all, corrupted, malicious, deps, boot, large
        #[arg(long, default_value = "all")]
        groups: String,
    },

    /// Publish test fixtures to Remi repository
    Publish,
}

#[derive(Subcommand)]
enum ManifestCommands {
    /// Reload manifests from disk and display updated list
    Reload,
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Load global config from `$CONARY_TEST_CONFIG` or default path.
fn load_config() -> Result<conary_test::config::distro::GlobalConfig> {
    let path = std::env::var_os("CONARY_TEST_CONFIG")
        .map(PathBuf::from)
        .unwrap_or(paths::default_config_path()?);
    conary_test::config::load_global_config(&path)
}

/// Return manifest directory from `$CONARY_TEST_MANIFESTS` or default.
fn manifest_dir() -> Result<PathBuf> {
    Ok(std::env::var_os("CONARY_TEST_MANIFESTS")
        .map(PathBuf::from)
        .unwrap_or(paths::default_manifest_dir()?))
}

/// Discover manifests matching a requested phase.
fn manifests_for_phase(phase: u32) -> Result<Vec<PathBuf>> {
    let dir_path = manifest_dir()?;
    if !dir_path.is_dir() {
        bail!("manifest directory not found: {}", dir_path.display());
    }

    let mut manifests = Vec::new();
    for entry in std::fs::read_dir(&dir_path)? {
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

    let path = paths::default_container_dir()?.join(filename);
    if !path.exists() {
        bail!("containerfile not found: {}", path.display());
    }
    Ok(path)
}

fn host_results_dir() -> Result<PathBuf> {
    let path = std::env::var_os("CONARY_TEST_RESULTS_DIR")
        .map(PathBuf::from)
        .unwrap_or(paths::default_results_dir()?);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path))
    }
}

/// Determine the project root directory.
///
/// Checks `CONARY_PROJECT_DIR` env var first, then walks up from the current
/// executable until a directory containing `Cargo.toml` is found.
fn project_dir() -> Result<String> {
    Ok(paths::project_dir()?.to_string_lossy().to_string())
}

/// Run a shell command and return (exit_code, stdout, stderr).
async fn run_command(cmd: &str, args: &[&str], cwd: Option<&str>) -> Result<(i32, String, String)> {
    let mut command = tokio::process::Command::new(cmd);
    command.args(args);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let output = command
        .output()
        .await
        .with_context(|| format!("failed to run {cmd}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    Ok((code, stdout, stderr))
}

/// Print command output as JSON or human-readable text.
fn print_step(label: &str, code: i32, stdout: &str, stderr: &str, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "step": label,
                "exit_code": code,
                "stdout": stdout.trim(),
                "stderr": stderr.trim(),
            })
        );
    } else {
        print_command_result(label, code, stdout, stderr);
    }
}

/// Print command output in a human-friendly format.
fn print_command_result(label: &str, code: i32, stdout: &str, stderr: &str) {
    let status = if code == 0 {
        color("OK", GREEN)
    } else {
        color("FAILED", RED)
    };
    println!("[{label}] exit={code} ({status})");
    if !stdout.is_empty() {
        let lines: Vec<&str> = stdout.lines().collect();
        let start = lines.len().saturating_sub(100);
        println!("--- stdout (last {} lines) ---", lines.len() - start);
        for line in &lines[start..] {
            println!("{line}");
        }
    }
    if !stderr.is_empty() {
        let lines: Vec<&str> = stderr.lines().collect();
        let start = lines.len().saturating_sub(50);
        println!("--- stderr (last {} lines) ---", lines.len() - start);
        for line in &lines[start..] {
            println!("{line}");
        }
    }
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
        let host_results_dir = host_results_dir()?;
        std::fs::create_dir_all(&host_results_dir).ok();

        let manifest_paths = match suite_path {
            Some(p) => {
                let path = PathBuf::from(p);
                // If the path doesn't exist, try resolving relative to the manifest directory
                let resolved = if path.exists() {
                    path
                } else {
                    let dir = manifest_dir()?;
                    let with_ext = dir.join(format!("{p}.toml"));
                    if with_ext.exists() {
                        with_ext
                    } else {
                        // Fall through with original path — load_manifest will produce a clear error
                        path
                    }
                };
                vec![resolved]
            }
            None => manifests_for_phase(phase)?,
        };

        // Check if all manifests contain only QEMU boot steps — if so,
        // skip container setup entirely (QEMU tests boot their own VMs).
        let all_qemu_only = manifest_paths.iter().all(|p| {
            conary_test::config::load_manifest(p)
                .map(|m| m.is_qemu_only())
                .unwrap_or(false)
        });

        if all_qemu_only {
            return run_qemu_only_suite(config, distro, phase, &manifest_paths, &host_results_dir)
                .await;
        }

        let backend = conary_test::container::BollardBackend::new()?;

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
        let container_id = backend.create(container_config.clone()).await?;
        tracing::info!(distro, id = %container_id, "Container created");

        use conary_test::container::ContainerBackend;
        backend.start(&container_id).await?;
        tracing::info!(distro, id = %container_id, "Container started");

        let mut aggregate_suite =
            conary_test::engine::suite::TestSuite::new(&format!("phase-{phase}"), phase);
        aggregate_suite.status = conary_test::engine::suite::RunStatus::Running;

        for manifest_path in &manifest_paths {
            let manifest = conary_test::config::load_manifest(manifest_path)
                .with_context(|| format!("failed to load manifest: {}", manifest_path.display()))?;
            initialize_container_state(
                config,
                distro,
                manifest.suite.phase > 1,
                &backend,
                &container_id,
            )
            .await?;

            let mut runner =
                conary_test::engine::runner::TestRunner::new(config.clone(), distro.to_string());
            let suite = runner
                .run(&manifest, &backend, &container_id, Some(&container_config))
                .await?;
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
        let keep_container = std::env::var("CONARY_TEST_KEEP_CONTAINER")
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
        if keep_container {
            tracing::warn!(
                distro,
                id = %container_id,
                "Keeping test container for forensic inspection"
            );
            eprintln!("CONARY_TEST_KEPT_CONTAINER={container_id}");
        } else {
            if let Err(e) = backend.stop(&container_id).await {
                tracing::warn!(error = %e, "Failed to stop container");
            }
            if let Err(e) = backend.remove(&container_id).await {
                tracing::warn!(error = %e, "Failed to remove container");
            }
        }

        Ok(!has_failures)
    })
}

/// Run a QEMU-only test suite without any container runtime.
///
/// QEMU tests boot their own VMs and execute commands over SSH.
/// The container backend, image build, and container lifecycle are
/// entirely skipped.
async fn run_qemu_only_suite(
    config: &conary_test::config::distro::GlobalConfig,
    distro: &str,
    phase: u32,
    manifest_paths: &[PathBuf],
    host_results_dir: &Path,
) -> Result<bool> {
    tracing::info!("QEMU-only suite detected, skipping container setup");

    // Create a dummy backend and container for the runner API.
    // QEMU steps ignore these — they boot their own VMs.
    let dummy_backend = conary_test::container::NullBackend;
    let dummy_container_id: conary_test::container::ContainerId = "qemu-standalone".to_string();
    let dummy_config = conary_test::container::ContainerConfig::default();

    let mut aggregate_suite =
        conary_test::engine::suite::TestSuite::new(&format!("phase-{phase}"), phase);
    aggregate_suite.status = conary_test::engine::suite::RunStatus::Running;

    for manifest_path in manifest_paths {
        let manifest = conary_test::config::load_manifest(manifest_path)
            .with_context(|| format!("failed to load manifest: {}", manifest_path.display()))?;

        let mut runner =
            conary_test::engine::runner::TestRunner::new(config.clone(), distro.to_string());
        let suite = runner
            .run(
                &manifest,
                &dummy_backend,
                &dummy_container_id,
                Some(&dummy_config),
            )
            .await?;
        for result in suite.results {
            aggregate_suite.record(result);
        }
    }
    aggregate_suite.finish();

    let json = conary_test::report::json::to_json_report(&aggregate_suite)?;
    println!("{json}");

    let results_file = host_results_dir.join(format!("{distro}-phase{phase}.json"));
    conary_test::report::json::write_json_report(&aggregate_suite, &results_file)?;
    tracing::info!(path = %results_file.display(), "Results written");

    Ok(aggregate_suite.failed() == 0)
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let json = cli.json;

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

        Commands::Serve {
            port,
            token,
            max_concurrent,
        } => {
            let token = token.or_else(|| std::env::var("CONARY_TEST_TOKEN").ok());
            if token.is_some() {
                tracing::info!("Bearer token authentication enabled");
            } else {
                tracing::warn!("No authentication token configured -- server is open");
            }
            let config = load_config()?;
            let state = conary_test::server::AppState::with_max_concurrent(
                config,
                manifest_dir()?.display().to_string(),
                max_concurrent,
            );
            tracing::info!(%port, max_concurrent, "Starting server");
            tokio::runtime::Runtime::new()?
                .block_on(conary_test::server::run_server(state, port, token))
        }

        Commands::List => {
            let dir = manifest_dir()?;
            let dir_path = dir.as_path();

            if !dir_path.is_dir() {
                tracing::warn!(path = %dir.display(), "Manifest directory not found");
                return Ok(());
            }

            let mut entries: Vec<_> = std::fs::read_dir(dir_path)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
                .collect();
            entries.sort_by_key(|e| e.file_name());

            if entries.is_empty() {
                println!("No test manifests found in {}", dir.display());
                return Ok(());
            }

            if json {
                let mut suites = Vec::new();
                for entry in entries {
                    let path = entry.path();
                    if let Ok(manifest) = conary_test::config::load_manifest(&path) {
                        suites.push(serde_json::json!({
                            "name": manifest.suite.name,
                            "phase": manifest.suite.phase,
                            "test_count": manifest.test.len(),
                        }));
                    }
                }
                println!("{}", serde_json::to_string_pretty(&suites)?);
            } else {
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
            }
            Ok(())
        }

        Commands::Images { command } => {
            let rt = tokio::runtime::Runtime::new()?;
            match command {
                ImageCommands::Build { distro } => {
                    let config = load_config()?;
                    rt.block_on(async {
                        let backend = conary_test::container::BollardBackend::new()?;
                        let cf_path = containerfile_path(&config, &distro)?;
                        tracing::info!(%distro, containerfile = %cf_path.display(), "Building image");
                        let tag =
                            conary_test::container::build_distro_image(&backend, &cf_path, &distro)
                                .await?;
                        if json {
                            println!(
                                "{}",
                                serde_json::json!({"distro": distro, "image": tag, "status": "built"})
                            );
                        } else {
                            tracing::info!(%distro, image = %tag, "Image built successfully");
                        }
                        Ok(())
                    })
                }
                ImageCommands::List => rt.block_on(async {
                    use conary_test::container::ContainerBackend;

                    let backend = conary_test::container::BollardBackend::new()?;
                    let images = backend.list_images().await?;

                    if images.is_empty() {
                        if json {
                            println!("[]");
                        } else {
                            println!("No images found");
                        }
                        return Ok(());
                    }

                    if json {
                        println!("{}", serde_json::to_string_pretty(&images)?);
                    } else {
                        println!("{:<20} {:<40} SIZE", "TAG", "ID");
                        println!("{}", "-".repeat(70));
                        for img in &images {
                            let tag = img.tags.first().map(String::as_str).unwrap_or("<none>");
                            let short_id = if img.id.len() > 12 {
                                &img.id[..12]
                            } else {
                                &img.id
                            };
                            let size_mb = img.size / (1024 * 1024);
                            println!("{tag:<20} {short_id:<40} {size_mb} MB");
                        }
                    }
                    Ok(())
                }),
                ImageCommands::Prune { keep } => rt.block_on(cmd_images_prune(keep, json)),
                ImageCommands::Info { image } => rt.block_on(cmd_images_info(&image, json)),
            }
        }

        Commands::Deploy { command } => {
            let rt = tokio::runtime::Runtime::new()?;
            match command {
                DeployCommands::Source { git_ref } => {
                    rt.block_on(cmd_deploy_source(git_ref.as_deref(), json))
                }
                DeployCommands::Rebuild { crate_name } => {
                    rt.block_on(cmd_deploy_rebuild(crate_name.as_deref(), json))
                }
                DeployCommands::Restart => rt.block_on(cmd_deploy_restart(json)),
                DeployCommands::Status { port } => rt.block_on(cmd_deploy_status(json, port)),
                DeployCommands::Rollout {
                    unit: _,
                    group: _,
                    git_ref: _,
                    path: _,
                } => bail!("deploy rollout not yet implemented"),
            }
        }

        Commands::Fixtures { command } => {
            let rt = tokio::runtime::Runtime::new()?;
            match command {
                FixtureCommands::Build { groups } => rt.block_on(cmd_fixtures_build(&groups, json)),
                FixtureCommands::Publish => rt.block_on(cmd_fixtures_publish(json)),
            }
        }

        Commands::Logs {
            test_id,
            run,
            step,
            stream,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(cmd_logs(&test_id, run, step, stream.as_deref(), json))
        }

        Commands::Health { port } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(cmd_health(json, port))
        }

        Commands::Manifests { command } => match command {
            ManifestCommands::Reload => cmd_manifests_reload(json),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    fn cwd_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn set_test_port_env(value: Option<&str>) {
        match value {
            Some(value) => unsafe { std::env::set_var("CONARY_TEST_PORT", value) },
            None => unsafe { std::env::remove_var("CONARY_TEST_PORT") },
        }
    }

    #[test]
    fn default_manifest_dir_exists_under_workspace_root() {
        let root = PathBuf::from(project_dir().expect("project dir"));
        let manifests = manifest_dir().expect("manifest dir");
        assert!(
            manifests.is_dir(),
            "expected default manifest dir to exist at {}",
            manifests.display()
        );
        assert!(
            manifests.starts_with(&root),
            "expected manifest dir {} to live under {}",
            manifests.display(),
            root.display()
        );
    }

    #[test]
    fn load_config_succeeds_from_workspace_root() {
        let _guard = cwd_lock().lock().expect("cwd lock");
        let original = std::env::current_dir().expect("current dir");
        let root = PathBuf::from(project_dir().expect("project dir"));
        assert!(
            std::env::var_os("CONARY_TEST_CONFIG").is_none(),
            "this test expects CONARY_TEST_CONFIG to be unset"
        );
        std::env::set_current_dir(&root).expect("set workspace root");
        let result = load_config();
        std::env::set_current_dir(original).expect("restore current dir");

        assert!(
            result.is_ok(),
            "expected load_config() to work from {}, got {result:?}",
            root.display()
        );
    }

    #[test]
    fn deploy_status_port_defaults_to_9090() {
        let _guard = env_lock().lock().expect("env lock");
        set_test_port_env(None);

        let cli = Cli::try_parse_from(["conary-test", "deploy", "status"]).unwrap();
        match cli.command {
            Commands::Deploy {
                command: DeployCommands::Status { port },
            } => assert_eq!(port, 9090),
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn deploy_status_port_uses_env_when_flag_is_absent() {
        let _guard = env_lock().lock().expect("env lock");
        set_test_port_env(Some("9191"));

        let cli = Cli::try_parse_from(["conary-test", "deploy", "status"]).unwrap();
        set_test_port_env(None);

        match cli.command {
            Commands::Deploy {
                command: DeployCommands::Status { port },
            } => assert_eq!(port, 9191),
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn explicit_port_flag_overrides_env_for_health() {
        let _guard = env_lock().lock().expect("env lock");
        set_test_port_env(Some("9191"));

        let cli = Cli::try_parse_from(["conary-test", "health", "--port", "8181"]).unwrap();
        set_test_port_env(None);

        match cli.command {
            Commands::Health { port } => assert_eq!(port, 8181),
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn deploy_rollout_parses_unit_with_ref() {
        let cli = Cli::try_parse_from([
            "conary-test",
            "deploy",
            "rollout",
            "--unit",
            "conary_test",
            "--ref",
            "main",
        ])
        .unwrap();

        match cli.command {
            Commands::Deploy {
                command:
                    DeployCommands::Rollout {
                        unit,
                        group,
                        git_ref,
                        path,
                    },
            } => {
                assert_eq!(unit.as_deref(), Some("conary_test"));
                assert_eq!(group, None);
                assert_eq!(git_ref.as_deref(), Some("main"));
                assert_eq!(path, None);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn deploy_rollout_parses_group_with_path() {
        let cli = Cli::try_parse_from([
            "conary-test",
            "deploy",
            "rollout",
            "--group",
            "control_plane",
            "--path",
            "~/Conary",
        ])
        .unwrap();

        match cli.command {
            Commands::Deploy {
                command:
                    DeployCommands::Rollout {
                        unit,
                        group,
                        git_ref,
                        path,
                    },
            } => {
                assert_eq!(unit, None);
                assert_eq!(group.as_deref(), Some("control_plane"));
                assert_eq!(git_ref, None);
                assert_eq!(path.as_deref(), Some(std::path::Path::new("~/Conary")));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn deploy_rollout_rejects_unit_and_group_together() {
        let error = Cli::try_parse_from([
            "conary-test",
            "deploy",
            "rollout",
            "--unit",
            "conary_test",
            "--group",
            "control_plane",
            "--ref",
            "main",
        ])
        .err()
        .expect("mixed target rejected");

        let rendered = error.to_string();
        assert!(rendered.contains("--unit"));
        assert!(rendered.contains("--group"));
    }

    #[test]
    fn deploy_rollout_rejects_ref_and_path_together() {
        let error = Cli::try_parse_from([
            "conary-test",
            "deploy",
            "rollout",
            "--unit",
            "conary_test",
            "--ref",
            "main",
            "--path",
            "~/Conary",
        ])
        .err()
        .expect("mixed source rejected");

        let rendered = error.to_string();
        assert!(rendered.contains("--ref"));
        assert!(rendered.contains("--path"));
    }

    #[test]
    fn deploy_rollout_requires_target_and_source() {
        let target_error =
            Cli::try_parse_from(["conary-test", "deploy", "rollout", "--ref", "main"])
                .err()
                .expect("missing target rejected");
        assert!(
            target_error.to_string().contains("--unit")
                || target_error.to_string().contains("--group")
        );

        let source_error =
            Cli::try_parse_from(["conary-test", "deploy", "rollout", "--unit", "conary_test"])
                .err()
                .expect("missing source rejected");
        assert!(
            source_error.to_string().contains("--ref")
                || source_error.to_string().contains("--path")
        );
    }
}
