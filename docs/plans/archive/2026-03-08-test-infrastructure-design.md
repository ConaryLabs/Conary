# Test Infrastructure Crate Design

## Problem

The current integration test system is a 1650-line Python script with a bash
orchestrator and manual Podman management. It works but is fragile, hard to
extend, and has no programmatic API. LLM agents can trigger CI workflows
through Remi's MCP but cannot observe, control, or interact with test
execution directly.

## Decision

New `conary-test` crate in the workspace -- a Rust test engine with declarative
TOML test manifests, container management via bollard, HTTP REST API, and MCP
interface. Fully replaces the Python runner, bash orchestrator, and all
associated scripting.

## Architecture

```
conary-test/
  Cargo.toml
  src/
    lib.rs              # Public API re-exports
    engine/
      mod.rs            # Test engine orchestrator
      suite.rs          # TestSuite, TestResult, TestStatus
      runner.rs         # Executes tests inside containers
      assertions.rs     # Assert helpers
    container/
      mod.rs            # ContainerBackend trait + bollard impl
      image.rs          # Image build/cache management
      lifecycle.rs      # Create, start, exec, stop, remove
    config/
      mod.rs            # TOML config loading
      manifest.rs       # Test manifest parsing
      distro.rs         # Distro-specific config
    report/
      mod.rs            # Result aggregation
      json.rs           # JSON output (backwards-compatible schema)
      stream.rs         # SSE event streaming
    server/
      mod.rs            # Axum HTTP server
      routes.rs         # REST API routes
      handlers.rs       # Request handlers
      mcp.rs            # MCP server (rmcp, standalone)
    cli.rs              # Binary entrypoint + clap CLI
```

**Key dependencies:** bollard (container API), axum + tokio (HTTP), rmcp (MCP),
serde + toml (config), clap (CLI).

## Test Definition Format

Tests are declarative TOML manifests replacing Python test functions.

```toml
[suite]
name = "Phase 1: Core Remi Integration"
phase = 1

[[test]]
id = "T01"
name = "remi_health_check"
description = "Verify Remi endpoint is reachable"
timeout = 30

[[test.step]]
run = "curl -sf ${REMI_ENDPOINT}/v1/health"
assert.exit_code = 0
assert.stdout_contains = "ok"

[[test]]
id = "T02"
name = "system_init"
description = "Initialize conary database"
timeout = 60

[[test.step]]
run = "conary system init"
assert.exit_code = 0

[[test.step]]
run = "conary repo list"
assert.stdout_contains = "default"
```

### Step Types

- `run` -- shell command (supports `${VAR}` interpolation from config)
- `conary` -- shorthand with auto `--db-path`
- `file_exists` / `file_not_exists` -- path assertions
- `file_checksum` -- SHA-256 verification
- `sleep` -- wait for async operations

### Assertions (per step)

- `assert.exit_code` -- expected exit code
- `assert.stdout_contains` / `assert.stdout_not_contains`
- `assert.stderr_contains`
- `assert.file_exists` / `assert.file_not_exists`
- `assert.file_checksum` -- `{path, sha256}`

### Control Flow

- `depends_on = ["T01"]` -- skip if dependency failed
- `fatal = true` -- abort suite on failure
- `group = "A"` -- logical grouping for skip-group behavior

## Container Management

Uses `bollard` to talk to Podman's Docker-compatible API socket.

### ContainerBackend Trait

```rust
#[async_trait]
pub trait ContainerBackend: Send + Sync {
    async fn build_image(&self, dockerfile: &Path, tag: &str, build_args: HashMap<String, String>) -> Result<String>;
    async fn create(&self, config: ContainerConfig) -> Result<ContainerId>;
    async fn start(&self, id: &ContainerId) -> Result<()>;
    async fn exec(&self, id: &ContainerId, cmd: &[&str], timeout: Duration) -> Result<ExecResult>;
    async fn stop(&self, id: &ContainerId) -> Result<()>;
    async fn remove(&self, id: &ContainerId) -> Result<()>;
    async fn copy_from(&self, id: &ContainerId, path: &str) -> Result<Vec<u8>>;
    async fn copy_to(&self, id: &ContainerId, path: &str, data: &[u8]) -> Result<()>;
    async fn logs(&self, id: &ContainerId) -> Result<String>;
}
```

`BollardBackend` implements against the real socket. Test code can mock it.

### ContainerConfig

```rust
pub struct ContainerConfig {
    pub image: String,
    pub env: HashMap<String, String>,
    pub volumes: Vec<VolumeMount>,
    pub privileged: bool,
    pub network_mode: String,
}
```

### Lifecycle

1. Build image if not cached (reuses existing Containerfiles)
2. Create container from distro config
3. Inject conary binary via `copy_to`
4. For each test: `exec` step commands, capture stdout/stderr/exit code, evaluate assertions
5. Extract results via `copy_from`
6. Stop and remove container

The container stays alive for the entire suite -- tests run as exec calls, not
separate container runs. Faster startup, real-time failure reaction.

## HTTP API

**Server:** `conary-test serve --port 9090`

| Method | Path | Purpose |
|--------|------|---------|
| GET | /v1/health | Server health |
| GET | /v1/suites | List available test suites |
| POST | /v1/runs | Start a test run |
| GET | /v1/runs | List runs with status |
| GET | /v1/runs/:id | Run details + results |
| GET | /v1/runs/:id/events | SSE live results stream |
| DELETE | /v1/runs/:id | Cancel running test |
| GET | /v1/distros | List configured distros |
| GET | /v1/images | List built container images |
| POST | /v1/images/build | Build/rebuild a distro image |

## MCP Tools

Standalone at `/mcp` on the test server, plus re-exported in Remi's MCP
(feature-gated library import, no network hop).

| Tool | Purpose |
|------|---------|
| list_suites() | Available test suites |
| start_run(suite, distro, phase) | Kick off a test run |
| get_run(run_id) | Status + results |
| list_runs(limit, status) | Recent runs |
| cancel_run(run_id) | Stop a running test |
| stream_run(run_id) | Current results snapshot |
| list_distros() | Configured distros |
| build_image(distro) | Build/rebuild container image |
| get_test(run_id, test_id) | Single test result with stdout/stderr |
| rerun_test(run_id, test_id) | Re-execute a single failed test |

### SSE Events

```
event: test_started
data: {"run_id": 1, "test_id": "T01", "name": "remi_health_check"}

event: test_passed
data: {"run_id": 1, "test_id": "T01", "duration_ms": 234}

event: test_failed
data: {"run_id": 1, "test_id": "T05", "message": "expected exit 0, got 1", "stdout": "..."}

event: run_complete
data: {"run_id": 1, "passed": 74, "failed": 2, "skipped": 0}
```

## CLI

Separate `conary-test` binary (same pattern as `conary-server`).

```
conary-test run --distro fedora43 --phase 1
conary-test run --suite phase1 --all-distros
conary-test serve --port 9090
conary-test list
conary-test images build --distro fedora43
```

## Remi Integration

`conary-server` adds `conary-test` as an optional dependency behind a feature
gate. Remi's MCP server imports the test engine library and registers the same
tools, calling the engine directly -- no HTTP hop. Same pattern as admin_service.

## Documentation & Deployment

### New docs
- `conary-test/README.md` -- crate overview, CLI usage, manifest format, API
- `docs/TESTING.md` -- replaces `docs/INTEGRATION-TESTING.md`

### Updated docs
- `CLAUDE.md` -- add conary-test to Build & Test, Architecture Glossary, Agents
- `.claude/rules/architecture.md` -- add conary-test module table
- `.claude/rules/infrastructure.md` -- update CI workflows, test runner references
- `.claude/rules/integration-tests.md` -- full rewrite for new system
- `ROADMAP.md` -- update status

### Deployed
- Site redeployed if about page changes
- Remi redeployed if MCP integration is added

### Cleanup
- Delete `tests/integration/remi/runner/test_runner.py`
- Delete `tests/integration/remi/run.sh`
- Update CI workflows to use `conary-test run` instead of `run.sh`

## Not In Scope

- macOS/Windows support (Linux-only)
- Remote container hosts (local Podman socket only)
- Web dashboard (API supports one, not building it)
- Migration tool for old Python tests (manual rewrite to TOML)
