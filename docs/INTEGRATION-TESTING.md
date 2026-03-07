---
last_updated: 2026-03-07
revision: 1
summary: Initial integration test documentation
---

# Integration Testing

Conary uses Podman containers to run integration tests on real Linux distributions. Tests exercise the full install/remove/update/adopt/generation lifecycle against a live Remi server.

## Prerequisites

- **Podman** (rootless works, but tests run as root inside containers)
- **Network access** to `packages.conary.io` (Remi server)
- A built conary binary (`cargo build`)

## Running Tests

```bash
# Build + run Phase 1 tests on Fedora 43
./tests/integration/remi/run.sh --build --distro fedora43

# Run with Phase 2 (deep E2E) tests
./tests/integration/remi/run.sh --build --distro fedora43 --phase2

# Use a pre-built binary
./tests/integration/remi/run.sh --binary target/debug/conary --distro fedora43

# Test a native package
./tests/integration/remi/run.sh --package packaging/rpm/output/conary.rpm --distro fedora43

# Rebuild container from scratch
./tests/integration/remi/run.sh --build --distro fedora43 --no-cache

# Keep results volume after run
./tests/integration/remi/run.sh --build --distro fedora43 --keep
```

### Available Distros

| Distro | Container | Base |
|--------|-----------|------|
| `fedora43` | `Containerfile.fedora43` | Fedora 43 |
| `ubuntu-noble` | `Containerfile.ubuntu-noble` | Ubuntu 24.04 LTS |
| `arch` | `Containerfile.arch` | Arch Linux (rolling) |

## Test Structure

### Phase 1: Core Integration (T01-T37)

Always runs. Tests basic conary operations against a live Remi server:

| Range | Category | Tests |
|-------|----------|-------|
| T01 | Health check | Remi endpoint reachable |
| T02-T04 | Repository | Add, list, sync |
| T05-T06 | Search | Package search |
| T07-T12 | Install/Remove | Install, verify files, list, remove, verify cleanup |
| T13-T17 | Package Info | Version, info, file list, path ownership |
| T18-T19 | Multi-package | Install second package, verify coexistence |
| T20-T21 | Adopt | Adopt system package, check status |
| T22-T23 | Pin | Pin/unpin package |
| T24 | History | Changeset history |
| T25-T27 | Dependencies | Install with deps, verify, multi-package coexist |
| T28-T31 | Dep modes | Satisfy, adopt, takeover, blocklist |
| T32 | Update | Update with adopted packages |
| T33-T37 | Generations | List, GC, info, takeover dry-run, composefs format |

### Phase 2: Deep E2E (T38-T71)

Runs with `--phase2` flag. Requires test fixture packages published to Remi.

| Group | Range | Category |
|-------|-------|----------|
| A | T38-T50 | Deep install flow (fixture packages, update, rollback, orphans, pin) |
| B | T51-T57 | Generation lifecycle (build, list, switch, rollback, GC, takeover) |
| C | T58-T61 | Bootstrap pipeline (dry-run, stage 0) |
| D | T62-T66 | Recipe & build (cook, PKGBUILD convert, hermetic build) |
| E | T67-T71 | Remi client (sparse index, chunk fetch, OCI manifests) |

## Configuration

All test parameters live in `tests/integration/remi/config.toml`:

```toml
[remi]
endpoint = "https://packages.conary.io"

[paths]
db = "/var/lib/conary/conary.db"
conary_bin = "/usr/bin/conary"
results_dir = "/results"
fixture_dir = "/opt/remi-tests/fixtures"

[distros.fedora43]
remi_distro = "fedora"
repo_name = "fedora-remi"
test_package = "which"
test_binary = "/usr/bin/which"
# ... more test packages
```

### Environment Overrides

Override any config value via environment variables:

| Variable | Overrides |
|----------|-----------|
| `REMI_ENDPOINT` | `[remi] endpoint` |
| `DB_PATH` | `[paths] db` |
| `CONARY_BIN` | `[paths] conary_bin` |
| `RESULTS_DIR` | `[paths] results_dir` |
| `DISTRO` | Which `[distros.*]` section to use |

## Results

Test results are written as JSON to `tests/integration/remi/results/<distro>.json`:

```json
{
  "distro": "fedora43",
  "endpoint": "https://packages.conary.io",
  "total": 37,
  "passed": 37,
  "failed": 0,
  "skipped": 0,
  "tests": [
    {"id": "T01", "name": "health_check", "status": "pass", "duration_ms": 206}
  ]
}
```

## CI Integration

Tests run automatically on the Forge server (`forge.conarylabs.com`):

| Workflow | Trigger | Tests |
|----------|---------|-------|
| `ci.yaml` | Every push to main | Build + unit tests + clippy |
| `integration.yaml` | Every push to main | Phase 1, all 3 distros |
| `e2e.yaml` | Daily + manual | Phase 1 + Phase 2, all 3 distros |
| `remi-health.yaml` | Every 6 hours | Remi endpoint health |

Trigger manually via Forgejo API:
```bash
curl -X POST "http://forge.conarylabs.com:3000/api/v1/repos/peter/Conary/actions/workflows/e2e.yaml/dispatches" \
  -H "Authorization: token $TOKEN" \
  -d '{"ref":"main"}'
```

## Adding Tests

1. Edit `tests/integration/remi/runner/test_runner.py`
2. Add your test function inside the appropriate `run_phase*` or `run_group_*` function
3. Register it with `suite.run_test("TXX", "test_name", test_fn, timeout=N)`
4. Use `conary(cfg, ...)` to run conary (auto-appends `--db-path`)
5. Use `no_db=True` for generation subcommands that don't accept `--db-path`
6. Use assertion helpers: `assert_contains`, `assert_file_exists`, `assert_file_checksum`

## Adding Distros

1. Create `tests/integration/remi/containers/Containerfile.<name>`
2. Add `[distros.<name>]` section to `config.toml`
3. Add to CI workflow matrices

## Troubleshooting

**"cannot start a transaction within a transaction"** during repo sync:
Fixed in commit 942c4b2. If seen again, check that `batch_insert()` doesn't nest transactions.

**ANSI escape codes in assertions:**
`run_cmd()` strips ANSI codes automatically. If a new code pattern breaks through, update `_ANSI_RE` in `test_runner.py`.

**"unexpected argument '--db-path'":**
The subcommand doesn't accept `--db-path`. Use `no_db=True` in the `conary()` call.
Check `src/cli/` to see which subcommands have `DbArgs`.

**Phase 2 tests fail with "package not found":**
Test fixture packages need to be published to Remi first:
```bash
./scripts/publish-test-fixtures.sh
```
