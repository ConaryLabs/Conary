---
last_updated: 2026-03-14
revision: 1
summary: Comprehensive overhaul of test infrastructure -- persistence, deployment, unified runner, DX
---

# Test Infrastructure Overhaul Design

## Problem Statement

The conary-test infrastructure has a solid foundation (MCP server, HTTP API,
container-based testing) but the developer experience is fragmented:

- **All state in-memory** -- restart conary-test, all run history and logs are
  lost. No way to review historical failures.
- **Manual deployment** -- rsync + SSH + cargo build + systemctl restart to
  deploy code to Forge. Same for fixtures to Remi. Easy to forget steps.
- **Two test runners** -- Python runner (run.sh) for Phase 1-2, Rust runner
  (conary-test) for Phase 3. Different behaviors, code paths.
- **Shallow logging** -- stdout/stderr concatenated per test, no per-step
  breakdown, no structured events, no conary tracing data.
- **No error taxonomy** -- free-form error strings. Can't distinguish
  infrastructure failures from test assertions from config problems.

## Design Decisions

| Area | Decision |
|------|----------|
| Python runner | Retire entirely, port all 76 tests to TOML manifests |
| Deployment trust | Full trust -- MCP can deploy, rebuild, restart, publish |
| Persistence | Remi is single source of truth, Forge is stateless |
| Log granularity | Per-step + structured events + conary tracing spans |
| Migration strategy | Big bang -- all 76 tests converted, delete Python runner |
| DX persona | Both human and agent, equally |
| Build order | Bottom-up: Remi API -> stateless executor -> manifests -> DX |

## Architecture Overview

```
Developer / Agent
     |
     |  MCP / CLI
     v
[conary-test on Forge]  ----streams results---->  [Remi :8082 admin API]
     |                                                   |
     | runs containers (Podman)                          | stores in SQLite
     | runs VMs (QEMU)                                   | serves via MCP
     | captures per-step logs                            | serves via HTTP
     | extracts tracing events                           |
     |                                                   |
     | deploy tools:                                [remi-admin MCP]
     |   deploy_source                                   |
     |   rebuild_binary                            test_list_runs
     |   restart_service                           test_get_run
     |   build_fixtures                            test_get_test
     |   publish_fixtures                          test_get_logs
     |   build_boot_image                          test_health
```

Forge is a stateless executor. It runs tests, captures logs, and streams
everything to Remi. Remi stores, indexes, and serves the data. Both Forge's
conary-test MCP and Remi's remi-admin MCP expose query tools -- Forge proxies
to Remi for any data queries.

## Layer 1: Remi Test Data API

### Database Schema

New tables in Remi's SQLite (alongside existing admin tables):

**test_runs:**
- `id` INTEGER PRIMARY KEY
- `suite` TEXT NOT NULL
- `distro` TEXT NOT NULL
- `phase` INTEGER NOT NULL
- `status` TEXT NOT NULL (pending, running, completed, cancelled, failed)
- `started_at` TEXT (ISO 8601)
- `completed_at` TEXT
- `triggered_by` TEXT (mcp-agent, ci, manual, cli)
- `source_commit` TEXT (git SHA, if known)
- `total` INTEGER DEFAULT 0
- `passed` INTEGER DEFAULT 0
- `failed` INTEGER DEFAULT 0
- `skipped` INTEGER DEFAULT 0

**test_results:**
- `id` INTEGER PRIMARY KEY
- `run_id` INTEGER NOT NULL REFERENCES test_runs(id)
- `test_id` TEXT NOT NULL (e.g., "T142")
- `name` TEXT NOT NULL
- `status` TEXT NOT NULL (passed, failed, skipped)
- `duration_ms` INTEGER
- `message` TEXT (failure message, if any)
- `attempt` INTEGER DEFAULT 1

**test_steps:**
- `id` INTEGER PRIMARY KEY
- `result_id` INTEGER NOT NULL REFERENCES test_results(id)
- `step_index` INTEGER NOT NULL
- `step_type` TEXT NOT NULL (conary, run, file_exists, file_not_exists, qemu_boot)
- `command` TEXT
- `exit_code` INTEGER
- `duration_ms` INTEGER

**test_logs:**
- `id` INTEGER PRIMARY KEY
- `step_id` INTEGER NOT NULL REFERENCES test_steps(id)
- `stream` TEXT NOT NULL (stdout, stderr, trace)
- `content` TEXT NOT NULL (ANSI-stripped)
- `raw_content` TEXT (with ANSI codes)

**test_events:**
- `id` INTEGER PRIMARY KEY
- `step_id` INTEGER NOT NULL REFERENCES test_steps(id)
- `event_type` TEXT NOT NULL (PackageInstalled, RepoSynced, AssertionFailed,
  DependencyResolved, FileDeployed, ScriptletExecuted, ...)
- `payload` TEXT (JSON)
- `timestamp` TEXT (ISO 8601)

### Admin API Endpoints

On Remi `:8082`, bearer auth required:

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/v1/admin/test-runs` | Create a new run |
| PUT | `/v1/admin/test-runs/{id}` | Update run status/counts |
| POST | `/v1/admin/test-runs/{id}/results` | Push a test result with steps/logs/events |
| GET | `/v1/admin/test-runs` | List runs (cursor pagination, filter by suite/distro/status) |
| GET | `/v1/admin/test-runs/{id}` | Get full run with results summary |
| GET | `/v1/admin/test-runs/{id}/tests/{test_id}` | Get test with steps and logs |
| GET | `/v1/admin/test-runs/{id}/tests/{test_id}/logs` | Logs only (filter by stream, step) |
| GET | `/v1/admin/test-runs/latest` | Most recent run per suite |
| GET | `/v1/admin/test-health` | Aggregate pass rates and trends |

### MCP Tools on remi-admin

New tools added to the existing remi-admin MCP server:

- `test_list_runs(limit, suite?, distro?, status?)` -- paginated run list
- `test_get_run(run_id)` -- full run details with result summaries
- `test_get_test(run_id, test_id)` -- single test with all steps and logs
- `test_get_logs(run_id, test_id, stream?, step_index?)` -- filtered logs
- `test_health()` -- aggregate: pass rates per suite, recent failures, trends

### Key Design Points

- Forge pushes results **per-test** as they complete, not per-run. Partial
  results are visible immediately during long runs.
- Logs are ANSI-stripped for `content`, raw preserved in `raw_content`.
- Events are structured JSON extracted by the test runner before pushing.
- Cursor-based pagination consistent with existing admin API.
- `triggered_by` field tracks provenance (which agent/CI/human started the run).

## Layer 2: Stateless Executor

### conary-test State Removal

Remove `DashMap<RunId, RunState>` in-memory store. Replace with Remi-backed
flow:

1. `start_run` -> POST to Remi `/v1/admin/test-runs`, get `run_id`
2. Per-test completion -> POST result + steps + logs to Remi
3. Run completion -> PUT run status to Remi
4. `get_run`, `get_test`, `get_test_logs` -> proxy to Remi and return response

conary-test still handles: container lifecycle, test execution, log capture,
event extraction. It just doesn't store anything locally.

### Per-Step Log Capture

Replace concatenated stdout/stderr with per-step capture:

- Each `StepAction` execution records its own stdout, stderr, exit code,
  duration independently
- After each step, build `TestStep` + `TestLog` structs
- For conary command steps, parse tracing output into `TestEvent` records

### Conary Tracing Integration

conary uses the `tracing` crate. Add support for structured trace output:

- New env var `CONARY_LOG_FORMAT=json` switches the tracing subscriber to
  JSON-formatted output on stderr
- When conary-test runs a conary command, it sets this env var
- Parse the JSON spans from stderr into `TestEvent` records:
  - `PackageInstalled { name, version, duration_ms }`
  - `RepoSynced { repo, packages_count }`
  - `DependencyResolved { package, deps_count }`
  - `FileDeployed { path, hash }`
  - etc.
- These events provide deep visibility into what conary did during each step

### Deployment MCP Tools

New tools on conary-test's MCP server:

| Tool | Purpose |
|------|---------|
| `deploy_source(git_ref?)` | Sync source from git ref (or current), rebuild conary + conary-test |
| `rebuild_binary(crate?)` | Run cargo build for specified crate (default: both) |
| `restart_service()` | Restart conary-test systemd user service, return health check |
| `build_fixtures(groups?)` | Run build scripts (all, corrupted, malicious, deps, boot) |
| `publish_fixtures()` | Push fixtures to Remi via admin API |
| `deploy_status()` | Current binary version, last deploy, service health, uptime |
| `build_boot_image(version?)` | Trigger bootstrap image build on Remi, return progress |

### Deploy Workflow

The entire deploy-test cycle becomes:

```
deploy_source(git_ref="main")   # sync + cargo build on Forge
restart_service()                # pick up new binary
build_fixtures(groups="all")     # rebuild test CCS packages
publish_fixtures()               # push to Remi
start_run(suite, distro, phase)  # run tests, stream to Remi
test_get_run(run_id)             # query results from Remi
```

No SSH, no rsync, no systemctl. All via MCP.

## Layer 3: Phase 1-2 Manifest Migration

### Scope

Port all 76 Python tests (T01-T37 Phase 1, T38-T76 Phase 2) to TOML manifests.
Existing Phase 1-2 manifests may already exist as stubs from earlier work --
audit and complete them.

### Manifest Organization

Follows existing Phase 3 pattern:

```
tests/integration/remi/manifests/
  phase1-core.toml           # T01-T10
  phase1-advanced.toml       # T11-T37
  phase2-group-a.toml        # T38-T50
  phase2-group-b.toml        # T51-T57
  phase2-group-c.toml        # T58-T61
  phase2-group-d.toml        # T62-T66
  phase2-group-e.toml        # T67-T71
  phase2-group-f.toml        # T72-T76
```

### Conversion Patterns

| Python | TOML |
|--------|------|
| `conary(cfg, "install", pkg)` | `conary = "install ${pkg} ..."` |
| `assert_contains(stdout, "x")` | `stdout_contains = "x"` |
| `run_cmd(["cmd", ...])` | `run = "cmd ..."` |
| `suite.checkpoint()` | `fatal = true` |
| `suite.failed_since()` | `depends_on = ["TXX"]` |
| `no_db=True` | `run = "${CONARY_BIN} system ..."` (bypass auto --db-path) |

### Work Items

1. Audit existing manifests against Python test functions for parity
2. Fill gaps (missing assertions, cleanup steps, edge cases)
3. Validate on Forge -- run via conary-test, compare against Python runner
4. Update CI workflows to use `conary-test run` instead of `run.sh`
5. Delete `runner/test_runner.py`, `run.sh`, Python-only CI steps

## Layer 4: Developer Experience

### Error Taxonomy

Every error response (API and MCP) includes structured fields:

```json
{
  "error": "test_timeout",
  "category": "infrastructure",
  "message": "Test T142 timed out after 300s",
  "transient": true,
  "hint": "Container may be overloaded. Try reducing concurrency.",
  "details": { "test_id": "T142", "timeout_seconds": 300 }
}
```

Categories:
- `infrastructure` -- container/network/OOM (usually transient)
- `assertion` -- test logic failure (not transient)
- `config` -- bad manifest/missing distro (not transient)
- `deployment` -- build failure/service down (not transient)

The `transient` field tells agents whether to retry automatically.

### Config Hot-Reload

conary-test watches `manifests/` directory for changes. When a TOML file
changes:
- Re-parse and validate
- Update in-memory suite registry
- Next `start_run` picks up new manifest
- `reload_manifests` MCP tool for explicit trigger

### Image Lifecycle

- `prune_images(keep?)` MCP tool -- remove images older than N days or not
  referenced by recent runs (default: keep last 3 per distro)
- `image_info(image)` MCP tool -- tag, build date, conary version, size
- Image tags include conary binary hash for version tracking
- Auto-prune on configurable schedule

### CLI / MCP Parity

Every MCP tool has a corresponding CLI subcommand:

```
conary-test run --suite phase1-core --distro fedora43
conary-test deploy --source . --rebuild --restart
conary-test fixtures build --all
conary-test fixtures publish
conary-test logs T142 --run latest --step 3
conary-test health
conary-test images prune --keep 3
```

CLI outputs human-friendly tables and colors. MCP/API returns structured JSON.
Same underlying implementation -- CLI and MCP are thin wrappers over shared
service functions.

### ANSI Handling

- Log capture strips ANSI from `content` field stored in Remi
- Raw output preserved in `raw_content` for terminal replay
- `get_logs` endpoint accepts `raw=true` parameter

## Implementation Order

1. **Layer 1: Remi Test Data API** -- schema, endpoints, MCP tools
2. **Layer 2: Stateless Executor** -- stream to Remi, per-step logs, tracing,
   deployment MCP tools
3. **Layer 3: Phase 1-2 Migration** -- audit manifests, fill gaps, validate,
   delete Python runner
4. **Layer 4: DX Polish** -- error taxonomy, config reload, image lifecycle,
   CLI parity

Each layer is independently deployable and testable.
