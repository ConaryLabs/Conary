---
last_updated: 2026-03-14
revision: 2
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
| Deployment trust | Full trust with bearer auth -- MCP can deploy, rebuild, restart, publish |
| Persistence | Remi is single source of truth, Forge has local WAL for resilience |
| Log granularity | Per-step logs + structured events (tracing integration deferred to Layer 5) |
| Migration strategy | Incremental validation -- audit manifests, parallel-run, then delete Python |
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

**Separate database file:** Test data lives in `/conary/test-data.db`, NOT in
the main conary package database. This keeps test infrastructure completely
independent from `conary-core`'s schema (currently v51 with 50+ tables). The
Remi server opens this database separately via a new `test_db::init()` function
with its own migration path. The main package DB is never touched.

New tables in `/conary/test-data.db`:

**test_runs:**
- `id` INTEGER PRIMARY KEY
- `suite` TEXT NOT NULL
- `distro` TEXT NOT NULL
- `phase` INTEGER NOT NULL
- `status` TEXT NOT NULL (pending, running, completed, cancelled, failed)
  - `completed` = all tests executed (some may have failed)
  - `failed` = infrastructure failure prevented the run from finishing
  - Prerequisite: add `RunStatus::Failed` variant to `suite.rs` enum
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
- **Cursor pagination:** `GET /v1/admin/test-runs?cursor=<run_id>&limit=20`.
  Cursor is the `id` field. Sort order: descending (newest first). Response:
  `{ "runs": [...], "next_cursor": <id|null> }`.
- **Log retention:** `raw_content` stored for 30 days, `content` for 90 days,
  run metadata kept indefinitely. `test_gc(older_than_days?)` MCP tool and
  `DELETE /v1/admin/test-runs/gc` endpoint for cleanup. `raw_content` is
  optional — only stored when test manifest sets `capture_raw = true` or
  via `?raw=true` query param on the push endpoint.
- **Storage estimate:** ~76 tests x ~5 steps x ~2KB logs = ~760KB per run.
  At 3 runs/day across 3 distros, ~2.3MB/day, ~70MB/month. Manageable.

## Layer 2: Stateless Executor

### conary-test State Removal

Remove `DashMap<RunId, RunState>` in-memory store. Replace with Remi-backed
flow:

1. `start_run` -> POST to Remi `/v1/admin/test-runs`, get `run_id`
2. Per-test completion -> POST result + steps + logs to Remi
3. Run completion -> PUT run status to Remi
4. `get_run`, `get_test`, `get_test_logs` -> proxy to Remi and return response

conary-test still handles: container lifecycle, test execution, log capture,
event extraction. It stores nothing permanently, but uses a local write-ahead
log for resilience.

### Remi Unreachability (Local WAL)

When a POST to Remi fails (network blip, Remi restarting, SQLite locked):

1. Buffer the result to a local SQLite WAL at `/tmp/conary-test-wal.db`
2. Background reconciliation loop retries buffered results every 30 seconds
   with exponential backoff (30s, 60s, 120s, max 5 min)
3. If Remi is down for the entire run, all results survive locally
4. `flush_pending` MCP tool for explicit retry
5. `pending_count` field in `deploy_status()` response shows buffered items
6. After successful flush, local WAL rows are deleted

This ensures no data loss even if Remi is unreachable. The WAL is a thin
SQLite file with the same schema as Remi's test tables — reconciliation is
a simple INSERT-or-UPDATE replay.

### Per-Step Log Capture

Replace concatenated stdout/stderr with per-step capture:

- Each `StepAction` execution records its own stdout, stderr, exit code,
  duration independently
- After each step, build `TestStep` + `TestLog` structs
- For conary command steps, parse tracing output into `TestEvent` records

### Conary Tracing Integration (Deferred — Layer 5)

conary uses the `tracing` crate. A future enhancement will add structured
trace output (`CONARY_LOG_FORMAT=json`) to capture spans like
`PackageInstalled`, `RepoSynced`, `DependencyResolved` as `TestEvent` records.

This is deferred because it requires modifying conary-core's logging
subscriber (cross-crate change) and adds parsing complexity. Layer 2 works
with per-step stdout/stderr capture, which already provides good debugging
data. The `test_events` table schema is included in Layer 1 so the storage
is ready when tracing integration lands.

Per-step capture already exists partially in `StepResult` (`executor.rs:32-42`)
with stdout, stderr, exit_code, and duration per step.

### Authentication for conary-test

Before adding deployment tools, conary-test's MCP/HTTP server gets bearer
token auth mirroring Remi's external admin router pattern. The token is
configured via `--token` CLI flag or `CONARY_TEST_TOKEN` env var (already
partially implemented in `server/auth.rs` and `server/routes.rs`).

Tool scopes:
- `test:read` — list_runs, get_run, get_test, get_logs, health (query tools)
- `test:write` — start_run, cancel_run, rerun_test, build_image (test ops)
- `deploy:*` — deploy_source, rebuild_binary, restart_service, build_fixtures,
  publish_fixtures, build_boot_image (deployment ops)

All scopes granted by default with a single token (matching "full trust"
decision). Scope enforcement allows future restriction if needed.

`deploy_source(git_ref?)` validates that the ref resolves to a commit in the
configured repository remote, not an arbitrary URL.

### Deployment MCP Tools

New tools on conary-test's MCP server (require `deploy:*` scope):

| Tool | Purpose |
|------|---------|
| `deploy_source(git_ref?)` | Sync source from git ref (or current), rebuild conary + conary-test |
| `rebuild_binary(crate?)` | Run cargo build for specified crate (default: both) |
| `restart_service()` | Restart conary-test systemd user service, return health check |
| `build_fixtures(groups?)` | Run build scripts (all, corrupted, malicious, deps, boot) |
| `publish_fixtures()` | Push fixtures to Remi via admin API |
| `deploy_status()` | Current binary version, last deploy, service health, uptime |
| `build_boot_image(version?)` | POST to Remi admin API `/v1/admin/bootstrap-image`, which runs the build on Remi (12 cores, KVM). Returns a job ID; poll via `deploy_status()` or Remi's `ci_get_run`. |

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
3. **Parallel validation** -- for each manifest, run both Python runner and
   conary-test against the same container image, diff the results. Fix any
   behavioral differences. This catches subtle issues like assertion order,
   cleanup timing, or variable expansion differences.
4. Update CI workflows to use `conary-test run` instead of `run.sh`
5. Delete `runner/test_runner.py`, `run.sh`, Python-only CI steps

Only delete the Python runner after all 8 manifests produce identical results
in parallel validation.

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

### Config Reload

Manifests are loaded at `start_run` time (current behavior — no change).
A `reload_manifests` MCP tool and CLI command forces a re-parse of all
manifest files without restarting the server. No filesystem watching —
manifest changes are infrequent and always intentional, so explicit reload
avoids race conditions and inotify complexity.

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
conary-test deploy source --ref main
conary-test deploy rebuild
conary-test deploy restart
conary-test deploy status
conary-test fixtures build --all
conary-test fixtures publish
conary-test logs T142 --run latest --step 3
conary-test health
conary-test images prune --keep 3
conary-test manifests reload
```

CLI subcommand structure matches MCP tool granularity (each MCP tool = one
CLI subcommand). The combined `deploy source --rebuild --restart` convenience
form is implemented as orchestration over the individual service functions.

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
