# Test Infrastructure Crate Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build `conary-test`, a Rust crate that replaces the Python test runner with a declarative TOML test engine, container management via bollard, HTTP API, and MCP interface.

**Architecture:** New workspace crate `conary-test/` with modules for config parsing, test engine, container lifecycle (bollard), HTTP server (axum), MCP server (rmcp), and CLI (clap). Follows the same patterns as `conary-server` -- lib.rs re-exports, Arc<RwLock<State>>, axum routers, rmcp tool_router macro.

**Tech Stack:** Rust 1.94, bollard (Podman/Docker API), axum + tokio (HTTP), rmcp (MCP), serde + toml (config), clap (CLI), tracing (logging).

**Design doc:** `docs/plans/2026-03-08-test-infrastructure-design.md`

---

### Task 1: Scaffold the crate and verify it compiles

Create the `conary-test/` crate skeleton with Cargo.toml, lib.rs, and a placeholder binary. Wire it into the workspace.

**Files:**
- Create: `conary-test/Cargo.toml`
- Create: `conary-test/src/lib.rs`
- Create: `conary-test/src/cli.rs`
- Modify: `Cargo.toml` (root, workspace members)

**Step 1: Create `conary-test/Cargo.toml`**

```toml
[package]
name = "conary-test"
version = "0.1.0"
edition = "2024"
rust-version = "1.94"
description = "Test infrastructure for Conary — declarative TOML test engine with container management, HTTP API, and MCP interface"

[dependencies]
# Workspace shared
anyhow.workspace = true
thiserror.workspace = true
serde.workspace = true
serde_json.workspace = true
toml.workspace = true
chrono.workspace = true
tokio.workspace = true
async-trait.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
clap.workspace = true
uuid.workspace = true
sha2.workspace = true
hex.workspace = true
futures.workspace = true

# Container management
bollard = "0.18"

# HTTP server
axum = "0.7"
tower-http = { version = "0.6", features = ["cors"] }
tokio-stream = "0.1"

# MCP
rmcp = { version = "1.1", features = ["server", "macros", "transport-streamable-http-server"] }
schemars = "1.0"

[[bin]]
name = "conary-test"
path = "src/cli.rs"
```

**Step 2: Create `conary-test/src/lib.rs`**

```rust
// conary-test/src/lib.rs

pub mod config;
pub mod engine;
pub mod container;
pub mod report;
pub mod server;
```

Each module will be a directory with `mod.rs`. For now, create placeholder files so it compiles:

- `conary-test/src/config/mod.rs` — `// conary-test/src/config/mod.rs`
- `conary-test/src/engine/mod.rs` — `// conary-test/src/engine/mod.rs`
- `conary-test/src/container/mod.rs` — `// conary-test/src/container/mod.rs`
- `conary-test/src/report/mod.rs` — `// conary-test/src/report/mod.rs`
- `conary-test/src/server/mod.rs` — `// conary-test/src/server/mod.rs`

**Step 3: Create `conary-test/src/cli.rs`**

```rust
// conary-test/src/cli.rs

use anyhow::Result;
use clap::{Parser, Subcommand};

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
        #[arg(long)]
        distro: String,

        /// Test phase (1 or 2)
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

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { distro, phase, suite, all_distros } => {
            tracing::info!(%distro, %phase, "Starting test run");
            // TODO: Task 4
            Ok(())
        }
        Commands::Serve { port } => {
            tracing::info!(%port, "Starting server");
            // TODO: Task 8
            Ok(())
        }
        Commands::List => {
            // TODO: Task 4
            Ok(())
        }
        Commands::Images { command } => {
            match command {
                ImageCommands::Build { distro } => {
                    tracing::info!(%distro, "Building image");
                    // TODO: Task 5
                }
                ImageCommands::List => {
                    // TODO: Task 5
                }
            }
            Ok(())
        }
    }
}
```

**Step 4: Add to workspace**

In root `Cargo.toml`, change:
```toml
members = [".", "conary-core", "conary-erofs", "conary-server"]
```
to:
```toml
members = [".", "conary-core", "conary-erofs", "conary-server", "conary-test"]
```

**Step 5: Verify it compiles**

Run: `cargo build -p conary-test`
Expected: Compiles with no errors. Warnings about unused imports are OK at this stage.

**Step 6: Verify clippy passes**

Run: `cargo clippy -p conary-test -- -D warnings`
Expected: PASS (fix any warnings before committing)

**Step 7: Commit**

```bash
git add conary-test/ Cargo.toml Cargo.lock
git commit -m "feat(test): scaffold conary-test crate with CLI skeleton"
```

---

### Task 2: Config module — parse test manifests and distro config

Implement TOML config loading that reads both the global config (distro definitions, paths, endpoints) and individual test suite manifests.

**Files:**
- Create: `conary-test/src/config/manifest.rs`
- Create: `conary-test/src/config/distro.rs`
- Modify: `conary-test/src/config/mod.rs`

**Step 1: Write tests for config parsing**

Add to `conary-test/src/config/mod.rs`:

```rust
// conary-test/src/config/mod.rs

pub mod distro;
pub mod manifest;

pub use distro::{DistroConfig, GlobalConfig};
pub use manifest::{Assertion, StepType, TestDef, TestManifest};

use anyhow::Result;
use std::path::Path;

/// Load a test manifest from a TOML file.
pub fn load_manifest(path: &Path) -> Result<TestManifest> {
    let content = std::fs::read_to_string(path)?;
    let manifest: TestManifest = toml::from_str(&content)?;
    Ok(manifest)
}

/// Load global config from a TOML file.
pub fn load_global_config(path: &Path) -> Result<GlobalConfig> {
    let content = std::fs::read_to_string(path)?;
    let config: GlobalConfig = toml::from_str(&content)?;
    config.apply_env_overrides()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_manifest() {
        let toml = r#"
[suite]
name = "Phase 1: Core"
phase = 1

[[test]]
id = "T01"
name = "health_check"
description = "Verify endpoint is reachable"
timeout = 30

[[test.step]]
run = "curl -sf http://localhost/health"

[test.step.assert]
exit_code = 0
stdout_contains = "ok"
"#;
        let manifest: TestManifest = toml::from_str(toml).unwrap();
        assert_eq!(manifest.suite.name, "Phase 1: Core");
        assert_eq!(manifest.suite.phase, 1);
        assert_eq!(manifest.test.len(), 1);
        assert_eq!(manifest.test[0].id, "T01");
        assert_eq!(manifest.test[0].step.len(), 1);
        assert_eq!(manifest.test[0].step[0].assert.as_ref().unwrap().exit_code, Some(0));
    }

    #[test]
    fn test_parse_multi_step_test() {
        let toml = r#"
[suite]
name = "Multi Step"
phase = 1

[[test]]
id = "T02"
name = "system_init"
description = "Initialize and verify"
timeout = 60
fatal = true

[[test.step]]
run = "conary system init"

[test.step.assert]
exit_code = 0

[[test.step]]
run = "conary repo list"

[test.step.assert]
stdout_contains = "default"
"#;
        let manifest: TestManifest = toml::from_str(toml).unwrap();
        assert_eq!(manifest.test[0].step.len(), 2);
        assert!(manifest.test[0].fatal.unwrap_or(false));
    }

    #[test]
    fn test_parse_depends_on() {
        let toml = r#"
[suite]
name = "Deps"
phase = 1

[[test]]
id = "T01"
name = "first"
description = "First test"
timeout = 10

[[test.step]]
run = "echo hi"

[[test]]
id = "T02"
name = "second"
description = "Depends on first"
timeout = 10
depends_on = ["T01"]

[[test.step]]
run = "echo bye"
"#;
        let manifest: TestManifest = toml::from_str(toml).unwrap();
        assert_eq!(manifest.test[1].depends_on.as_ref().unwrap(), &["T01"]);
    }

    #[test]
    fn test_parse_global_config() {
        let toml = r#"
[remi]
endpoint = "https://packages.conary.io"

[paths]
db = "/tmp/conary-test.db"
conary_bin = "/usr/bin/conary"
results_dir = "/tmp/results"
fixture_dir = "/opt/fixtures"

[distros.fedora43]
remi_distro = "fedora-43"
repo_name = "fedora-43"
containerfile = "containers/Containerfile.fedora43"
test_package_1 = "which"
test_binary_1 = "/usr/bin/which"
test_package_2 = "tree"
test_binary_2 = "/usr/bin/tree"
test_package_3 = "jq"
test_binary_3 = "/usr/bin/jq"
"#;
        let config: GlobalConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.remi.endpoint, "https://packages.conary.io");
        assert!(config.distros.contains_key("fedora43"));
        let fedora = &config.distros["fedora43"];
        assert_eq!(fedora.remi_distro, "fedora-43");
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p conary-test`
Expected: FAIL — `TestManifest`, `GlobalConfig` types don't exist yet.

**Step 3: Implement `manifest.rs`**

```rust
// conary-test/src/config/manifest.rs

use serde::Deserialize;

/// Top-level test manifest (one TOML file = one suite).
#[derive(Debug, Clone, Deserialize)]
pub struct TestManifest {
    pub suite: SuiteDef,
    pub test: Vec<TestDef>,
}

/// Suite metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct SuiteDef {
    pub name: String,
    pub phase: u32,
}

/// Single test definition.
#[derive(Debug, Clone, Deserialize)]
pub struct TestDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub timeout: u64,
    #[serde(default)]
    pub step: Vec<TestStep>,
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
    #[serde(default)]
    pub fatal: Option<bool>,
    #[serde(default)]
    pub group: Option<String>,
}

/// A single step within a test.
#[derive(Debug, Clone, Deserialize)]
pub struct TestStep {
    /// Shell command to run.
    #[serde(default)]
    pub run: Option<String>,

    /// Conary shorthand (auto-adds --db-path).
    #[serde(default)]
    pub conary: Option<String>,

    /// File existence check.
    #[serde(default)]
    pub file_exists: Option<String>,

    /// File absence check.
    #[serde(default)]
    pub file_not_exists: Option<String>,

    /// File checksum verification.
    #[serde(default)]
    pub file_checksum: Option<FileChecksum>,

    /// Sleep duration in seconds.
    #[serde(default)]
    pub sleep: Option<u64>,

    /// Assertions on step outcome.
    #[serde(default)]
    pub assert: Option<Assertion>,
}

/// Step type — derived from which field is set on TestStep.
#[derive(Debug, Clone)]
pub enum StepType {
    Run(String),
    Conary(String),
    FileExists(String),
    FileNotExists(String),
    FileChecksum(FileChecksum),
    Sleep(u64),
}

impl TestStep {
    /// Determine the step type from the populated field.
    pub fn step_type(&self) -> Option<StepType> {
        if let Some(cmd) = &self.run {
            Some(StepType::Run(cmd.clone()))
        } else if let Some(cmd) = &self.conary {
            Some(StepType::Conary(cmd.clone()))
        } else if let Some(path) = &self.file_exists {
            Some(StepType::FileExists(path.clone()))
        } else if let Some(path) = &self.file_not_exists {
            Some(StepType::FileNotExists(path.clone()))
        } else if let Some(chk) = &self.file_checksum {
            Some(StepType::FileChecksum(chk.clone()))
        } else if let Some(secs) = self.sleep {
            Some(StepType::Sleep(secs))
        } else {
            None
        }
    }
}

/// SHA-256 file checksum.
#[derive(Debug, Clone, Deserialize)]
pub struct FileChecksum {
    pub path: String,
    pub sha256: String,
}

/// Assertions that can be applied to a step result.
#[derive(Debug, Clone, Deserialize)]
pub struct Assertion {
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub stdout_contains: Option<String>,
    #[serde(default)]
    pub stdout_not_contains: Option<String>,
    #[serde(default)]
    pub stderr_contains: Option<String>,
    #[serde(default)]
    pub file_exists: Option<String>,
    #[serde(default)]
    pub file_not_exists: Option<String>,
    #[serde(default)]
    pub file_checksum: Option<FileChecksum>,
}
```

**Step 4: Implement `distro.rs`**

```rust
// conary-test/src/config/distro.rs

use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

/// Global config (loaded from config.toml).
#[derive(Debug, Clone, Deserialize)]
pub struct GlobalConfig {
    pub remi: RemiConfig,
    pub paths: PathsConfig,
    #[serde(default)]
    pub setup: SetupConfig,
    #[serde(default)]
    pub distros: HashMap<String, DistroConfig>,
    #[serde(default)]
    pub fixtures: Option<FixtureConfig>,
}

impl GlobalConfig {
    /// Apply environment variable overrides.
    pub fn apply_env_overrides(mut self) -> Result<Self> {
        if let Ok(val) = std::env::var("REMI_ENDPOINT") {
            self.remi.endpoint = val;
        }
        if let Ok(val) = std::env::var("DB_PATH") {
            self.paths.db = val;
        }
        if let Ok(val) = std::env::var("CONARY_BIN") {
            self.paths.conary_bin = val;
        }
        if let Ok(val) = std::env::var("RESULTS_DIR") {
            self.paths.results_dir = val;
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemiConfig {
    pub endpoint: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PathsConfig {
    pub db: String,
    pub conary_bin: String,
    pub results_dir: String,
    #[serde(default)]
    pub fixture_dir: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SetupConfig {
    #[serde(default)]
    pub remove_default_repos: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DistroConfig {
    pub remi_distro: String,
    pub repo_name: String,
    #[serde(default)]
    pub containerfile: Option<String>,
    #[serde(default)]
    pub test_package_1: Option<String>,
    #[serde(default)]
    pub test_binary_1: Option<String>,
    #[serde(default)]
    pub test_package_2: Option<String>,
    #[serde(default)]
    pub test_binary_2: Option<String>,
    #[serde(default)]
    pub test_package_3: Option<String>,
    #[serde(default)]
    pub test_binary_3: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixtureConfig {
    #[serde(default)]
    pub test_package_name: Option<String>,
    #[serde(default)]
    pub marker_file_v1: Option<String>,
    #[serde(default)]
    pub marker_file_v2: Option<String>,
    #[serde(default)]
    pub v1_version: Option<String>,
    #[serde(default)]
    pub v2_version: Option<String>,
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p conary-test`
Expected: All 4 tests PASS.

**Step 6: Run clippy**

Run: `cargo clippy -p conary-test -- -D warnings`
Expected: PASS

**Step 7: Commit**

```bash
git add conary-test/src/config/
git commit -m "feat(test): add config module — TOML manifest and distro config parsing"
```

---

### Task 3: Engine module — test suite, results, and runner

Implement the test engine core: `TestSuite` that tracks results, `TestRunner` that executes steps and evaluates assertions, and `TestResult`/`TestStatus` types.

**Files:**
- Create: `conary-test/src/engine/suite.rs`
- Create: `conary-test/src/engine/runner.rs`
- Create: `conary-test/src/engine/assertions.rs`
- Modify: `conary-test/src/engine/mod.rs`

**Step 1: Write tests for engine types and assertions**

Add to `conary-test/src/engine/mod.rs`:

```rust
// conary-test/src/engine/mod.rs

pub mod assertions;
pub mod runner;
pub mod suite;

pub use assertions::evaluate_assertion;
pub use runner::TestRunner;
pub use suite::{RunStatus, TestResult, TestStatus, TestSuite};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::manifest::Assertion;

    #[test]
    fn test_suite_tracks_results() {
        let mut suite = TestSuite::new("test-suite", 1);
        suite.record(TestResult {
            id: "T01".into(),
            name: "first".into(),
            status: TestStatus::Passed,
            duration_ms: 100,
            message: None,
            stdout: None,
            stderr: None,
        });
        suite.record(TestResult {
            id: "T02".into(),
            name: "second".into(),
            status: TestStatus::Failed,
            duration_ms: 200,
            message: Some("expected 0 got 1".into()),
            stdout: Some("output".into()),
            stderr: None,
        });

        assert_eq!(suite.passed(), 1);
        assert_eq!(suite.failed(), 1);
        assert_eq!(suite.total(), 2);
        assert!(suite.has_failed("T02"));
        assert!(!suite.has_failed("T01"));
    }

    #[test]
    fn test_suite_dependency_check() {
        let mut suite = TestSuite::new("deps", 1);
        suite.record(TestResult {
            id: "T01".into(),
            name: "first".into(),
            status: TestStatus::Failed,
            duration_ms: 50,
            message: None,
            stdout: None,
            stderr: None,
        });

        // T02 depends on T01 which failed
        assert!(suite.should_skip(&Some(vec!["T01".into()])));
        // T03 depends on nothing
        assert!(!suite.should_skip(&None));
        // T04 depends on T99 which hasn't run (treat as not-failed)
        assert!(!suite.should_skip(&Some(vec!["T99".into()])));
    }

    #[test]
    fn test_assertion_exit_code() {
        let assertion = Assertion {
            exit_code: Some(0),
            stdout_contains: None,
            stdout_not_contains: None,
            stderr_contains: None,
            file_exists: None,
            file_not_exists: None,
            file_checksum: None,
        };

        let result = evaluate_assertion(&assertion, 0, "hello", "");
        assert!(result.is_ok());

        let result = evaluate_assertion(&assertion, 1, "", "error");
        assert!(result.is_err());
    }

    #[test]
    fn test_assertion_stdout_contains() {
        let assertion = Assertion {
            exit_code: None,
            stdout_contains: Some("ok".into()),
            stdout_not_contains: None,
            stderr_contains: None,
            file_exists: None,
            file_not_exists: None,
            file_checksum: None,
        };

        assert!(evaluate_assertion(&assertion, 0, "status: ok", "").is_ok());
        assert!(evaluate_assertion(&assertion, 0, "status: fail", "").is_err());
    }

    #[test]
    fn test_assertion_combined() {
        let assertion = Assertion {
            exit_code: Some(0),
            stdout_contains: Some("ok".into()),
            stdout_not_contains: Some("error".into()),
            stderr_contains: None,
            file_exists: None,
            file_not_exists: None,
            file_checksum: None,
        };

        assert!(evaluate_assertion(&assertion, 0, "status: ok", "").is_ok());
        assert!(evaluate_assertion(&assertion, 0, "ok error", "").is_err());
        assert!(evaluate_assertion(&assertion, 1, "ok", "").is_err());
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p conary-test`
Expected: FAIL — types don't exist.

**Step 3: Implement `suite.rs`**

```rust
// conary-test/src/engine/suite.rs

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;

/// Overall status of a test run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Pending,
    Running,
    Completed,
    Cancelled,
}

/// Individual test result status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
}

/// Result of a single test.
#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub id: String,
    pub name: String,
    pub status: TestStatus,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

/// Tracks all results for a test suite run.
#[derive(Debug, Clone, Serialize)]
pub struct TestSuite {
    pub name: String,
    pub phase: u32,
    pub status: RunStatus,
    pub results: Vec<TestResult>,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    failed_ids: HashMap<String, bool>,
}

impl TestSuite {
    pub fn new(name: &str, phase: u32) -> Self {
        Self {
            name: name.to_string(),
            phase,
            status: RunStatus::Pending,
            results: Vec::new(),
            started_at: Utc::now(),
            finished_at: None,
            failed_ids: HashMap::new(),
        }
    }

    /// Record a test result.
    pub fn record(&mut self, result: TestResult) {
        if result.status == TestStatus::Failed {
            self.failed_ids.insert(result.id.clone(), true);
        }
        self.results.push(result);
    }

    /// Check if a test with the given ID has failed.
    pub fn has_failed(&self, id: &str) -> bool {
        self.failed_ids.contains_key(id)
    }

    /// Check if a test should be skipped based on its dependencies.
    /// Returns true if ANY dependency has failed.
    pub fn should_skip(&self, depends_on: &Option<Vec<String>>) -> bool {
        match depends_on {
            None => false,
            Some(deps) => deps.iter().any(|dep| self.has_failed(dep)),
        }
    }

    pub fn passed(&self) -> usize {
        self.results.iter().filter(|r| r.status == TestStatus::Passed).count()
    }

    pub fn failed(&self) -> usize {
        self.results.iter().filter(|r| r.status == TestStatus::Failed).count()
    }

    pub fn skipped(&self) -> usize {
        self.results.iter().filter(|r| r.status == TestStatus::Skipped).count()
    }

    pub fn total(&self) -> usize {
        self.results.len()
    }

    /// Mark the suite as complete.
    pub fn finish(&mut self) {
        self.status = RunStatus::Completed;
        self.finished_at = Some(Utc::now());
    }
}
```

**Step 4: Implement `assertions.rs`**

```rust
// conary-test/src/engine/assertions.rs

use crate::config::manifest::Assertion;
use anyhow::{bail, Result};

/// Evaluate an assertion against command output.
pub fn evaluate_assertion(
    assertion: &Assertion,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) -> Result<()> {
    if let Some(expected) = assertion.exit_code {
        if exit_code != expected {
            bail!("expected exit code {expected}, got {exit_code}");
        }
    }

    if let Some(ref needle) = assertion.stdout_contains {
        if !stdout.contains(needle.as_str()) {
            bail!("stdout does not contain \"{needle}\"");
        }
    }

    if let Some(ref needle) = assertion.stdout_not_contains {
        if stdout.contains(needle.as_str()) {
            bail!("stdout unexpectedly contains \"{needle}\"");
        }
    }

    if let Some(ref needle) = assertion.stderr_contains {
        if !stderr.contains(needle.as_str()) {
            bail!("stderr does not contain \"{needle}\"");
        }
    }

    Ok(())
}
```

**Step 5: Create placeholder `runner.rs`**

```rust
// conary-test/src/engine/runner.rs

use crate::config::distro::GlobalConfig;
use crate::config::manifest::TestManifest;
use crate::engine::suite::TestSuite;

/// Executes tests from a manifest against a container.
pub struct TestRunner {
    pub config: GlobalConfig,
    pub manifest: TestManifest,
    pub distro: String,
}

impl TestRunner {
    pub fn new(config: GlobalConfig, manifest: TestManifest, distro: String) -> Self {
        Self { config, manifest, distro }
    }
}
```

The actual `run()` method will be implemented in Task 6 after the container module exists.

**Step 6: Run tests to verify they pass**

Run: `cargo test -p conary-test`
Expected: All tests PASS.

**Step 7: Run clippy**

Run: `cargo clippy -p conary-test -- -D warnings`
Expected: PASS

**Step 8: Commit**

```bash
git add conary-test/src/engine/
git commit -m "feat(test): add engine module — TestSuite, assertions, runner skeleton"
```

---

### Task 4: Container module — bollard backend

Implement the `ContainerBackend` trait and `BollardBackend` implementation for managing Podman containers.

**Files:**
- Create: `conary-test/src/container/backend.rs`
- Create: `conary-test/src/container/image.rs`
- Create: `conary-test/src/container/lifecycle.rs`
- Modify: `conary-test/src/container/mod.rs`

**Step 1: Write tests for container types**

Add to `conary-test/src/container/mod.rs`:

```rust
// conary-test/src/container/mod.rs

pub mod backend;
pub mod image;
pub mod lifecycle;

pub use backend::{BollardBackend, ContainerBackend, ContainerConfig, ContainerId, ExecResult};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_container_config_defaults() {
        let config = ContainerConfig {
            image: "fedora:43".into(),
            env: HashMap::new(),
            volumes: Vec::new(),
            privileged: false,
            network_mode: "bridge".into(),
        };
        assert_eq!(config.image, "fedora:43");
        assert!(!config.privileged);
    }

    #[test]
    fn test_exec_result() {
        let result = ExecResult {
            exit_code: 0,
            stdout: "hello\n".into(),
            stderr: String::new(),
        };
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello"));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p conary-test`
Expected: FAIL — types don't exist.

**Step 3: Implement `backend.rs`**

```rust
// conary-test/src/container/backend.rs

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// Opaque container identifier.
pub type ContainerId = String;

/// Result of executing a command inside a container.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Volume mount specification.
#[derive(Debug, Clone)]
pub struct VolumeMount {
    pub host_path: String,
    pub container_path: String,
    pub read_only: bool,
}

/// Container creation configuration.
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    pub image: String,
    pub env: HashMap<String, String>,
    pub volumes: Vec<VolumeMount>,
    pub privileged: bool,
    pub network_mode: String,
}

/// Backend trait for container operations.
#[async_trait]
pub trait ContainerBackend: Send + Sync {
    async fn build_image(
        &self,
        dockerfile: &Path,
        tag: &str,
        build_args: HashMap<String, String>,
    ) -> Result<String>;

    async fn create(&self, config: ContainerConfig) -> Result<ContainerId>;
    async fn start(&self, id: &ContainerId) -> Result<()>;
    async fn exec(
        &self,
        id: &ContainerId,
        cmd: &[&str],
        timeout: Duration,
    ) -> Result<ExecResult>;
    async fn stop(&self, id: &ContainerId) -> Result<()>;
    async fn remove(&self, id: &ContainerId) -> Result<()>;
    async fn copy_from(&self, id: &ContainerId, path: &str) -> Result<Vec<u8>>;
    async fn copy_to(&self, id: &ContainerId, path: &str, data: &[u8]) -> Result<()>;
    async fn logs(&self, id: &ContainerId) -> Result<String>;
}

/// Bollard-based implementation using Podman's Docker-compatible API.
pub struct BollardBackend {
    docker: bollard::Docker,
}

impl BollardBackend {
    /// Connect to the local Podman/Docker socket.
    pub fn new() -> Result<Self> {
        // bollard auto-detects Podman socket at standard locations
        let docker = bollard::Docker::connect_with_local_defaults()?;
        Ok(Self { docker })
    }

    /// Connect to a specific socket path.
    pub fn with_socket(socket_path: &str) -> Result<Self> {
        let docker = bollard::Docker::connect_with_socket(socket_path, 120, bollard::API_DEFAULT_VERSION)?;
        Ok(Self { docker })
    }
}
```

**Step 4: Implement `lifecycle.rs`**

This implements the `ContainerBackend` trait for `BollardBackend`:

```rust
// conary-test/src/container/lifecycle.rs

use crate::container::backend::{
    BollardBackend, ContainerBackend, ContainerConfig, ContainerId, ExecResult, VolumeMount,
};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use bollard::container::{
    Config, CreateContainerOptions, RemoveContainerOptions, StartContainerOptions,
    StopContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecResults};
use futures::StreamExt;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

#[async_trait]
impl ContainerBackend for BollardBackend {
    async fn build_image(
        &self,
        dockerfile: &Path,
        tag: &str,
        build_args: HashMap<String, String>,
    ) -> Result<String> {
        use bollard::image::BuildImageOptions;
        use std::io::Read;

        let context_dir = dockerfile
            .parent()
            .context("Dockerfile has no parent directory")?;

        // Create tar archive of context directory
        let mut tar_builder = tar::Builder::new(Vec::new());
        tar_builder.append_dir_all(".", context_dir)?;
        let tar_data = tar_builder.into_inner()?;

        let options = BuildImageOptions {
            dockerfile: dockerfile
                .file_name()
                .context("no filename")?
                .to_str()
                .context("non-UTF8 filename")?,
            t: tag,
            buildargs: build_args,
            rm: true,
            ..Default::default()
        };

        let mut stream = self.docker.build_image(options, None, Some(tar_data.into()));

        while let Some(msg) = stream.next().await {
            let info = msg?;
            if let Some(error) = info.error {
                bail!("Image build failed: {error}");
            }
            if let Some(stream) = info.stream {
                tracing::debug!("{}", stream.trim_end());
            }
        }

        Ok(tag.to_string())
    }

    async fn create(&self, config: ContainerConfig) -> Result<ContainerId> {
        let mut binds = Vec::new();
        for vol in &config.volumes {
            let mode = if vol.read_only { "ro" } else { "rw" };
            binds.push(format!("{}:{}:{},Z", vol.host_path, vol.container_path, mode));
        }

        let env: Vec<String> = config
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();

        let container_config = Config {
            image: Some(config.image),
            env: Some(env),
            host_config: Some(bollard::service::HostConfig {
                binds: Some(binds),
                privileged: Some(config.privileged),
                network_mode: Some(config.network_mode),
                ..Default::default()
            }),
            // Keep container alive with a long-running command
            cmd: Some(vec!["sleep".to_string(), "86400".to_string()]),
            ..Default::default()
        };

        let response = self
            .docker
            .create_container(None::<CreateContainerOptions<String>>, container_config)
            .await?;

        Ok(response.id)
    }

    async fn start(&self, id: &ContainerId) -> Result<()> {
        self.docker
            .start_container(id, None::<StartContainerOptions<String>>)
            .await?;
        Ok(())
    }

    async fn exec(
        &self,
        id: &ContainerId,
        cmd: &[&str],
        timeout: Duration,
    ) -> Result<ExecResult> {
        let exec = self
            .docker
            .create_exec(
                id,
                CreateExecOptions {
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
                    ..Default::default()
                },
            )
            .await?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        let output = self.docker.start_exec(&exec.id, None).await?;

        if let StartExecResults::Attached { mut output: stream, .. } = output {
            let collect = async {
                while let Some(msg) = stream.next().await {
                    let chunk = msg?;
                    match chunk {
                        bollard::container::LogOutput::StdOut { message } => {
                            stdout.push_str(&String::from_utf8_lossy(&message));
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            stderr.push_str(&String::from_utf8_lossy(&message));
                        }
                        _ => {}
                    }
                }
                Ok::<_, anyhow::Error>(())
            };

            tokio::time::timeout(timeout, collect)
                .await
                .context("exec timed out")??;
        }

        // Get exit code
        let inspect = self.docker.inspect_exec(&exec.id).await?;
        let exit_code = inspect.exit_code.unwrap_or(-1) as i32;

        Ok(ExecResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    async fn stop(&self, id: &ContainerId) -> Result<()> {
        self.docker
            .stop_container(id, Some(StopContainerOptions { t: 10 }))
            .await?;
        Ok(())
    }

    async fn remove(&self, id: &ContainerId) -> Result<()> {
        self.docker
            .remove_container(
                id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await?;
        Ok(())
    }

    async fn copy_from(&self, id: &ContainerId, path: &str) -> Result<Vec<u8>> {
        let stream = self.docker.download_from_container(id, Some(bollard::container::DownloadFromContainerOptions { path: path.to_string() })).await?;
        // bollard returns a tar archive; extract the first file
        let bytes: Vec<u8> = {
            let mut all_bytes = Vec::new();
            let mut stream = stream;
            while let Some(chunk) = stream.next().await {
                all_bytes.extend_from_slice(&chunk?);
            }
            all_bytes
        };

        // Extract from tar
        let mut archive = tar::Archive::new(&bytes[..]);
        for entry in archive.entries()? {
            let mut entry = entry?;
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut data)?;
            return Ok(data);
        }
        bail!("No file found in container at {path}");
    }

    async fn copy_to(&self, id: &ContainerId, path: &str, data: &[u8]) -> Result<()> {
        use std::path::PathBuf;

        let dest = PathBuf::from(path);
        let filename = dest
            .file_name()
            .context("path has no filename")?
            .to_str()
            .context("non-UTF8 path")?;
        let parent = dest
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".into());

        // Create tar with the file
        let mut tar_builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar_builder.append_data(&mut header, filename, data)?;
        let tar_data = tar_builder.into_inner()?;

        self.docker
            .upload_to_container(
                id,
                Some(bollard::container::UploadToContainerOptions {
                    path: parent,
                    ..Default::default()
                }),
                tar_data.into(),
            )
            .await?;
        Ok(())
    }

    async fn logs(&self, id: &ContainerId) -> Result<String> {
        use bollard::container::LogsOptions;

        let mut output = String::new();
        let mut stream = self.docker.logs(
            id,
            Some(LogsOptions::<String> {
                stdout: true,
                stderr: true,
                ..Default::default()
            }),
        );

        while let Some(msg) = stream.next().await {
            let chunk = msg?;
            output.push_str(&chunk.to_string());
        }
        Ok(output)
    }
}
```

**Step 5: Create placeholder `image.rs`**

```rust
// conary-test/src/container/image.rs

use crate::container::backend::ContainerBackend;
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

/// Build a container image for a distro.
pub async fn build_distro_image(
    backend: &dyn ContainerBackend,
    containerfile: &Path,
    distro: &str,
) -> Result<String> {
    let tag = format!("conary-test-{distro}:latest");
    backend
        .build_image(containerfile, &tag, HashMap::new())
        .await
}
```

**Step 6: Run tests and clippy**

Run: `cargo test -p conary-test`
Expected: All tests PASS.

Run: `cargo clippy -p conary-test -- -D warnings`
Expected: PASS (fix any warnings — bollard API may have deprecation warnings; address them)

**Note:** The bollard `download_from_container` API may differ slightly between versions. Check Context7 for the exact bollard 0.18 API if compilation fails, and adjust accordingly.

**Step 7: Commit**

```bash
git add conary-test/src/container/
git commit -m "feat(test): add container module — bollard backend with ContainerBackend trait"
```

---

### Task 5: Wire runner to container backend

Connect the `TestRunner` to actually execute test steps inside containers via the `ContainerBackend`.

**Files:**
- Modify: `conary-test/src/engine/runner.rs`
- Modify: `conary-test/src/engine/mod.rs` (add tests)

**Step 1: Write integration-style tests using a mock backend**

Add to `conary-test/src/engine/runner.rs`:

```rust
// conary-test/src/engine/runner.rs

use crate::config::distro::GlobalConfig;
use crate::config::manifest::{TestDef, TestManifest, TestStep};
use crate::container::backend::{ContainerBackend, ContainerId, ExecResult};
use crate::engine::assertions::evaluate_assertion;
use crate::engine::suite::{RunStatus, TestResult, TestStatus, TestSuite};
use anyhow::Result;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Executes tests from a manifest against a container.
pub struct TestRunner {
    pub config: GlobalConfig,
    pub distro: String,
}

impl TestRunner {
    pub fn new(config: GlobalConfig, distro: String) -> Self {
        Self { config, distro }
    }

    /// Run all tests from a manifest inside the given container.
    pub async fn run(
        &self,
        manifest: &TestManifest,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
    ) -> Result<TestSuite> {
        let mut suite = TestSuite::new(&manifest.suite.name, manifest.suite.phase);
        suite.status = RunStatus::Running;

        let vars = self.build_vars();

        for test_def in &manifest.test {
            // Check dependencies
            if suite.should_skip(&test_def.depends_on) {
                suite.record(TestResult {
                    id: test_def.id.clone(),
                    name: test_def.name.clone(),
                    status: TestStatus::Skipped,
                    duration_ms: 0,
                    message: Some("skipped: dependency failed".into()),
                    stdout: None,
                    stderr: None,
                });
                continue;
            }

            let result = self
                .run_test(test_def, backend, container_id, &vars)
                .await;
            let is_fatal = test_def.fatal.unwrap_or(false);
            let failed = result.status == TestStatus::Failed;

            suite.record(result);

            if failed && is_fatal {
                tracing::warn!(test_id = %test_def.id, "Fatal test failed, aborting suite");
                break;
            }
        }

        suite.finish();
        Ok(suite)
    }

    /// Run a single test (all its steps).
    async fn run_test(
        &self,
        test_def: &TestDef,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
        vars: &HashMap<String, String>,
    ) -> TestResult {
        let start = Instant::now();
        let timeout = Duration::from_secs(test_def.timeout);

        let mut last_stdout = String::new();
        let mut last_stderr = String::new();

        for step in &test_def.step {
            let step_type = match step.step_type() {
                Some(t) => t,
                None => {
                    return TestResult {
                        id: test_def.id.clone(),
                        name: test_def.name.clone(),
                        status: TestStatus::Failed,
                        duration_ms: start.elapsed().as_millis() as u64,
                        message: Some("step has no action defined".into()),
                        stdout: None,
                        stderr: None,
                    };
                }
            };

            match step_type {
                crate::config::manifest::StepType::Run(cmd) => {
                    let expanded = self.expand_vars(&cmd, vars);
                    match backend
                        .exec(container_id, &["sh", "-c", &expanded], timeout)
                        .await
                    {
                        Ok(exec_result) => {
                            last_stdout = exec_result.stdout.clone();
                            last_stderr = exec_result.stderr.clone();

                            if let Some(ref assertion) = step.assert {
                                if let Err(e) = evaluate_assertion(
                                    assertion,
                                    exec_result.exit_code,
                                    &exec_result.stdout,
                                    &exec_result.stderr,
                                ) {
                                    return TestResult {
                                        id: test_def.id.clone(),
                                        name: test_def.name.clone(),
                                        status: TestStatus::Failed,
                                        duration_ms: start.elapsed().as_millis() as u64,
                                        message: Some(e.to_string()),
                                        stdout: Some(exec_result.stdout),
                                        stderr: Some(exec_result.stderr),
                                    };
                                }
                            }
                        }
                        Err(e) => {
                            return TestResult {
                                id: test_def.id.clone(),
                                name: test_def.name.clone(),
                                status: TestStatus::Failed,
                                duration_ms: start.elapsed().as_millis() as u64,
                                message: Some(format!("exec failed: {e}")),
                                stdout: None,
                                stderr: None,
                            };
                        }
                    }
                }
                crate::config::manifest::StepType::Conary(args) => {
                    let db_path = &self.config.paths.db;
                    let bin = &self.config.paths.conary_bin;
                    let full_cmd = format!("{bin} {args} --db-path {db_path}");
                    match backend
                        .exec(container_id, &["sh", "-c", &full_cmd], timeout)
                        .await
                    {
                        Ok(exec_result) => {
                            last_stdout = exec_result.stdout.clone();
                            last_stderr = exec_result.stderr.clone();

                            if let Some(ref assertion) = step.assert {
                                if let Err(e) = evaluate_assertion(
                                    assertion,
                                    exec_result.exit_code,
                                    &exec_result.stdout,
                                    &exec_result.stderr,
                                ) {
                                    return TestResult {
                                        id: test_def.id.clone(),
                                        name: test_def.name.clone(),
                                        status: TestStatus::Failed,
                                        duration_ms: start.elapsed().as_millis() as u64,
                                        message: Some(e.to_string()),
                                        stdout: Some(exec_result.stdout),
                                        stderr: Some(exec_result.stderr),
                                    };
                                }
                            }
                        }
                        Err(e) => {
                            return TestResult {
                                id: test_def.id.clone(),
                                name: test_def.name.clone(),
                                status: TestStatus::Failed,
                                duration_ms: start.elapsed().as_millis() as u64,
                                message: Some(format!("exec failed: {e}")),
                                stdout: None,
                                stderr: None,
                            };
                        }
                    }
                }
                crate::config::manifest::StepType::Sleep(secs) => {
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                }
                crate::config::manifest::StepType::FileExists(path) => {
                    match backend
                        .exec(container_id, &["test", "-e", &path], timeout)
                        .await
                    {
                        Ok(r) if r.exit_code != 0 => {
                            return TestResult {
                                id: test_def.id.clone(),
                                name: test_def.name.clone(),
                                status: TestStatus::Failed,
                                duration_ms: start.elapsed().as_millis() as u64,
                                message: Some(format!("file does not exist: {path}")),
                                stdout: None,
                                stderr: None,
                            };
                        }
                        Err(e) => {
                            return TestResult {
                                id: test_def.id.clone(),
                                name: test_def.name.clone(),
                                status: TestStatus::Failed,
                                duration_ms: start.elapsed().as_millis() as u64,
                                message: Some(format!("exec failed: {e}")),
                                stdout: None,
                                stderr: None,
                            };
                        }
                        _ => {}
                    }
                }
                crate::config::manifest::StepType::FileNotExists(path) => {
                    match backend
                        .exec(container_id, &["test", "!", "-e", &path], timeout)
                        .await
                    {
                        Ok(r) if r.exit_code != 0 => {
                            return TestResult {
                                id: test_def.id.clone(),
                                name: test_def.name.clone(),
                                status: TestStatus::Failed,
                                duration_ms: start.elapsed().as_millis() as u64,
                                message: Some(format!("file unexpectedly exists: {path}")),
                                stdout: None,
                                stderr: None,
                            };
                        }
                        Err(e) => {
                            return TestResult {
                                id: test_def.id.clone(),
                                name: test_def.name.clone(),
                                status: TestStatus::Failed,
                                duration_ms: start.elapsed().as_millis() as u64,
                                message: Some(format!("exec failed: {e}")),
                                stdout: None,
                                stderr: None,
                            };
                        }
                        _ => {}
                    }
                }
                crate::config::manifest::StepType::FileChecksum(ref chk) => {
                    let cmd = format!("sha256sum {} | cut -d' ' -f1", chk.path);
                    match backend
                        .exec(container_id, &["sh", "-c", &cmd], timeout)
                        .await
                    {
                        Ok(r) => {
                            let actual = r.stdout.trim();
                            if actual != chk.sha256 {
                                return TestResult {
                                    id: test_def.id.clone(),
                                    name: test_def.name.clone(),
                                    status: TestStatus::Failed,
                                    duration_ms: start.elapsed().as_millis() as u64,
                                    message: Some(format!(
                                        "checksum mismatch for {}: expected {}, got {actual}",
                                        chk.path, chk.sha256
                                    )),
                                    stdout: None,
                                    stderr: None,
                                };
                            }
                        }
                        Err(e) => {
                            return TestResult {
                                id: test_def.id.clone(),
                                name: test_def.name.clone(),
                                status: TestStatus::Failed,
                                duration_ms: start.elapsed().as_millis() as u64,
                                message: Some(format!("exec failed: {e}")),
                                stdout: None,
                                stderr: None,
                            };
                        }
                    }
                }
            }
        }

        TestResult {
            id: test_def.id.clone(),
            name: test_def.name.clone(),
            status: TestStatus::Passed,
            duration_ms: start.elapsed().as_millis() as u64,
            message: None,
            stdout: if last_stdout.is_empty() {
                None
            } else {
                Some(last_stdout)
            },
            stderr: if last_stderr.is_empty() {
                None
            } else {
                Some(last_stderr)
            },
        }
    }

    /// Build variable map for ${VAR} interpolation.
    fn build_vars(&self) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        vars.insert(
            "REMI_ENDPOINT".into(),
            self.config.remi.endpoint.clone(),
        );
        vars.insert("DB_PATH".into(), self.config.paths.db.clone());
        vars.insert(
            "CONARY_BIN".into(),
            self.config.paths.conary_bin.clone(),
        );
        vars
    }

    /// Expand ${VAR} references in a string.
    fn expand_vars(&self, input: &str, vars: &HashMap<String, String>) -> String {
        let mut result = input.to_string();
        for (key, value) in vars {
            result = result.replace(&format!("${{{key}}}"), value);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::distro::{GlobalConfig, PathsConfig, RemiConfig, SetupConfig};
    use crate::config::manifest::{Assertion, SuiteDef, TestManifest, TestStep};
    use crate::container::backend::{ContainerConfig, VolumeMount};
    use std::sync::Mutex;

    /// Mock backend that records exec calls and returns preset results.
    struct MockBackend {
        exec_results: Mutex<Vec<ExecResult>>,
        exec_calls: Mutex<Vec<Vec<String>>>,
    }

    impl MockBackend {
        fn new(results: Vec<ExecResult>) -> Self {
            Self {
                exec_results: Mutex::new(results),
                exec_calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl ContainerBackend for MockBackend {
        async fn build_image(
            &self,
            _: &Path,
            tag: &str,
            _: HashMap<String, String>,
        ) -> Result<String> {
            Ok(tag.to_string())
        }
        async fn create(&self, _: ContainerConfig) -> Result<ContainerId> {
            Ok("mock-id".into())
        }
        async fn start(&self, _: &ContainerId) -> Result<()> {
            Ok(())
        }
        async fn exec(
            &self,
            _: &ContainerId,
            cmd: &[&str],
            _: Duration,
        ) -> Result<ExecResult> {
            self.exec_calls
                .lock()
                .unwrap()
                .push(cmd.iter().map(|s| s.to_string()).collect());
            let mut results = self.exec_results.lock().unwrap();
            if results.is_empty() {
                Ok(ExecResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            } else {
                Ok(results.remove(0))
            }
        }
        async fn stop(&self, _: &ContainerId) -> Result<()> {
            Ok(())
        }
        async fn remove(&self, _: &ContainerId) -> Result<()> {
            Ok(())
        }
        async fn copy_from(&self, _: &ContainerId, _: &str) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }
        async fn copy_to(&self, _: &ContainerId, _: &str, _: &[u8]) -> Result<()> {
            Ok(())
        }
        async fn logs(&self, _: &ContainerId) -> Result<String> {
            Ok(String::new())
        }
    }

    fn test_config() -> GlobalConfig {
        GlobalConfig {
            remi: RemiConfig {
                endpoint: "https://test.example.com".into(),
            },
            paths: PathsConfig {
                db: "/tmp/test.db".into(),
                conary_bin: "/usr/bin/conary".into(),
                results_dir: "/tmp/results".into(),
                fixture_dir: None,
            },
            setup: SetupConfig::default(),
            distros: HashMap::new(),
            fixtures: None,
        }
    }

    #[tokio::test]
    async fn test_runner_passes_on_success() {
        let backend = MockBackend::new(vec![ExecResult {
            exit_code: 0,
            stdout: "ok".into(),
            stderr: String::new(),
        }]);

        let manifest = TestManifest {
            suite: SuiteDef {
                name: "test".into(),
                phase: 1,
            },
            test: vec![TestDef {
                id: "T01".into(),
                name: "check".into(),
                description: "desc".into(),
                timeout: 30,
                step: vec![TestStep {
                    run: Some("echo ok".into()),
                    conary: None,
                    file_exists: None,
                    file_not_exists: None,
                    file_checksum: None,
                    sleep: None,
                    assert: Some(Assertion {
                        exit_code: Some(0),
                        stdout_contains: Some("ok".into()),
                        stdout_not_contains: None,
                        stderr_contains: None,
                        file_exists: None,
                        file_not_exists: None,
                        file_checksum: None,
                    }),
                }],
                depends_on: None,
                fatal: None,
                group: None,
            }],
        };

        let runner = TestRunner::new(test_config(), "fedora43".into());
        let suite = runner.run(&manifest, &backend, &"cid".into()).await.unwrap();

        assert_eq!(suite.passed(), 1);
        assert_eq!(suite.failed(), 0);
    }

    #[tokio::test]
    async fn test_runner_fails_on_bad_exit_code() {
        let backend = MockBackend::new(vec![ExecResult {
            exit_code: 1,
            stdout: "error".into(),
            stderr: "fail".into(),
        }]);

        let manifest = TestManifest {
            suite: SuiteDef {
                name: "test".into(),
                phase: 1,
            },
            test: vec![TestDef {
                id: "T01".into(),
                name: "check".into(),
                description: "desc".into(),
                timeout: 30,
                step: vec![TestStep {
                    run: Some("false".into()),
                    conary: None,
                    file_exists: None,
                    file_not_exists: None,
                    file_checksum: None,
                    sleep: None,
                    assert: Some(Assertion {
                        exit_code: Some(0),
                        stdout_contains: None,
                        stdout_not_contains: None,
                        stderr_contains: None,
                        file_exists: None,
                        file_not_exists: None,
                        file_checksum: None,
                    }),
                }],
                depends_on: None,
                fatal: None,
                group: None,
            }],
        };

        let runner = TestRunner::new(test_config(), "fedora43".into());
        let suite = runner.run(&manifest, &backend, &"cid".into()).await.unwrap();

        assert_eq!(suite.passed(), 0);
        assert_eq!(suite.failed(), 1);
        assert!(suite.results[0].message.as_ref().unwrap().contains("exit code"));
    }

    #[tokio::test]
    async fn test_runner_skips_on_dep_failure() {
        let backend = MockBackend::new(vec![ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        }]);

        let manifest = TestManifest {
            suite: SuiteDef {
                name: "test".into(),
                phase: 1,
            },
            test: vec![
                TestDef {
                    id: "T01".into(),
                    name: "first".into(),
                    description: "fails".into(),
                    timeout: 10,
                    step: vec![TestStep {
                        run: Some("false".into()),
                        conary: None,
                        file_exists: None,
                        file_not_exists: None,
                        file_checksum: None,
                        sleep: None,
                        assert: Some(Assertion {
                            exit_code: Some(0),
                            stdout_contains: None,
                            stdout_not_contains: None,
                            stderr_contains: None,
                            file_exists: None,
                            file_not_exists: None,
                            file_checksum: None,
                        }),
                    }],
                    depends_on: None,
                    fatal: None,
                    group: None,
                },
                TestDef {
                    id: "T02".into(),
                    name: "second".into(),
                    description: "depends on T01".into(),
                    timeout: 10,
                    step: vec![TestStep {
                        run: Some("echo hi".into()),
                        conary: None,
                        file_exists: None,
                        file_not_exists: None,
                        file_checksum: None,
                        sleep: None,
                        assert: None,
                    }],
                    depends_on: Some(vec!["T01".into()]),
                    fatal: None,
                    group: None,
                },
            ],
        };

        let runner = TestRunner::new(test_config(), "fedora43".into());
        let suite = runner.run(&manifest, &backend, &"cid".into()).await.unwrap();

        assert_eq!(suite.failed(), 1);
        assert_eq!(suite.skipped(), 1);
        assert_eq!(suite.results[1].status, TestStatus::Skipped);
    }

    #[test]
    fn test_expand_vars() {
        let runner = TestRunner::new(test_config(), "fedora43".into());
        let mut vars = HashMap::new();
        vars.insert("REMI_ENDPOINT".into(), "https://example.com".into());
        let result = runner.expand_vars("curl ${REMI_ENDPOINT}/health", &vars);
        assert_eq!(result, "curl https://example.com/health");
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p conary-test`
Expected: All tests PASS.

**Step 3: Run clippy**

Run: `cargo clippy -p conary-test -- -D warnings`
Expected: PASS

**Step 4: Commit**

```bash
git add conary-test/src/engine/
git commit -m "feat(test): wire test runner to container backend with mock tests"
```

---

### Task 6: Report module — JSON output and SSE streaming

Implement JSON result output (backwards-compatible with existing Python runner schema) and SSE event types.

**Files:**
- Create: `conary-test/src/report/json.rs`
- Create: `conary-test/src/report/stream.rs`
- Modify: `conary-test/src/report/mod.rs`

**Step 1: Write tests**

```rust
// conary-test/src/report/mod.rs

pub mod json;
pub mod stream;

pub use json::write_json_report;
pub use stream::TestEvent;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::suite::{RunStatus, TestResult, TestStatus, TestSuite};

    #[test]
    fn test_json_report_format() {
        let mut suite = TestSuite::new("Phase 1", 1);
        suite.status = RunStatus::Completed;
        suite.record(TestResult {
            id: "T01".into(),
            name: "health_check".into(),
            status: TestStatus::Passed,
            duration_ms: 150,
            message: None,
            stdout: None,
            stderr: None,
        });

        let json = json::to_json_report(&suite).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["suite_name"], "Phase 1");
        assert_eq!(parsed["phase"], 1);
        assert_eq!(parsed["summary"]["passed"], 1);
        assert_eq!(parsed["summary"]["failed"], 0);
        assert_eq!(parsed["results"][0]["id"], "T01");
        assert_eq!(parsed["results"][0]["status"], "passed");
    }

    #[test]
    fn test_event_serialization() {
        let event = TestEvent::TestPassed {
            run_id: 1,
            test_id: "T01".into(),
            duration_ms: 234,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"test_id\":\"T01\""));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p conary-test`
Expected: FAIL

**Step 3: Implement `json.rs`**

```rust
// conary-test/src/report/json.rs

use crate::engine::suite::TestSuite;
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct JsonReport<'a> {
    suite_name: &'a str,
    phase: u32,
    status: &'a str,
    summary: Summary,
    results: &'a [crate::engine::suite::TestResult],
}

#[derive(Serialize)]
struct Summary {
    total: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
}

/// Serialize a test suite to a JSON string.
pub fn to_json_report(suite: &TestSuite) -> Result<String> {
    let report = JsonReport {
        suite_name: &suite.name,
        phase: suite.phase,
        status: match suite.status {
            crate::engine::suite::RunStatus::Pending => "pending",
            crate::engine::suite::RunStatus::Running => "running",
            crate::engine::suite::RunStatus::Completed => "completed",
            crate::engine::suite::RunStatus::Cancelled => "cancelled",
        },
        summary: Summary {
            total: suite.total(),
            passed: suite.passed(),
            failed: suite.failed(),
            skipped: suite.skipped(),
        },
        results: &suite.results,
    };
    Ok(serde_json::to_string_pretty(&report)?)
}

/// Write JSON report to a file.
pub fn write_json_report(suite: &TestSuite, path: &Path) -> Result<()> {
    let json = to_json_report(suite)?;
    std::fs::write(path, json)?;
    Ok(())
}
```

**Step 4: Implement `stream.rs`**

```rust
// conary-test/src/report/stream.rs

use serde::Serialize;

/// Server-Sent Event types for live streaming.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum TestEvent {
    #[serde(rename = "test_started")]
    TestStarted {
        run_id: u64,
        test_id: String,
        name: String,
    },

    #[serde(rename = "test_passed")]
    TestPassed {
        run_id: u64,
        test_id: String,
        duration_ms: u64,
    },

    #[serde(rename = "test_failed")]
    TestFailed {
        run_id: u64,
        test_id: String,
        message: String,
        stdout: Option<String>,
    },

    #[serde(rename = "test_skipped")]
    TestSkipped {
        run_id: u64,
        test_id: String,
        message: String,
    },

    #[serde(rename = "run_complete")]
    RunComplete {
        run_id: u64,
        passed: usize,
        failed: usize,
        skipped: usize,
    },
}

impl TestEvent {
    /// Format as SSE text.
    pub fn to_sse(&self) -> String {
        let (event_name, data) = match self {
            Self::TestStarted { .. } => ("test_started", serde_json::to_string(self).unwrap()),
            Self::TestPassed { .. } => ("test_passed", serde_json::to_string(self).unwrap()),
            Self::TestFailed { .. } => ("test_failed", serde_json::to_string(self).unwrap()),
            Self::TestSkipped { .. } => ("test_skipped", serde_json::to_string(self).unwrap()),
            Self::RunComplete { .. } => ("run_complete", serde_json::to_string(self).unwrap()),
        };
        format!("event: {event_name}\ndata: {data}\n\n")
    }
}
```

**Step 5: Run tests**

Run: `cargo test -p conary-test`
Expected: All tests PASS.

**Step 6: Run clippy and commit**

```bash
cargo clippy -p conary-test -- -D warnings
git add conary-test/src/report/
git commit -m "feat(test): add report module — JSON output and SSE event streaming"
```

---

### Task 7: HTTP server — Axum routes and handlers

Implement the REST API server with health, suites, runs, and image management endpoints.

**Files:**
- Create: `conary-test/src/server/routes.rs`
- Create: `conary-test/src/server/handlers.rs`
- Create: `conary-test/src/server/state.rs`
- Modify: `conary-test/src/server/mod.rs`

**Step 1: Implement shared state**

```rust
// conary-test/src/server/state.rs

use crate::config::distro::GlobalConfig;
use crate::engine::suite::TestSuite;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Run counter for generating unique IDs.
static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Shared server state.
#[derive(Clone)]
pub struct AppState {
    pub config: GlobalConfig,
    pub manifest_dir: String,
    pub runs: Arc<RwLock<HashMap<u64, TestSuite>>>,
}

impl AppState {
    pub fn new(config: GlobalConfig, manifest_dir: String) -> Self {
        Self {
            config,
            manifest_dir,
            runs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn next_run_id() -> u64 {
        RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    }
}
```

**Step 2: Implement handlers**

```rust
// conary-test/src/server/handlers.rs

use crate::server::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub async fn health() -> &'static str {
    "ok"
}

pub async fn list_suites(State(state): State<AppState>) -> Json<Value> {
    // List TOML files in manifest directory
    let mut suites = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&state.manifest_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(manifest) = toml::from_str::<crate::config::manifest::TestManifest>(&content) {
                        suites.push(serde_json::json!({
                            "file": path.file_name().unwrap().to_string_lossy(),
                            "name": manifest.suite.name,
                            "phase": manifest.suite.phase,
                            "tests": manifest.test.len(),
                        }));
                    }
                }
            }
        }
    }
    Json(serde_json::json!({ "suites": suites }))
}

#[derive(Deserialize)]
pub struct StartRunRequest {
    pub suite: String,
    pub distro: String,
    #[serde(default = "default_phase")]
    pub phase: u32,
}

fn default_phase() -> u32 {
    1
}

pub async fn start_run(
    State(state): State<AppState>,
    Json(req): Json<StartRunRequest>,
) -> Result<Json<Value>, StatusCode> {
    let run_id = AppState::next_run_id();

    // Load manifest
    let manifest_path = std::path::PathBuf::from(&state.manifest_dir).join(&req.suite);
    let content = std::fs::read_to_string(&manifest_path).map_err(|_| StatusCode::NOT_FOUND)?;
    let manifest: crate::config::manifest::TestManifest =
        toml::from_str(&content).map_err(|_| StatusCode::BAD_REQUEST)?;

    let suite = crate::engine::suite::TestSuite::new(&manifest.suite.name, manifest.suite.phase);

    // Store the suite
    state.runs.write().await.insert(run_id, suite);

    // TODO: spawn background task to actually run tests (needs container backend)
    tracing::info!(run_id, distro = %req.distro, suite = %req.suite, "Test run queued");

    Ok(Json(serde_json::json!({
        "run_id": run_id,
        "status": "pending",
    })))
}

pub async fn list_runs(State(state): State<AppState>) -> Json<Value> {
    let runs = state.runs.read().await;
    let list: Vec<Value> = runs
        .iter()
        .map(|(id, suite)| {
            serde_json::json!({
                "run_id": id,
                "name": suite.name,
                "status": suite.status,
                "passed": suite.passed(),
                "failed": suite.failed(),
                "total": suite.total(),
            })
        })
        .collect();
    Json(serde_json::json!({ "runs": list }))
}

pub async fn get_run(
    State(state): State<AppState>,
    Path(run_id): Path<u64>,
) -> Result<Json<Value>, StatusCode> {
    let runs = state.runs.read().await;
    let suite = runs.get(&run_id).ok_or(StatusCode::NOT_FOUND)?;
    let json = crate::report::json::to_json_report(suite).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let value: Value = serde_json::from_str(&json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(value))
}

pub async fn list_distros(State(state): State<AppState>) -> Json<Value> {
    let distros: Vec<Value> = state
        .config
        .distros
        .iter()
        .map(|(name, cfg)| {
            serde_json::json!({
                "name": name,
                "remi_distro": cfg.remi_distro,
                "containerfile": cfg.containerfile,
            })
        })
        .collect();
    Json(serde_json::json!({ "distros": distros }))
}
```

**Step 3: Implement routes**

```rust
// conary-test/src/server/routes.rs

use crate::server::handlers;
use crate::server::state::AppState;
use axum::routing::{get, post};
use axum::Router;

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(handlers::health))
        .route("/v1/suites", get(handlers::list_suites))
        .route("/v1/runs", post(handlers::start_run))
        .route("/v1/runs", get(handlers::list_runs))
        .route("/v1/runs/{id}", get(handlers::get_run))
        .route("/v1/distros", get(handlers::list_distros))
        .with_state(state)
}
```

**Step 4: Wire up `server/mod.rs`**

```rust
// conary-test/src/server/mod.rs

pub mod handlers;
pub mod routes;
pub mod state;

pub use routes::create_router;
pub use state::AppState;

use anyhow::Result;

/// Start the HTTP server.
pub async fn run_server(state: AppState, port: u16) -> Result<()> {
    let app = create_router(state);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("conary-test server listening on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}
```

**Step 5: Run tests and clippy**

Run: `cargo test -p conary-test`
Expected: All existing tests PASS.

Run: `cargo clippy -p conary-test -- -D warnings`
Expected: PASS

**Step 6: Commit**

```bash
git add conary-test/src/server/
git commit -m "feat(test): add HTTP server — Axum routes, handlers, shared state"
```

---

### Task 8: MCP server — rmcp tool definitions

Implement the MCP server with tools matching the design: list_suites, start_run, get_run, etc.

**Files:**
- Create: `conary-test/src/server/mcp.rs`
- Modify: `conary-test/src/server/mod.rs`
- Modify: `conary-test/src/server/routes.rs`

**Step 1: Implement MCP server**

```rust
// conary-test/src/server/mcp.rs

use crate::server::state::AppState;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool_router;
use rmcp::{Error as McpError, ServerHandler};
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartRunParams {
    /// Path to test suite TOML file
    pub suite: String,
    /// Distro name (e.g. "fedora43")
    pub distro: String,
    /// Test phase (1 or 2)
    #[serde(default = "default_phase")]
    pub phase: u32,
}

fn default_phase() -> u32 {
    1
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunIdParams {
    /// Run ID
    pub run_id: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TestIdParams {
    /// Run ID
    pub run_id: u64,
    /// Test ID (e.g. "T01")
    pub test_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRunsParams {
    /// Maximum number of runs to return
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    10
}

#[derive(Clone)]
pub struct TestMcpServer {
    state: AppState,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl TestMcpServer {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    fn text_result(text: String) -> CallToolResult {
        CallToolResult::text(text)
    }

    fn json_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
        let text = serde_json::to_string_pretty(value)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(Self::text_result(text))
    }
}

#[tool_router]
impl TestMcpServer {
    /// List available test suites
    #[tool(description = "List all available test suite TOML manifests with their names, phases, and test counts")]
    async fn list_suites(&self) -> Result<CallToolResult, McpError> {
        let mut suites = Vec::new();
        let entries = std::fs::read_dir(&self.state.manifest_dir)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(manifest) = toml::from_str::<crate::config::manifest::TestManifest>(&content) {
                        suites.push(serde_json::json!({
                            "file": path.file_name().unwrap().to_string_lossy(),
                            "name": manifest.suite.name,
                            "phase": manifest.suite.phase,
                            "tests": manifest.test.len(),
                        }));
                    }
                }
            }
        }
        Self::json_result(&suites)
    }

    /// Start a test run
    #[tool(description = "Start a new test run for a given suite and distro. Returns a run_id for tracking.")]
    async fn start_run(&self, params: StartRunParams) -> Result<CallToolResult, McpError> {
        let run_id = AppState::next_run_id();
        let manifest_path = std::path::PathBuf::from(&self.state.manifest_dir).join(&params.suite);
        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| McpError::invalid_params(format!("Suite not found: {e}"), None))?;
        let manifest: crate::config::manifest::TestManifest = toml::from_str(&content)
            .map_err(|e| McpError::invalid_params(format!("Invalid manifest: {e}"), None))?;

        let suite = crate::engine::suite::TestSuite::new(&manifest.suite.name, manifest.suite.phase);
        self.state.runs.write().await.insert(run_id, suite);

        tracing::info!(run_id, distro = %params.distro, "MCP: test run queued");
        Self::json_result(&serde_json::json!({ "run_id": run_id, "status": "pending" }))
    }

    /// Get test run status and results
    #[tool(description = "Get the status and results for a specific test run by run_id")]
    async fn get_run(&self, params: RunIdParams) -> Result<CallToolResult, McpError> {
        let runs = self.state.runs.read().await;
        let suite = runs
            .get(&params.run_id)
            .ok_or_else(|| McpError::invalid_params("Run not found", None))?;
        let json = crate::report::json::to_json_report(suite)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(Self::text_result(json))
    }

    /// List recent test runs
    #[tool(description = "List recent test runs with their status and summary counts")]
    async fn list_runs(&self, params: ListRunsParams) -> Result<CallToolResult, McpError> {
        let runs = self.state.runs.read().await;
        let mut list: Vec<serde_json::Value> = runs
            .iter()
            .map(|(id, suite)| {
                serde_json::json!({
                    "run_id": id,
                    "name": suite.name,
                    "status": suite.status,
                    "passed": suite.passed(),
                    "failed": suite.failed(),
                })
            })
            .collect();
        list.truncate(params.limit);
        Self::json_result(&list)
    }

    /// Get a single test result
    #[tool(description = "Get detailed result for a single test within a run, including stdout/stderr")]
    async fn get_test(&self, params: TestIdParams) -> Result<CallToolResult, McpError> {
        let runs = self.state.runs.read().await;
        let suite = runs
            .get(&params.run_id)
            .ok_or_else(|| McpError::invalid_params("Run not found", None))?;
        let result = suite
            .results
            .iter()
            .find(|r| r.id == params.test_id)
            .ok_or_else(|| McpError::invalid_params("Test not found", None))?;
        Self::json_result(result)
    }

    /// List configured distros
    #[tool(description = "List all configured distros available for testing")]
    async fn list_distros(&self) -> Result<CallToolResult, McpError> {
        let distros: Vec<serde_json::Value> = self
            .state
            .config
            .distros
            .iter()
            .map(|(name, cfg)| {
                serde_json::json!({
                    "name": name,
                    "remi_distro": cfg.remi_distro,
                })
            })
            .collect();
        Self::json_result(&distros)
    }
}
```

**Step 2: Add MCP to server/mod.rs**

Add `pub mod mcp;` to `conary-test/src/server/mod.rs`.

**Step 3: Add MCP route to routes.rs**

In `create_router()`, add the MCP service nest. Follow the same pattern as Remi's `create_external_admin_router`:

```rust
// In routes.rs, add to create_router():
let state_for_mcp = state.clone();
let mcp_service = rmcp::transport::streamable_http_server::StreamableHttpService::new(
    move || Ok(crate::server::mcp::TestMcpServer::new(state_for_mcp.clone())),
    std::sync::Arc::new(rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default()),
    Default::default(),
);

// Add to router:
.nest_service("/mcp", mcp_service)
```

**Step 4: Run tests and clippy**

Run: `cargo test -p conary-test`
Expected: PASS

Run: `cargo clippy -p conary-test -- -D warnings`
Expected: PASS

**Step 5: Commit**

```bash
git add conary-test/src/server/
git commit -m "feat(test): add MCP server with rmcp tool definitions"
```

---

### Task 9: Wire CLI to real functionality

Connect the CLI subcommands to the actual engine, container backend, and server.

**Files:**
- Modify: `conary-test/src/cli.rs`

**Step 1: Update cli.rs to use real modules**

Update `Commands::Serve` to call `server::run_server()`:

```rust
Commands::Serve { port } => {
    let config = load_config()?;
    let state = conary_test::server::AppState::new(config, manifest_dir());
    tokio::runtime::Runtime::new()?
        .block_on(conary_test::server::run_server(state, port))
}
```

Update `Commands::Run` to create a BollardBackend, load manifest, build image, create container, run tests, and print results:

```rust
Commands::Run { distro, phase, suite, all_distros } => {
    tokio::runtime::Runtime::new()?.block_on(async {
        let config = load_config()?;
        let backend = conary_test::container::BollardBackend::new()?;

        // Build image
        let distro_config = config.distros.get(&distro)
            .ok_or_else(|| anyhow::anyhow!("Unknown distro: {distro}"))?;
        let containerfile = distro_config.containerfile.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No containerfile for {distro}"))?;

        let tag = conary_test::container::image::build_distro_image(
            &backend,
            std::path::Path::new(containerfile),
            &distro,
        ).await?;

        // Create and start container
        let container_id = backend.create(conary_test::container::ContainerConfig {
            image: tag,
            env: std::collections::HashMap::new(),
            volumes: Vec::new(),
            privileged: true,
            network_mode: "host".into(),
        }).await?;
        backend.start(&container_id).await?;

        // Load manifest
        let suite_path = suite.unwrap_or_else(|| format!("phase{phase}.toml"));
        let manifest_path = std::path::PathBuf::from(manifest_dir()).join(&suite_path);
        let manifest = conary_test::config::load_manifest(&manifest_path)?;

        // Run tests
        let runner = conary_test::engine::TestRunner::new(config.clone(), distro.clone());
        let suite_result = runner.run(&manifest, &backend, &container_id).await?;

        // Print results
        println!("{}", conary_test::report::json::to_json_report(&suite_result)?);

        // Write to results dir
        let results_path = std::path::PathBuf::from(&config.paths.results_dir)
            .join(format!("{distro}-phase{phase}.json"));
        if let Some(parent) = results_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        conary_test::report::write_json_report(&suite_result, &results_path)?;

        // Cleanup
        backend.stop(&container_id).await?;
        backend.remove(&container_id).await?;

        if suite_result.failed() > 0 {
            std::process::exit(1);
        }
        Ok(())
    })
}
```

Add helper functions at the top of cli.rs:

```rust
fn load_config() -> Result<conary_test::config::distro::GlobalConfig> {
    let path = std::env::var("CONARY_TEST_CONFIG")
        .unwrap_or_else(|_| "tests/integration/remi/config.toml".into());
    conary_test::config::load_global_config(std::path::Path::new(&path))
}

fn manifest_dir() -> String {
    std::env::var("CONARY_TEST_MANIFESTS")
        .unwrap_or_else(|_| "tests/integration/remi/manifests".into())
}
```

**Step 2: Run build and clippy**

Run: `cargo build -p conary-test`
Expected: Compiles.

Run: `cargo clippy -p conary-test -- -D warnings`
Expected: PASS

**Step 3: Commit**

```bash
git add conary-test/src/cli.rs
git commit -m "feat(test): wire CLI subcommands to engine, container backend, and server"
```

---

### Task 10: Convert Phase 1 tests to TOML manifests

Convert the first 10 Phase 1 tests (T01-T10) from Python to declarative TOML manifests. This validates the format works end-to-end.

**Files:**
- Create: `tests/integration/remi/manifests/phase1-core.toml`

**Step 1: Read the Python test runner**

Read `tests/integration/remi/runner/test_runner.py` to understand each test's exact commands and assertions. Focus on T01-T10 in `run_phase1()`.

**Step 2: Create TOML manifest**

Create `tests/integration/remi/manifests/phase1-core.toml` with the TOML equivalents. Example structure (adapt exact commands from what Python does):

```toml
[suite]
name = "Phase 1: Core Remi Integration (T01-T10)"
phase = 1

[[test]]
id = "T01"
name = "remi_health_check"
description = "Verify Remi endpoint is reachable"
timeout = 30

[[test.step]]
run = "curl -sf ${REMI_ENDPOINT}/v1/health"

[test.step.assert]
exit_code = 0

# ... T02 through T10 follow the same pattern
```

Match every assertion the Python tests make. Use `conary` step type for commands that need `--db-path`.

**Step 3: Validate the manifest parses**

Write a quick test or use `cargo test -p conary-test` to load the manifest:

```rust
#[test]
fn test_load_phase1_manifest() {
    let path = std::path::Path::new("tests/integration/remi/manifests/phase1-core.toml");
    if path.exists() {
        let manifest = crate::config::load_manifest(path).unwrap();
        assert!(manifest.test.len() >= 10);
    }
}
```

**Step 4: Commit**

```bash
git add tests/integration/remi/manifests/
git commit -m "feat(test): convert Phase 1 tests T01-T10 to TOML manifest"
```

---

### Task 11: Convert remaining Phase 1 tests (T11-T37) to TOML

**Files:**
- Create: `tests/integration/remi/manifests/phase1-advanced.toml`

Follow the same approach as Task 10. Split into a second manifest file to keep them manageable. Read the Python test runner for T11-T37.

**Step 1: Read Python tests T11-T37**

Read `tests/integration/remi/runner/test_runner.py` focusing on the remaining Phase 1 tests.

**Step 2: Create manifest**

Create `tests/integration/remi/manifests/phase1-advanced.toml` with all remaining Phase 1 tests.

**Step 3: Validate and commit**

```bash
cargo test -p conary-test
git add tests/integration/remi/manifests/
git commit -m "feat(test): convert Phase 1 tests T11-T37 to TOML manifests"
```

---

### Task 12: Update all documentation

Update CLAUDE.md, architecture rules, integration test rules, infrastructure rules, and create the crate README.

**Files:**
- Create: `conary-test/README.md`
- Modify: `CLAUDE.md`
- Modify: `.claude/rules/architecture.md`
- Modify: `.claude/rules/infrastructure.md`
- Modify: `.claude/rules/integration-tests.md`

**Step 1: Create crate README**

Create `conary-test/README.md` covering:
- What the crate does (one paragraph)
- CLI usage examples (`conary-test run`, `conary-test serve`, `conary-test list`)
- Test manifest format (brief example)
- HTTP API endpoint table
- MCP tools table
- Configuration (env vars, config.toml reference)

**Step 2: Update CLAUDE.md**

Add `conary-test` to the Build & Test section:
```
cargo build -p conary-test               # Test infrastructure crate
cargo test -p conary-test                # Test engine unit tests
```

Add to Architecture Glossary:
```
- **conary-test**: Test infrastructure — declarative TOML engine, container management, HTTP API, MCP
```

**Step 3: Update `.claude/rules/architecture.md`**

Add `conary-test` section to the Key Modules table:

```markdown
### conary-test -- Test infrastructure

| Module | Purpose |
|--------|---------|
| `src/config/` | TOML manifest and distro config parsing |
| `src/engine/` | Test suite, runner, assertions |
| `src/container/` | ContainerBackend trait, bollard implementation |
| `src/report/` | JSON output, SSE event streaming |
| `src/server/` | Axum HTTP API, MCP server |
| `src/cli.rs` | Binary entrypoint |
```

Update workspace structure line to mention 5 crates.

**Step 4: Update `.claude/rules/infrastructure.md`**

Add conary-test to Scripts table. Update Integration Tests section to mention both the legacy Python runner and the new Rust engine.

**Step 5: Update `.claude/rules/integration-tests.md`**

Add a section at the top noting the new Rust test engine exists alongside the Python runner. Document:
- How to run via `conary-test run --distro fedora43`
- Where manifests live (`tests/integration/remi/manifests/`)
- How the TOML format maps to the old Python tests

**Step 6: Run clippy on whole workspace to verify nothing broke**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS

**Step 7: Commit**

```bash
git add conary-test/README.md CLAUDE.md .claude/rules/
git commit -m "docs: update all project documentation for conary-test crate"
```

---

### Task 13: Final verification

Run the full test suite and clippy across the workspace. Verify everything compiles and tests pass.

**Step 1: Build the whole workspace**

Run: `cargo build`
Expected: PASS

**Step 2: Run all tests**

Run: `cargo test`
Expected: All tests PASS (~200 existing + new conary-test tests)

**Step 3: Run clippy on everything**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS

**Step 4: Verify conary-test binary works**

Run: `cargo run -p conary-test -- --help`
Expected: Shows help text with `run`, `serve`, `list`, `images` subcommands.

Run: `cargo run -p conary-test -- list`
Expected: Either lists suites or shows empty list (depends on manifest dir).

**Step 5: Commit any final fixes**

If any issues found, fix and commit with appropriate message.
