---
last_updated: 2026-03-28
revision: 6
summary: Clarify what Phase 4 proves versus which flows remain intentionally preview-only
---

# Integration Testing

Conary uses Podman containers to run integration tests on real Linux distributions. Tests exercise the full install/remove/update/adopt/generation lifecycle against a live Remi server.

## Prerequisites

- **Podman** (rootless works, but tests run as root inside containers)
- **Network access** to `packages.conary.io` (Remi server)
- A built conary binary (`cargo build`)
- The conary-test crate (`cargo build -p conary-test`)

## Running Tests

```bash
# Run Phase 1 core tests on Fedora 43
cargo run -p conary-test -- run --suite phase1-core --distro fedora43 --phase 1

# Run all Phase 1 tests
cargo run -p conary-test -- run --suite phase1-core --distro fedora43 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro fedora43 --phase 1

# Run Phase 2 (deep E2E) tests
cargo run -p conary-test -- run --suite phase2-group-a --distro fedora43 --phase 2

# Run all tests for a phase
cargo run -p conary-test -- run --distro fedora43 --phase 1

# List available suites
cargo run -p conary-test -- list
```

## CLI Subcommands

Every MCP tool has a CLI equivalent for human use:

| Command | Purpose |
|---------|---------|
| `conary-test run --suite <name> --distro <distro> --phase <N>` | Execute a test suite |
| `conary-test deploy source [--ref <git-ref>]` | Deploy source and rebuild |
| `conary-test deploy restart` | Restart the test service |
| `conary-test deploy status` | Show version, uptime, WAL pending |
| `conary-test fixtures build [--groups all]` | Build test fixture CCS packages |
| `conary-test fixtures publish` | Publish fixtures to Remi |
| `conary-test logs <test-id> [--run <id>] [--step <N>]` | Retrieve test logs |
| `conary-test health` | Service health summary |
| `conary-test images prune [--keep <N>]` | Remove old container images |
| `conary-test images info <image>` | Inspect container image |
| `conary-test manifests reload` | Reload TOML manifests without restart |

Add `--json` to any command for machine-readable output.

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

### Phase 2: Deep E2E (T38-T76)

Requires test fixture packages published to Remi.

| Group | Range | Category |
|-------|-------|----------|
| A | T38-T50 | Deep install flow (fixture packages, update, rollback, orphans, pin) |
| B | T51-T57 | Generation lifecycle (build, list, switch, rollback, GC, takeover) |
| C | T58-T61 | Bootstrap pipeline (dry-run, stage 0) |
| D | T62-T66 | Recipe & build (cook, PKGBUILD convert, hermetic build) |
| E | T67-T71 | Remi client (sparse index, chunk fetch, OCI manifests) |
| F | T72-T76 | Self-update (channel get/set/reset, version check, mock server) |

### Phase 3: Adversarial (Groups G-N)

Adversarial and stress tests.

| Group | Category |
|-------|----------|
| G-M | Container-based adversarial tests |
| N (container) | Container-based adversarial tests |
| N (QEMU) | QEMU boot tests |

### Phase 4: Feature Validation (Groups A-E)

Validates the active, user-facing command surface and checks that claimed
features still match the current binary. Where a flow is intentionally
preview-only or not yet implemented, the manifest asserts that it fails
cleanly with an explicit message rather than pretending it is production-ready.

| Group | Tests | Category |
|-------|-------|----------|
| A | T160-T176 | Config, distro, canonical, groups, registry |
| B | T177-T195 | Label, model, collection, derive |
| C | T196-T213 | CCS, bootstrap, cache, query, repo management |
| D | T221-T255 | Provenance, capability, trust, system ops, federation, automation |
| E | T230-T249 | Cross-distro compatibility |

Phase 4 is intentionally mixed:

- Positive-path coverage proves real flows such as tracked-config backup/restore,
  label mutation, trigger mutation, `ccs shell`, `ccs run`, selective CCS
  component installs, native local RPM/DEB/Arch installs, TUF bootstrap with a
  signed test root, provenance diff, and pinned-fingerprint federation peers.
- Preview-only flows are still exercised, but the assertions check for the
  expected explanatory output. Current examples are automation history and
  persisting automation configuration changes.

## QEMU Boot Tests

Tests requiring kernel/boot file deployment use the `qemu_boot` step type:

```toml
[[test.step]]
[test.step.qemu_boot]
image = "minimal-boot-v1"
memory_mb = 2048
timeout_seconds = 240
ssh_port = 2222
commands = ["uname -r", "ls /boot/vmlinuz*"]
expect_output = ["vmlinuz"]
```

QEMU images are downloaded from `https://packages.conary.io/test-artifacts/` and cached locally. Tests gracefully skip when QEMU tools are unavailable.

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

## Error Responses

API and MCP errors include structured fields for programmatic handling:

```json
{
  "error": "test_timeout",
  "category": "infrastructure",
  "message": "Test T142 timed out after 300s",
  "transient": true,
  "hint": "Try reducing concurrency or increasing timeout."
}
```

Categories: `infrastructure` (transient), `assertion` (test logic), `config` (manifest/distro), `deployment` (build/service), `validation` (request).

## Result Persistence

Test results are streamed to Remi's admin API as each test completes. If Remi is unreachable, results are buffered in a local SQLite write-ahead log (`/tmp/conary-test-wal.db`) and retried automatically with exponential backoff.

## CI Integration

Tests run automatically on the Forge server (`forge.conarylabs.com`):

| Workflow | Trigger | Tests |
|----------|---------|-------|
| `ci.yaml` | Every push to main | Build + unit tests + clippy |
| `integration.yaml` | Every push to main | Phase 1, all 3 distros |
| `e2e.yaml` | Daily + manual | Phase 1 + Phase 2 + Phase 3, all 3 distros |
| `remi-health.yaml` | Every 6 hours | Remi endpoint health |

Trigger manually via Forgejo API:
```bash
curl -X POST "http://forge.conarylabs.com:3000/api/v1/repos/peter/Conary/actions/workflows/e2e.yaml/dispatches" \
  -H "Authorization: token $TOKEN" \
  -d '{"ref":"main"}'
```

## Adding Tests

1. Create or edit a TOML manifest in `tests/integration/remi/manifests/`
2. Define test steps using the manifest schema (run, assert, mock_server, etc.)
3. Run with `cargo run -p conary-test -- run --suite <manifest> --distro <distro> --phase <N>`

## Adding Distros

1. Create `tests/integration/remi/containers/Containerfile.<name>`
2. Add `[distros.<name>]` section to `config.toml`
3. Add to CI workflow matrices

## Troubleshooting

**"cannot start a transaction within a transaction"** during repo sync:
Fixed in commit 942c4b2. If seen again, check that `batch_insert()` doesn't nest transactions.

**"unexpected argument '--db-path'":**
The subcommand doesn't accept `--db-path`. Check `src/cli/` to see which subcommands have `DbArgs`.

**Phase 2 tests fail with "package not found":**
Test fixture packages need to be published to Remi first:
```bash
./scripts/publish-test-fixtures.sh
```
