# conary-test

Declarative test infrastructure for Conary integration testing. Replaces the
Python test runner with a Rust engine that reads TOML test manifests, manages
containers via bollard, and exposes an HTTP API and MCP server for orchestration
by CI pipelines and LLM agents.

## Architecture

| Module | Purpose |
|--------|---------|
| `src/config/` | TOML manifest and distro config parsing |
| `src/container/` | ContainerBackend trait, bollard implementation |
| `src/engine/runner.rs` | Test runner -- executes manifests against containers |
| `src/engine/executor.rs` | Step executor -- StepAction enum, per-step execution |
| `src/engine/variables.rs` | Variable substitution engine (`${VAR}` expansion) |
| `src/engine/container_coordinator.rs` | Container lifecycle orchestration and cleanup |
| `src/engine/suite.rs` | TestSuite, TestResult, RunStatus types |
| `src/engine/assertions.rs` | Assertion evaluation (exit code, stdout, file checks) |
| `src/engine/mock_server.rs` | In-container mock HTTP server for testing |
| `src/engine/qemu.rs` | QEMU boot step support |
| `src/error.rs` | Typed ConaryTestError enum (Container, Timeout, Cancelled, etc.) |
| `src/report/` | JSON output, SSE event streaming |
| `src/server/handlers.rs` | Axum HTTP handlers |
| `src/server/routes.rs` | Router construction (HTTP + MCP) |
| `src/server/service.rs` | Shared business logic for HTTP and MCP |
| `src/server/state.rs` | AppState with DashMap for concurrent run tracking |
| `src/server/mcp.rs` | MCP server (23 tools via rmcp) |
| `src/cli.rs` | Binary entrypoint |

## CLI Usage

```bash
# Run Phase 1 tests on Fedora 43
conary-test run --distro fedora43 --phase 1

# Run a specific suite on all configured distros
conary-test run --suite phase1-core.toml --all-distros

# Start the HTTP/MCP server
conary-test serve --port 9090

# List available test suites
conary-test list

# Build container images
conary-test images build --distro fedora43
```

## Test Manifest Format

Tests are defined in TOML manifests under `tests/integration/remi/manifests/`.
Each manifest declares a suite with metadata and a list of test steps:

```toml
[suite]
name = "phase1-core"
phase = 1
description = "Core Remi integration tests T01-T10"

[[test]]
id = "T01"
name = "health_check"
command = ["conary", "remote", "health"]
assert_contains = ["healthy"]
timeout = 30
```

### Manifest Files

```
tests/integration/remi/manifests/
  phase1-core.toml          # T01-T10
  phase1-advanced.toml      # T11-T37
  phase2-group-a.toml       # T38-T50 (Deep install)
  phase2-group-b.toml       # T51-T57 (Generations)
  phase2-group-c.toml       # T58-T61 (Bootstrap)
  phase2-group-d.toml       # T62-T66 (Recipe/build)
  phase2-group-e.toml       # T67-T71 (Remi client)
  phase2-group-f.toml       # T72-T76 (Self-update)
  phase3-group-g.toml       # Adversarial tests
  phase3-group-h.toml       # Adversarial tests
  phase3-group-i.toml       # Adversarial tests
  phase3-group-j.toml       # Adversarial tests
  phase3-group-k.toml       # Adversarial tests
  phase3-group-l.toml       # Adversarial tests
  phase3-group-m.toml       # Adversarial tests
  phase3-group-n-container.toml  # Container-based adversarial
  phase3-group-n-qemu.toml       # QEMU boot tests
  phase4-group-a.toml       # T160-T176 (Config/Distro/Canonical)
  phase4-group-b.toml       # T177-T195 (Label/Model/Collection)
  phase4-group-c.toml       # T196-T213 (CCS/Bootstrap/Cache)
  phase4-group-d.toml       # T214-T229 (Trust/Federation/Provenance)
  phase4-group-e.toml       # T230-T249 (Cross-distro)
```

## HTTP API

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/v1/health` | Server health check |
| GET | `/v1/suites` | List available test suites |
| POST | `/v1/runs` | Start a new test run |
| GET | `/v1/runs` | List recent runs |
| GET | `/v1/runs/{id}` | Get run details and results |
| GET | `/v1/runs/{id}/stream` | SSE event stream for a run |
| POST | `/v1/runs/{id}/cancel` | Cancel a running test run |
| GET | `/v1/runs/{id}/artifacts` | Get run artifacts and summary |
| POST | `/v1/runs/{id}/tests/{test_id}/rerun` | Re-run a single test |
| GET | `/v1/runs/{id}/tests/{test_id}/logs` | Get test stdout/stderr logs |
| GET | `/v1/distros` | List configured distros |
| GET | `/v1/images` | List available container images |
| POST | `/v1/images/build` | Build a container image |
| POST | `/v1/cleanup` | Remove stopped containers |

The MCP endpoint is mounted at `/mcp` (Streamable HTTP transport).

## MCP Tools

| Tool | Description |
|------|-------------|
| `list_suites` | List available test suite TOML manifests |
| `start_run` | Start a new test run (suite, distro, phase) |
| `get_run` | Get status and full results for a run |
| `list_runs` | List recent runs with summary info |
| `get_test` | Get a single test result by run ID and test ID |
| `list_distros` | List all configured distros |
| `cancel_run` | Cancel a running test run |
| `rerun_test` | Re-run a single test from a previous run |
| `get_test_logs` | Get stdout/stderr logs from all test attempts |
| `get_run_artifacts` | Get artifact information and summary |
| `build_image` | Build a container image for a distro |
| `list_images` | List available container images |
| `cleanup_containers` | Remove stopped conary-test containers |

## Configuration

Environment variables override values from `tests/integration/remi/config.toml`:

| Variable | Purpose |
|----------|---------|
| `CONARY_TEST_CONFIG` | Path to global config TOML |
| `CONARY_TEST_MANIFESTS` | Path to manifest directory |
| `REMI_ENDPOINT` | Remi server endpoint URL |
| `DB_PATH` | SQLite database path |
| `CONARY_BIN` | Path to conary binary |
| `RESULTS_DIR` | Directory for JSON result output |
| `DISTRO` | Target distro name |

## State Management

Run state is stored in-memory using `DashMap` for lock-free concurrent access.
Each run gets a unique monotonic ID and an optional cancellation flag. The server
supports SSE streaming for live test progress events via `/v1/runs/{id}/stream`.
