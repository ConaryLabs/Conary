# conary-test Crate Hardening Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden the conary-test crate with module decomposition, typed errors, expanded MCP/HTTP API for full LLM orchestration, robustness improvements, and code quality fixes.

**Architecture:** Extract runner.rs into focused modules (executor, variables, container coordinator), replace anyhow with typed errors, expand MCP from 6 to 13 tools, add SSE streaming, shared MCP helpers in conary-core, and DashMap-based concurrent state.

**Tech Stack:** Rust 2024, rmcp (Streamable HTTP), bollard, axum, tokio, DashMap, thiserror, serde.

---

## Chunk 0: Foundation — Error System and Shared MCP Helpers

### Task 0: Create typed error enum

**Files:**
- Create: `conary-test/src/error.rs`
- Modify: `conary-test/src/lib.rs`

- [ ] **Step 1: Write error enum with tests**

Create `conary-test/src/error.rs`:

```rust
// conary-test/src/error.rs

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConaryTestError {
    #[error("container error: {message}")]
    Container {
        message: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("assertion failed in {test_id}: {message}")]
    AssertionFailed { test_id: String, message: String },

    #[error("test timed out after {timeout_secs}s: {test_id}")]
    Timeout { test_id: String, timeout_secs: u64 },

    #[error("config error: {0}")]
    Config(String),

    #[error("manifest error in {file}: {message}")]
    Manifest { file: String, message: String },

    #[error("qemu error: {0}")]
    Qemu(String),

    #[error("mock server error: {0}")]
    MockServer(String),

    #[error("run not found: {0}")]
    RunNotFound(String),

    #[error("test not found: {run_id}/{test_id}")]
    TestNotFound { run_id: String, test_id: String },

    #[error("run cancelled: {0}")]
    Cancelled(String),

    #[error("{0}")]
    Internal(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, ConaryTestError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_error_displays_message() {
        let err = ConaryTestError::Container {
            message: "failed to start".to_string(),
            source: None,
        };
        assert_eq!(err.to_string(), "container error: failed to start");
    }

    #[test]
    fn assertion_error_includes_test_id() {
        let err = ConaryTestError::AssertionFailed {
            test_id: "T01".to_string(),
            message: "exit code mismatch".to_string(),
        };
        assert!(err.to_string().contains("T01"));
    }

    #[test]
    fn internal_wraps_anyhow() {
        let anyhow_err = anyhow::anyhow!("something went wrong");
        let err: ConaryTestError = anyhow_err.into();
        assert!(err.to_string().contains("something went wrong"));
    }
}
```

- [ ] **Step 2: Export from lib.rs**

Add `pub mod error;` to `conary-test/src/lib.rs` after `pub mod container;`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p conary-test error -- --nocapture`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add conary-test/src/error.rs conary-test/src/lib.rs
git commit -m "feat(test): add typed ConaryTestError enum"
```

### Task 1: Extract shared MCP helpers into conary-core

**Files:**
- Create: `conary-core/src/mcp/mod.rs`
- Modify: `conary-core/src/lib.rs`
- Modify: `conary-server/src/server/mcp.rs`
- Modify: `conary-test/src/server/mcp.rs`

- [ ] **Step 1: Create shared helpers module**

Create `conary-core/src/mcp/mod.rs`:

```rust
// conary-core/src/mcp/mod.rs

//! Shared MCP helper functions for Remi and conary-test servers.

use rmcp::ErrorData as McpError;
use serde::Serialize;
use std::fmt::Display;

/// Serialize a value to pretty JSON, mapping failures to [`McpError`].
pub fn to_json_text<T: Serialize>(value: &T) -> std::result::Result<String, McpError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))
}

/// Validate a path parameter against a safe pattern for URL interpolation.
///
/// Rejects values containing slashes, `..`, null bytes, or characters
/// outside `[a-zA-Z0-9._-]`.
pub fn validate_path_param(value: &str, param_name: &str) -> std::result::Result<(), McpError> {
    if value.is_empty()
        || value.contains('/')
        || value.contains("..")
        || value.contains('\0')
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        Err(McpError::invalid_params(
            format!("Invalid {param_name}: must match [a-zA-Z0-9._-]+"),
            None,
        ))
    } else {
        Ok(())
    }
}

/// Create a "not found" MCP error for a named entity.
pub fn map_not_found(entity: &str, id: impl Display) -> McpError {
    McpError::resource_not_found(format!("{entity} '{id}' not found"), None)
}

/// Create an internal MCP error from any displayable error.
pub fn map_internal(err: impl Display) -> McpError {
    McpError::internal_error(err.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_json_text_serializes_struct() {
        let val = serde_json::json!({"key": "value"});
        let text = to_json_text(&val).unwrap();
        assert!(text.contains("\"key\""));
    }

    #[test]
    fn validate_path_param_rejects_slash() {
        assert!(validate_path_param("foo/bar", "test").is_err());
    }

    #[test]
    fn validate_path_param_accepts_valid() {
        assert!(validate_path_param("ci.yaml", "workflow").is_ok());
    }
}
```

- [ ] **Step 2: Add rmcp dependency to conary-core Cargo.toml**

Add to `conary-core/Cargo.toml` under `[dependencies]`:
```toml
rmcp = { workspace = true, features = ["server"], optional = true }
```

Add a feature gate:
```toml
[features]
mcp = ["rmcp"]
```

- [ ] **Step 3: Export from conary-core/src/lib.rs**

Add `#[cfg(feature = "mcp")] pub mod mcp;` to `conary-core/src/lib.rs`.

- [ ] **Step 4: Update conary-server to use shared helpers**

In `conary-server/src/server/mcp.rs`:
- Remove local `to_json_text()` and `validate_path_param()` functions
- Add `use conary_core::mcp::{to_json_text, validate_path_param, map_not_found, map_internal};`
- Replace `McpError::invalid_params(format!("... not found"))` calls with `map_not_found()`
- Replace `McpError::internal_error(...)` calls with `map_internal()`

In `conary-server/Cargo.toml`, ensure `conary-core` dependency includes `features = ["mcp"]`.

- [ ] **Step 5: Update conary-test to use shared helpers**

In `conary-test/src/server/mcp.rs`:
- Remove the local `to_json_text()` function (only helper present — `validate_path_param` is not in this file)
- Add `use conary_core::mcp::to_json_text;`

In `conary-test/Cargo.toml`, ensure `conary-core` dependency includes `features = ["mcp"]`.

- [ ] **Step 6: Add server_info helper for ServerHandler boilerplate**

In `conary-core/src/mcp/mod.rs`, add a helper that both servers can use:
```rust
pub fn server_info(name: &str, version: &str, instructions: &str) -> ServerInfo {
    InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
        .with_server_info(Implementation::new(name, version))
        .with_instructions(instructions)
}
```

Update both `conary-server/src/server/mcp.rs` and `conary-test/src/server/mcp.rs` `ServerHandler::get_info()` implementations to call `conary_core::mcp::server_info()`.

- [ ] **Step 7: Run tests across all crates**

Run: `cargo test -p conary-core mcp -- --nocapture`
Run: `cargo build --features server` (verify Remi still compiles)
Run: `cargo test -p conary-test` (verify test crate still works)
Run: `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 8: Commit**

```bash
git add conary-core/src/mcp/ conary-core/src/lib.rs conary-core/Cargo.toml conary-server/src/server/mcp.rs conary-server/Cargo.toml conary-test/src/server/mcp.rs conary-test/Cargo.toml
git commit -m "refactor(core): extract shared MCP helpers into conary-core"
```

## Chunk 1: Module Restructuring

### Task 2: Extract variable substitution engine

**Files:**
- Create: `conary-test/src/engine/variables.rs`
- Modify: `conary-test/src/engine/runner.rs`
- Modify: `conary-test/src/engine/mod.rs`

- [ ] **Step 1: Identify variable substitution code in runner.rs**

Read `conary-test/src/engine/runner.rs` and find all variable substitution logic — functions that expand `{{VAR}}` placeholders, handle distro overrides, and read environment variables. Extract these into `variables.rs`.

- [ ] **Step 2: Create variables.rs with tests**

Create `conary-test/src/engine/variables.rs` with:
- `pub fn expand_variables(template: &str, vars: &HashMap<String, String>) -> String`
- `pub fn build_test_variables(config: &GlobalConfig, distro: &str, test: &TestDef, overrides: &HashMap<String, String>) -> HashMap<String, String>`
- Tests: basic expansion, missing variable left as-is, distro override precedence

- [ ] **Step 3: Update runner.rs to use variables module**

Replace inline variable substitution in `runner.rs` with calls to `variables::expand_variables()`.

- [ ] **Step 4: Export from mod.rs**

Add `pub mod variables;` to `conary-test/src/engine/mod.rs`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p conary-test`
Expected: All 81+ existing tests pass, plus new variable tests.

- [ ] **Step 6: Commit**

```bash
git add conary-test/src/engine/variables.rs conary-test/src/engine/runner.rs conary-test/src/engine/mod.rs
git commit -m "refactor(test): extract variable substitution engine"
```

### Task 3: Extract step executor with StepAction enum

**Files:**
- Create: `conary-test/src/engine/executor.rs`
- Modify: `conary-test/src/engine/runner.rs`
- Modify: `conary-test/src/engine/mod.rs`

- [ ] **Step 1: Create StepAction enum**

In `conary-test/src/engine/executor.rs`, define the dispatch enum that wraps existing manifest `StepType` variants:

```rust
pub enum StepAction {
    Run(String),
    Conary(String),
    FileExists(PathBuf),
    FileNotExists(PathBuf),
    FileExecutable(PathBuf),
    DirExists(PathBuf),
    FileChecksum { path: PathBuf, sha256: String },
    Sleep(u64),
    KillAfterLog(KillAfterLog),
    QemuBoot(QemuBoot),
}
```

Add `StepAction::from_step_type(step: &StepType, vars: &HashMap<String, String>) -> Self` that converts manifest types with variable expansion.

- [ ] **Step 2: Move step execution logic from runner.rs to executor.rs**

Create `pub async fn execute_step(action: &StepAction, backend: &dyn ContainerBackend, container_id: &ContainerId, timeout: Duration) -> Result<ExecResult>` that handles each variant via exhaustive match.

- [ ] **Step 3: Add tests for executor**

Test: `Run` step produces expected exec result. Test: `FileExists` step with assertion. Test: unknown step type is a compile error (verified by exhaustive match).

- [ ] **Step 4: Update runner.rs to delegate to executor**

Replace the step execution `if/else` chain with `executor::execute_step()` calls.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test -p conary-test`
```bash
git add conary-test/src/engine/executor.rs conary-test/src/engine/runner.rs conary-test/src/engine/mod.rs
git commit -m "refactor(test): extract step executor with StepAction enum"
```

### Task 4: Extract container coordinator

**Files:**
- Create: `conary-test/src/engine/container_coordinator.rs`
- Modify: `conary-test/src/engine/runner.rs`
- Modify: `conary-test/src/engine/mod.rs`
- Modify: `conary-test/src/container/backend.rs`

- [ ] **Step 1: Add inspect_container and list_images to ContainerBackend trait**

In `conary-test/src/container/backend.rs`, add:
```rust
async fn inspect_container(&self, id: &ContainerId) -> Result<ContainerInspection>;
async fn list_images(&self) -> Result<Vec<ImageInfo>>;
```

Define `ContainerInspection` (memory_limit, tmpfs, network_mode) and `ImageInfo` (id, tag, size) structs.

- [ ] **Step 2: Implement in BollardBackend**

In `conary-test/src/container/lifecycle.rs`, implement `inspect_container` using bollard's `inspect_container` API and `list_images` using bollard's `list_images` API.

- [ ] **Step 3: Implement new trait methods on MockBackend**

In `conary-test/src/container/mod.rs` (or wherever `MockBackend` is defined for tests), implement `inspect_container` (return a default `ContainerInspection`) and `list_images` (return empty vec) so tests compile.

- [ ] **Step 4: Create container_coordinator.rs**

Extract from runner.rs:
- `ContainerCoordinator` struct holding `Arc<dyn ContainerBackend>` and cleanup tracking
- `pub async fn setup_container(&mut self, config: &ContainerConfig, resources: &ResourceConstraints) -> Result<ContainerId>` — creates container, verifies resource constraints via inspect
- `pub async fn teardown_container(&mut self, id: &ContainerId) -> Result<()>` — removes container
- `Drop` impl that logs warnings for any containers not explicitly torn down
- Resource verification: after create, inspect and check memory/tmpfs/network match requested constraints

- [ ] **Step 5: Add tests**

Test: `setup_container` with MockBackend verifies inspect is called. Test: `Drop` guard logs warning for leaked container.

- [ ] **Step 6: Update runner.rs to use coordinator**

Replace direct backend create/start/stop/remove calls with coordinator methods.

- [ ] **Step 7: Run tests and commit**

Run: `cargo test -p conary-test`
```bash
git add conary-test/src/engine/container_coordinator.rs conary-test/src/engine/runner.rs conary-test/src/engine/mod.rs conary-test/src/container/backend.rs conary-test/src/container/lifecycle.rs
git commit -m "refactor(test): extract container coordinator with resource verification"
```

## Chunk 2: Robustness Improvements

### Task 5: Add Cancelled status and AttemptResult

**Files:**
- Modify: `conary-test/src/engine/suite.rs`
- Modify: `conary-test/src/config/manifest.rs`

- [ ] **Step 1: Add Cancelled to TestStatus**

In `conary-test/src/engine/suite.rs`, add `Cancelled` variant to the `TestStatus` enum (not `RunStatus`, which already has its own `Cancelled` variant at line 13 — these are separate enums). Update any match arms on `TestStatus`.

- [ ] **Step 2: Add AttemptResult struct**

In `suite.rs`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptResult {
    pub attempt: u32,
    pub status: TestStatus,
    pub message: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub duration_ms: u64,
}
```

Add `pub attempts: Vec<AttemptResult>` to `TestResult`. Ensure top-level `status` and `message` still reflect the final attempt for backward compatibility.

- [ ] **Step 3: Add manifest fields**

In `conary-test/src/config/manifest.rs`, add:
- `retry_delay_ms: Option<u64>` to `TestDef` (default 0)
- `timeout: Option<u64>` to `TestManifest` suite section (suite-level timeout)
- `timeout: Option<u64>` to step definition (step-level timeout override)

- [ ] **Step 4: Add tests**

Test: `TestStatus::Cancelled` serializes correctly. Test: `AttemptResult` round-trips through JSON. Test: manifest with `retry_delay_ms` parses.

- [ ] **Step 5: Commit**

```bash
git add conary-test/src/engine/suite.rs conary-test/src/config/manifest.rs
git commit -m "feat(test): add Cancelled status, AttemptResult, and retry/timeout manifest fields"
```

### Task 6: Implement cancellation and timeout enforcement

**Files:**
- Modify: `conary-test/src/engine/runner.rs`
- Modify: `conary-test/src/engine/container_coordinator.rs`
- Modify: `conary-test/src/server/state.rs`

- [ ] **Step 1: Add cancellation flag to state and runner**

First, add `dashmap` to `conary-test/Cargo.toml` under `[dependencies]`:
```toml
dashmap = "6"
```

Then in `conary-test/src/server/state.rs`, add `pub cancellation_flags: DashMap<u64, Arc<AtomicBool>>` to `AppState`.

In runner, accept `cancel_flag: Arc<AtomicBool>` at construction. Check `cancel_flag.load(Ordering::Relaxed)` between steps and between tests. If set, mark current test as `Cancelled` and stop.

- [ ] **Step 2: Implement suite-level timeout**

In runner, wrap the entire suite execution in `tokio::time::timeout()` using the manifest's `suite.timeout` value. On timeout, cancel remaining tests and trigger container cleanup.

- [ ] **Step 3: Implement step-level timeout**

In executor, use per-step timeout from the step definition if present, falling back to the test-level timeout.

- [ ] **Step 4: Ensure container cleanup on all exit paths**

Container coordinator's `Drop` impl kills and removes any tracked containers. Verify this works on timeout and cancellation paths.

- [ ] **Step 5: Add tests**

Test: cancellation flag stops runner mid-suite. Test: suite timeout triggers cleanup. Test: step timeout is separate from test timeout.

- [ ] **Step 6: Commit**

```bash
git add conary-test/src/engine/runner.rs conary-test/src/engine/container_coordinator.rs conary-test/src/server/state.rs conary-test/Cargo.toml
git commit -m "feat(test): implement cancellation, suite/step timeouts, and cleanup guarantees"
```

## Chunk 3: SSE Streaming and State Modernization

### Task 7: Add broadcast channel and SSE event streaming

**Files:**
- Modify: `conary-test/src/report/stream.rs`
- Modify: `conary-test/src/server/state.rs`
- Modify: `conary-test/src/server/handlers.rs`
- Modify: `conary-test/src/server/routes.rs`
- Modify: `conary-test/src/engine/runner.rs`

- [ ] **Step 1: Add new TestEvent variants**

`TestEvent` already exists in `conary-test/src/report/stream.rs` with 5 variants (TestStarted, TestPassed, TestFailed, TestSkipped, RunComplete). Add two new variants:
```rust
#[serde(rename = "suite_started")]
SuiteStarted { run_id: u64, suite: String, phase: u32, total: usize },

#[serde(rename = "step_output")]
StepOutput { run_id: u64, test_id: String, step: u32, line: String },
```

Update `to_sse()` match arms.

- [ ] **Step 2: Add broadcast channel to AppState**

In `state.rs`, add `pub event_tx: tokio::sync::broadcast::Sender<TestEvent>` with capacity 1024. Create in `AppState::new()`.

- [ ] **Step 3: Wire runner to emit events**

In runner, accept `broadcast::Sender<TestEvent>` and emit events at: suite start, test start, step output, test pass/fail/skip, run complete.

- [ ] **Step 4: Add SSE endpoint**

In `handlers.rs`, add `pub async fn stream_run(...)` that subscribes to the broadcast channel, filters by `run_id`, and streams as `text/event-stream`.

In `routes.rs`, add `GET /v1/runs/:id/stream` route.

- [ ] **Step 5: Add tests**

Test: broadcast channel delivers events to subscriber. Test: SSE formatting matches `text/event-stream` spec. Test: filtering by run_id works.

- [ ] **Step 6: Commit**

```bash
git add conary-test/src/report/stream.rs conary-test/src/server/state.rs conary-test/src/server/handlers.rs conary-test/src/server/routes.rs conary-test/src/engine/runner.rs
git commit -m "feat(test): add SSE event streaming for live test observation"
```

### Task 8: Modernize AppState with DashMap

**Files:**
- Modify: `conary-test/src/server/state.rs`
- Modify: `conary-test/src/server/service.rs`
- Modify: `conary-test/src/server/handlers.rs`
- Modify: `conary-test/src/server/mcp.rs`

- [ ] **Step 1: Replace HashMap + RwLock with DashMap**

In `state.rs`, change `pub runs: Arc<RwLock<HashMap<u64, TestSuite>>>` to `pub runs: DashMap<u64, TestSuite>`. Update `AppState::new()`. (`dashmap` was already added to Cargo.toml in Task 6.)

- [ ] **Step 2: Update all callers**

In `service.rs`, `handlers.rs`, and `mcp.rs`, replace `state.runs.read().await` / `state.runs.write().await` patterns with `state.runs.get()` / `state.runs.insert()`. DashMap methods are synchronous — no `.await` needed.

- [ ] **Step 3: Add LRU eviction**

In `state.rs`, add a helper that evicts the oldest run (by `started_at`) when `runs.len() >= MAX_RUNS` before inserting.

- [ ] **Step 4: Run tests and commit**

Run: `cargo test -p conary-test`
```bash
git add conary-test/src/server/state.rs conary-test/src/server/service.rs conary-test/src/server/handlers.rs conary-test/src/server/mcp.rs conary-test/Cargo.toml
git commit -m "refactor(test): replace RwLock<HashMap> with DashMap for concurrent state"
```

## Chunk 4: MCP & HTTP API Expansion

### Task 9: Add cancel_run and rerun_test tools

**Files:**
- Modify: `conary-test/src/server/mcp.rs`
- Modify: `conary-test/src/server/handlers.rs`
- Modify: `conary-test/src/server/routes.rs`
- Modify: `conary-test/src/server/service.rs`

- [ ] **Step 1: Add service functions**

In `service.rs`:
- `pub fn cancel_run(state: &AppState, run_id: u64) -> Result<(), ConaryTestError>` — sets cancellation flag
- `pub async fn rerun_test(state: &AppState, run_id: u64, test_id: &str, distro: Option<&str>) -> Result<u64, ConaryTestError>` — finds the test def, creates a new single-test run

- [ ] **Step 2: Add MCP tools**

In `mcp.rs`, add `cancel_run` and `rerun_test` tools using `#[tool]` macro. Map `ConaryTestError` to `McpError`.

- [ ] **Step 3: Add HTTP endpoints**

In `handlers.rs`, add `delete_run` (DELETE `/v1/runs/:id`) and `rerun_test` (POST `/v1/runs/:id/tests/:test_id/rerun`). Wire in `routes.rs`.

- [ ] **Step 4: Add tests**

Test: cancel_run sets flag and returns ok. Test: rerun_test creates new run with single test. Test: HTTP 404 for unknown run_id.

- [ ] **Step 5: Commit**

```bash
git add conary-test/src/server/mcp.rs conary-test/src/server/handlers.rs conary-test/src/server/routes.rs conary-test/src/server/service.rs
git commit -m "feat(test): add cancel_run and rerun_test MCP tools and HTTP endpoints"
```

### Task 10: Add get_test_logs and get_run_artifacts tools

**Files:**
- Modify: `conary-test/src/server/mcp.rs`
- Modify: `conary-test/src/server/handlers.rs`
- Modify: `conary-test/src/server/routes.rs`
- Modify: `conary-test/src/server/service.rs`

- [ ] **Step 1: Add service functions**

- `pub fn get_test_logs(state: &AppState, run_id: u64, test_id: &str) -> Result<TestLogs, ConaryTestError>` — extracts stdout/stderr from all attempts
- `pub fn get_run_artifacts(state: &AppState, run_id: u64) -> Result<RunArtifacts, ConaryTestError>` — returns JSON report path + file list

- [ ] **Step 2: Add MCP tools and HTTP endpoints**

MCP: `get_test_logs`, `get_run_artifacts`.
HTTP: `GET /v1/runs/:id/tests/:test_id/logs`, inherent in `GET /v1/runs/:id` for artifacts.

- [ ] **Step 3: Add tests and commit**

```bash
git commit -m "feat(test): add get_test_logs and get_run_artifacts tools"
```

### Task 11: Add image management and cleanup tools

**Files:**
- Modify: `conary-test/src/server/mcp.rs`
- Modify: `conary-test/src/server/handlers.rs`
- Modify: `conary-test/src/server/routes.rs`
- Modify: `conary-test/src/server/service.rs`
- Modify: `conary-test/src/cli.rs`

- [ ] **Step 1: Add service functions**

- `pub async fn build_image(state: &AppState, distro: &str) -> Result<BuildResult, ConaryTestError>`
- `pub async fn list_images(state: &AppState) -> Result<Vec<ImageInfo>, ConaryTestError>`
- `pub async fn cleanup_containers(state: &AppState) -> Result<CleanupResult, ConaryTestError>`

- [ ] **Step 2: Add MCP tools**

`build_image`, `list_images`, `cleanup_containers` — all via `#[tool]` macro.

- [ ] **Step 3: Add HTTP endpoints**

`POST /v1/images/build`, `GET /v1/images`, `POST /v1/cleanup`. Wire in routes.

- [ ] **Step 4: Wire images list CLI command**

In `cli.rs`, implement the `images list` subcommand (currently returns "not yet implemented").

- [ ] **Step 5: Add tests and commit**

```bash
git commit -m "feat(test): add image management and cleanup MCP tools"
```

## Chunk 5: Code Quality and Final Verification

### Task 12: Verify file headers and fix code quality

**Files:**
- All `.rs` files in `conary-test/src/`

- [ ] **Step 1: Verify all file headers**

Check every `.rs` file has the path comment as first line per CLAUDE.md convention. Fix any missing.

- [ ] **Step 2: Run full verification**

```bash
cargo test -p conary-test
cargo clippy -p conary-test -- -D warnings
cargo fmt -p conary-test -- --check
cargo test  # full workspace
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 3: Add error mapping tests**

Test: `ConaryTestError::RunNotFound` maps to HTTP 404.
Test: `ConaryTestError::Timeout` maps to HTTP 408.
Test: `ConaryTestError::Cancelled` maps to HTTP 409.
Test: `ConaryTestError::Container` maps to MCP internal error.

- [ ] **Step 4: Add concurrent run test**

Test: start two runs simultaneously, both complete independently without interference.

- [ ] **Step 5: Add real container smoke test**

Add `#[ignore]` test that requires podman/docker:
```rust
#[tokio::test]
#[ignore] // Requires podman/docker runtime
async fn smoke_test_real_container() {
    // Create BollardBackend, build a minimal image, create container,
    // exec "echo hello", verify output, teardown
}
```

- [ ] **Step 6: Update MCP tool count test**

Update the tool count assertion in `mcp.rs` tests from 6 to 13.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "test(test): add error mapping, concurrent run, and container smoke tests"
```

### Task 13: Update documentation

**Files:**
- Modify: `conary-test/README.md`
- Modify: `.claude/rules/architecture.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update conary-test README**

Update module listing, API endpoint table (13 MCP tools, all HTTP endpoints), error handling section.

- [ ] **Step 2: Update architecture rule**

Add new modules to the conary-test section in `.claude/rules/architecture.md`: `executor.rs`, `variables.rs`, `container_coordinator.rs`, `error.rs`.

- [ ] **Step 3: Update test count in CLAUDE.md**

Update the test count to reflect new tests added.

- [ ] **Step 4: Move spec to archive**

```bash
mkdir -p docs/superpowers/specs/archive
mv docs/superpowers/specs/2026-03-13-conary-test-hardening-design.md docs/superpowers/specs/archive/
```

- [ ] **Step 5: Commit**

```bash
git add conary-test/README.md .claude/rules/architecture.md CLAUDE.md docs/superpowers/
git commit -m "docs: update conary-test documentation for hardening changes"
```
