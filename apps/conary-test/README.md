# conary-test

Declarative test infrastructure for Conary integration testing. Replaces the
Python test runner with a Rust engine that reads TOML test manifests and
manages containers via bollard. The Remi result-streaming and WAL path is
retained for the planned run-path wiring; local CLI runs currently keep their
results in local JSON files.

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
| `src/bootstrap.rs` | Local developer prerequisite and smoke-readiness inspection |
| `src/suite_inventory.rs` | Suite manifest inventory for agent-facing read-only resources |
| `src/report/` | JSON output and run-event streaming |
| `src/remi_client.rs` | Remi test-data API client and retained result-streaming client |
| `src/wal.rs` | Retained SQLite result buffer for the planned Remi streaming path |
| `src/cli.rs` | Binary entrypoint |

## CLI Usage

```bash
# Inspect local developer prerequisites and smoke-readiness state
cargo run -p conary-test -- bootstrap check --json

# Preview the local developer smoke proof loop without executing it
cargo run -p conary-test -- bootstrap smoke --dry-run --json

# Run the local developer smoke proof loop
cargo run -p conary-test -- bootstrap smoke --json

# Run Phase 1 tests on Fedora 44 from the repo root
cargo run -p conary-test -- run --distro fedora44 --phase 1

# Run a specific suite on all configured distros
cargo run -p conary-test -- run --suite phase1-core --all-distros --phase 1

# List available test suites
cargo run -p conary-test -- list

# Build container images
cargo run -p conary-test -- images build --distro fedora44

# List built container images
cargo run -p conary-test -- images list

```

`bootstrap smoke` is a local developer proof loop for a checkout. It may build
images, start containers, and write conary-test result files through the normal
test runner path. It is not package publishing and does not require cloud
credentials.

## Test Manifest Format

Tests are defined in TOML manifests under
`apps/conary/tests/integration/remi/manifests/`.
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
apps/conary/tests/integration/remi/manifests/
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
  phase3-group-o-generation-export.toml  # Generation export QEMU tests
  phase3-composefs-modernization.toml    # Composefs atomic generation QEMU checks
  phase3-active-generation-handoff.toml  # Selected-generation native authority handoff proof
  phase4-group-a.toml       # T160-T176 (Config/Distro/Canonical/Groups/Registry)
  phase4-group-b.toml       # T177-T195 (Label/Model/Collection/Derive)
  phase4-group-c.toml       # T196-T220 (CCS ops / query / repo management)
  phase4-group-d.toml       # T221-T255 (Provenance/Capability/Trust/System/Federation/Automation)
  phase4-group-e.toml       # T256-T277 (Cross-distro compatibility overlay: distro policy/replatform/takeover)
  phase4-native-pm-parity.toml  # Three-distro native package-manager parity proof
  phase4-security-advisory-pipeline.toml  # Trusted advisory ingestion and security update proof
```

## Network surfaces

`conary-test` is a local CLI and test engine. It does not bind an HTTP socket or
provide an MCP server. Remi owns the live MCP and test-data service surfaces;
the harness uses `REMI_ADMIN_ENDPOINT` and `REMI_ADMIN_TOKEN` for health, log
queries, and fixture publication. The retained result-streaming/WAL path is
currently unconstructed for local runs; issue #354 tracks its wiring.

Forge-hosted validation and conary-test deployment are decommissioned. Use
`scripts/local-qemu-validation.sh` on a local KVM-capable development machine
for temporary QEMU release evidence.

`conary-test deploy status --json` reports the invoking binary's local build
metadata, checkout state, and managed-rollout provenance. It does not query a
remote runner or a local HTTP service. The `deploy` namespace is read-only and
has no source, rebuild, restart, or rollout execution command.
`conary-test health --json` emits one normalized envelope with local
`deploy_status`, optional `remi`, and optional `reason`.

## Configuration

Environment variables override values from
`apps/conary/tests/integration/remi/config.toml`:

| Variable | Purpose |
|----------|---------|
| `CONARY_TEST_CONFIG` | Path to global config TOML |
| `CONARY_TEST_MANIFESTS` | Path to manifest directory |
| `REMI_ENDPOINT` | Remi server endpoint URL |
| `REMI_ADMIN_ENDPOINT` | Remi admin REST base URL for health, log queries, fixture publication, and the planned result-streaming path |
| `REMI_ADMIN_TOKEN` | Bearer token for the Remi admin API |
| `DB_PATH` | SQLite database path |
| `CONARY_BIN` | Path to conary binary |
| `RESULTS_DIR` | Directory for JSON result output |
| `DISTRO` | Target distro name |

## State Management

The run engine owns suite execution and JSON result generation in the invoking
CLI process. The Remi streaming/WAL delivery path is retained but currently
has no production constructor, so local runs do not stream results or populate
the WAL. Wiring that path is tracked in issue #354.
