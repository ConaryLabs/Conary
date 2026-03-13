---
last_updated: 2026-03-13
revision: 1
summary: Design spec for hardening the conary-test crate across module structure, error system, API surface, robustness, and code quality
---

# conary-test Crate Hardening Design

**Goal:** Make the conary-test crate bulletproof — split oversized modules, introduce typed errors, expand the MCP/HTTP API for full LLM orchestration, harden robustness, and clean up idiomatic Rust issues.

**Scope:** conary-test crate only (conary-test/).

**Current state:** 6,277 lines across 24 .rs files, 81 unit tests, functional HTTP + MCP interfaces, bollard-based container management, TOML manifest-driven test execution.

---

## 1. Module Restructuring

Split `engine/runner.rs` (1,601 lines) into focused, single-responsibility modules.

### New module layout

| Module | Responsibility | Extracted From |
|--------|---------------|----------------|
| `engine/runner.rs` | Core test loop: iterate tests, track results, coordinate suite lifecycle | runner.rs (keep ~300 lines) |
| `engine/executor.rs` | Step dispatch: match step type via `StepAction` enum, call handlers, collect output | runner.rs |
| `engine/variables.rs` | Variable substitution engine (`{{VAR}}` expansion, distro overrides, env lookups) | runner.rs |
| `engine/container_coordinator.rs` | Container setup/teardown per test, fresh containers for retries, volume mounts, constraint verification | runner.rs |

Existing modules stay as-is: `qemu.rs` (362), `mock_server.rs` (169), `assertions.rs` (137), `suite.rs` (122).

### Public interface

`TestRunner::run()` remains the single entry point. Internal modules communicate through well-defined structs:
- `runner.rs` owns the test loop and delegates to `executor.rs` for each step
- `executor.rs` calls `container_coordinator.rs` for container operations
- `variables.rs` is a pure function layer called by both runner and executor

### Boundary: container_coordinator.rs vs container/lifecycle.rs

- `container/lifecycle.rs` = low-level bollard API calls (create, start, exec, stop, remove, copy). Stays as-is.
- `engine/container_coordinator.rs` = per-test orchestration: decides when to create/destroy containers, handles fresh containers for retries, verifies resource constraints after creation, manages the `Drop` guard for cleanup. Calls into `lifecycle.rs` via the `ContainerBackend` trait.

---

## 2. Error System

Replace `anyhow::Result` with a typed error enum.

### Error enum

New file: `error.rs` at crate root.

```rust
#[derive(Debug, thiserror::Error)]
pub enum ConaryTestError {
    #[error("container error: {message}")]
    Container { message: String, source: Option<Box<dyn std::error::Error + Send + Sync>> },

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
```

### Migration strategy

- `type Result<T> = std::result::Result<T, ConaryTestError>;`
- `Internal` variant wraps anyhow for gradual migration
- MCP layer maps variants to appropriate rmcp error codes
- HTTP layer maps to status codes (404 for NotFound, 408 for Timeout, 409 for Cancelled, 500 for Internal)

---

## 3. MCP & HTTP API Expansion

### Transport note

The MCP endpoint already uses **Streamable HTTP** transport via rmcp's `StreamableHttpService` (MCP spec 2025-03-26). This is the current standard — the older HTTP+SSE MCP transport is deprecated. No transport changes needed.

The SSE streaming described below for test events is a separate, custom HTTP endpoint (`/v1/runs/{id}/stream`) — not part of MCP transport. SSE is the right choice here for push-based event observation over plain HTTP.

### New MCP tools (6 existing + 7 new = 13 total)

| Tool | Signature | Purpose |
|------|-----------|---------|
| `cancel_run` | `(run_id: String)` | Cancel running test, kill container, set Cancelled status |
| `rerun_test` | `(run_id: String, test_id: String, distro: Option<String>)` | Re-run a single failed test, return new run_id |
| `get_test_logs` | `(run_id: String, test_id: String)` | Full stdout/stderr from all steps of a test |
| `build_image` | `(distro: String)` | Build container image, return success/failure + build log |
| `list_images` | `()` | List available built images with tags and sizes |
| `cleanup_containers` | `()` | Remove stopped test containers and dangling images |
| `get_run_artifacts` | `(run_id: String)` | Return JSON report path + generated file list |

### New HTTP endpoints

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/v1/runs/{id}/stream` | SSE stream of TestEvents |
| DELETE | `/v1/runs/{id}` | Cancel a running test |
| POST | `/v1/runs/{id}/tests/{test_id}/rerun` | Re-run single test |
| GET | `/v1/runs/{id}/tests/{test_id}/logs` | Full test logs |
| POST | `/v1/images/build` | Build container image |
| GET | `/v1/images` | List images |
| POST | `/v1/cleanup` | Clean up stopped containers |

### SSE event stream

Wire existing `TestEvent` enum to a `tokio::sync::broadcast` channel. Current variants (all carry `run_id`):

```
TestEvent::TestStarted { run_id, test_id, name }          // exists
TestEvent::TestPassed { run_id, test_id, duration_ms }     // exists
TestEvent::TestFailed { run_id, test_id, message, stdout } // exists
TestEvent::TestSkipped { run_id, test_id, message }        // exists
TestEvent::RunComplete { run_id, passed, failed, skipped } // exists
TestEvent::SuiteStarted { run_id, suite, phase, total }    // new
TestEvent::StepOutput { run_id, test_id, step, line }      // new
```

The runner emits events to the broadcast channel. The SSE endpoint (`/v1/runs/{id}/stream`) subscribes and forwards as `text/event-stream`. MCP clients use polling via `get_run()` / `get_test()` instead.

---

## 4. Robustness Improvements

### Resource isolation verification

After container creation, inspect the container via bollard and verify that requested constraints were actually applied:
- `memory_limit_mb` → check `HostConfig.Memory`
- `tmpfs_size_mb` → check `HostConfig.Tmpfs`
- `network_isolated` → check `HostConfig.NetworkMode`

Fail fast with `ConaryTestError::Container` if the runtime doesn't support a constraint (e.g., rootless podman without cgroup v2).

### Retry hardening

- Each retry gets its own isolated container (already true, enforce explicitly)
- Configurable retry delay with backoff: `retry_delay_ms` manifest field (default 0)
- All retry attempts recorded in results, not just the final one
- `TestResult.attempts: Vec<AttemptResult>` replaces single pass/fail:

```rust
struct AttemptResult {
    attempt: u32,           // 1-indexed
    status: TestStatus,     // Passed | Failed | Skipped | Cancelled
    message: Option<String>,
    stdout: Option<String>,
    stderr: Option<String>,
    duration_ms: u64,
}
```

- JSON report format gains an `attempts` array per test. The top-level `status` and `message` reflect the final attempt for backward compatibility

### Cancellation

- `AppState` stores a `HashMap<u64, Arc<AtomicBool>>` of cancellation flags, keyed by run_id
- `cancel_run` MCP/HTTP tool sets the flag for the given run_id
- `TestRunner` receives the `Arc<AtomicBool>` at construction and checks it between steps and between tests
- Container gets killed immediately on cancel via `ContainerBackend::kill()`
- Partial results preserved — completed tests keep their results, in-progress test marked Cancelled
- The cancellation flag is removed from `AppState` when the run finishes

### Timeout enforcement

- **Suite-level timeout:** total wall clock for the entire suite run (new manifest field: `suite.timeout`)
- **Step-level timeout:** per-step override, separate from test timeout (new step field: `timeout`)
- **Clean teardown on timeout:** container removal runs even when timeout fires, no orphans

### Container cleanup guarantee

- `Drop` guard on container IDs (no new dependency needed) — on runner exit (normal or panic), always remove containers
- `cleanup_containers` MCP/HTTP endpoint for manual cleanup of anything leaked
- Log a warning if cleanup fails (don't mask the original error)

---

## 5. Code Quality & Idioms

### File headers

Verify all .rs files have the path comment per CLAUDE.md convention. Fix any missing.

### Typed step dispatch

Replace the `if/else` chain in step execution with an exhaustive enum:

```rust
enum StepAction {
    Run(String),
    Conary(String),
    FileExists(PathBuf),
    FileNotExists(PathBuf),
    FileExecutable(PathBuf),
    DirExists(PathBuf),
    FileChecksum { path: PathBuf, sha256: String },
    Sleep(u64),
    KillAfterLog { conary: String, pattern: String, timeout_seconds: u64 },
    QemuBoot { image: String, memory_mb: u64, timeout_seconds: u64, ssh_port: u16, commands: Vec<String>, expect_output: Vec<String> },
}
```

Adding a new step type without a handler becomes a compile error. `StepAction` wraps the existing config types from `manifest.rs` (e.g., `QemuBoot` struct) rather than duplicating their fields — it's a dispatch enum, not a data-definition replacement.

### Server state

`AppState` stores run history as `HashMap<u64, TestSuite>` behind `Arc<RwLock<_>>`, capped at 100 by evicting oldest `started_at`. Replace with `DashMap<u64, TestSuite>` (add `dashmap` to `conary-test/Cargo.toml`) for concurrent access without full-collection `RwLock` contention, with LRU eviction on insert when capacity is reached. Also add the cancellation flag map (`DashMap<u64, Arc<AtomicBool>>`) here, as described in Section 4.

### Test coverage expansion

| Area | Current | Target |
|------|---------|--------|
| Config parsing | 9 tests | Keep |
| Assertions | 8 tests | Keep |
| Container config | 5 tests | Keep |
| Runner logic | ~10 tests | Add executor, variables, coordinator unit tests |
| Server/MCP | ~12 tests | Add tests for new tools (cancel, rerun, logs, images, cleanup) |
| SSE streaming | 0 tests | Add broadcast channel + subscriber tests |
| Concurrent runs | 0 tests | Add test for two simultaneous runs |
| Real container | 0 tests | Add `#[ignore]` smoke test requiring podman/docker |
| Error mapping | 0 tests | Add tests for error → HTTP status and error → MCP code mapping |

---

## 6. Shared MCP Infrastructure

Both `conary-server` (Remi, 16 tools) and `conary-test` (6 tools, expanding to 13) use the same rmcp patterns: `#[tool_router]`, `ServerHandler`, `StreamableHttpService`, `Arc<RwLock<State>>`. Both duplicate helper logic:

- `to_json_text()` — serialize to pretty JSON, map errors to `McpError`
- Error-to-MCP mapping (`ServiceError` → `McpError` codes)
- `validate_path_param()` — safe path validation
- `ServerHandler::get_info()` boilerplate

### Extract shared MCP helpers into conary-core

Create `conary-core/src/mcp/mod.rs` (feature-gated behind `mcp` or always available):

```rust
pub fn to_json_text<T: Serialize>(value: &T) -> Result<String, McpError>;
pub fn validate_path_param(value: &str, param_name: &str) -> Result<(), McpError>;
pub fn map_not_found(entity: &str, id: impl Display) -> McpError;
pub fn map_internal(err: impl Display) -> McpError;
```

Both `conary-server/src/server/mcp.rs` and `conary-test/src/server/mcp.rs` import from `conary_core::mcp` instead of duplicating. The typed `ConaryTestError` → `McpError` mapping lives in conary-test (specific to its error enum), but the generic helpers are shared.

This avoids reimplementing the same boilerplate when adding tools to either server.

---

## Files to create or modify

### New files
- `conary-core/src/mcp/mod.rs` — shared MCP helpers (feature-gated if needed)
- `conary-test/src/error.rs` — typed error enum
- `conary-test/src/engine/executor.rs` — step dispatch
- `conary-test/src/engine/variables.rs` — variable substitution
- `conary-test/src/engine/container_coordinator.rs` — container lifecycle per test

### Modified files
- `conary-test/src/engine/runner.rs` — extract internals, keep core loop
- `conary-test/src/engine/mod.rs` — export new modules
- `conary-test/src/server/mcp.rs` — add 7 new tools
- `conary-test/src/server/handlers.rs` — add new endpoints
- `conary-test/src/server/routes.rs` — wire new routes
- `conary-test/src/server/state.rs` — DashMap + broadcast channel + cancellation flags
- `conary-test/src/engine/suite.rs` — add `Cancelled` variant to `TestStatus` enum
- `conary-test/Cargo.toml` — add `dashmap` dependency
- `conary-test/src/server/service.rs` — new service methods
- `conary-test/src/report/stream.rs` — add StepOutput event variant
- `conary-test/src/config/manifest.rs` — add retry_delay_ms, suite timeout, step timeout fields
- `conary-test/src/container/lifecycle.rs` — resource verification, image listing
- `conary-test/src/container/backend.rs` — add `list_images` and `inspect_container` to trait (`build_image` already exists)
- `conary-test/src/lib.rs` — export error module
- `conary-test/src/cli.rs` — wire images list command
- `conary-core/src/lib.rs` — export mcp module
- `conary-server/src/server/mcp.rs` — use shared helpers from conary-core::mcp
