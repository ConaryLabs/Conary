# conary-test

Declarative test infrastructure for Conary integration testing. Replaces the
Python test runner with a Rust engine that reads TOML test manifests, manages
containers via bollard, and exposes an HTTP API and MCP server for orchestration
by CI pipelines and LLM agents.

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
```

## HTTP API

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/v1/health` | Server health check |
| GET | `/v1/suites` | List available test suites |
| POST | `/v1/runs` | Start a new test run |
| GET | `/v1/runs` | List recent runs |
| GET | `/v1/runs/{id}` | Get run details and results |
| GET | `/v1/distros` | List configured distros |

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
