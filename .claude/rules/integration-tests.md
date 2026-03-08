# Integration Test Infrastructure

Podman-based integration tests that verify conary works end-to-end on real Linux
distros (Fedora 43, Ubuntu Noble, Arch). Python 3.11+ test runner, TOML config,
JSON results.

## Quick Reference

```bash
# Run locally on Forge (SSH: ssh peter@forge.conarylabs.com)
./tests/integration/remi/run.sh --build --distro fedora43

# Run Phase 2 (deep E2E) tests too
./tests/integration/remi/run.sh --build --distro fedora43 --phase2

# All distros
for d in fedora43 ubuntu-noble arch; do
  ./tests/integration/remi/run.sh --build --distro $d
done

# Use a pre-built binary
./tests/integration/remi/run.sh --binary /path/to/conary --distro fedora43

# Use a native package
./tests/integration/remi/run.sh --package packaging/rpm/output/conary.rpm --distro fedora43
```

## File Layout

```
tests/integration/remi/
  config.toml                    # Single source of truth (endpoints, distros, fixtures)
  run.sh                         # Podman orchestrator (builds image, runs container)
  runner/test_runner.py           # Python test runner (all test logic)
  containers/
    Containerfile.fedora43        # Fedora 43 test image
    Containerfile.ubuntu-noble    # Ubuntu 24.04 test image
    Containerfile.arch            # Arch Linux test image
  results/                       # JSON output (gitignored)

tests/fixtures/
  conary-test-fixture/            # CCS fixture packages (v1, v2) for Phase 2
    build-all.sh                  # Builds fixture CCS packages
    v1/ccs.toml, v1/stage/        # v1 fixture files
    v2/ccs.toml, v2/stage/        # v2 fixture files
  recipes/simple-hello/           # Recipe fixture for cook tests

scripts/
  publish-test-fixtures.sh        # Publishes fixtures to Remi for all distros
```

## Configuration: config.toml

All test parameters live in `tests/integration/remi/config.toml`. To change endpoints,
test packages, distro mappings, or fixture checksums, edit this file.

**Environment variable overrides:**
- `REMI_ENDPOINT` -- override `[remi] endpoint`
- `DB_PATH` -- override `[paths] db`
- `CONARY_BIN` -- override `[paths] conary_bin`
- `RESULTS_DIR` -- override `[paths] results_dir`
- `DISTRO` -- select which `[distros.*]` section to use

## Test Runner: test_runner.py

Python 3.11+ stdlib-only (no pip dependencies). Key components:

- `Config.load(toml_path)` -- loads config with env overrides
- `TestSuite` -- tracks results, supports checkpoints, skip groups, fatal stops
- `conary(cfg, *args)` -- runs conary binary with `--db-path` appended
  - Use `no_db=True` for subcommands that don't accept `--db-path`
    (e.g., `system generation list/gc/switch/rollback/info`)
- `run_cmd(args)` -- runs arbitrary commands, strips ANSI escape codes
- Assertions: `assert_contains`, `assert_not_contains`, `assert_file_exists`,
  `assert_file_checksum`, `assert_file_not_exists`

## Test Phases

**Phase 1 (T01-T37):** Core Remi integration -- always runs
- Health check, repo ops, install/remove/update, adopt, pin, deps, generations

**Phase 2 (T38-T76):** Deep E2E -- runs with `--phase2` flag
- Group A (T38-T50): Deep install flow with fixture packages
- Group B (T51-T57): Generation lifecycle (build/switch/rollback/gc)
- Group C (T58-T61): Bootstrap pipeline (dry-run, stage 0)
- Group D (T62-T66): Recipe & build (cook, PKGBUILD, hermetic)
- Group E (T67-T71): Remi client (sparse index, chunk fetch, OCI)
- Group F (T72-T76): Self-update (channel get/set/reset, check, mock server)

## CI Workflows

| Workflow | File | Trigger | What |
|----------|------|---------|------|
| CI | `.forgejo/workflows/ci.yaml` | Push to main | Build + test + clippy + smoke |
| Integration | `.forgejo/workflows/integration.yaml` | Push to main | 3-distro Phase 1 matrix |
| E2E | `.forgejo/workflows/e2e.yaml` | Daily 06:00 UTC + manual | 3-distro Phase 1+2 |
| Remi Health | `.forgejo/workflows/remi-health.yaml` | Every 6 hours | Endpoint verification |

## Adding a New Test

1. Add test function in `test_runner.py` inside the appropriate `run_phase*` or `run_group_*`
2. Use `suite.run_test("TXX", "test_name", test_fn, timeout=N)` to register it
3. Use `conary(cfg, ...)` to run conary commands (auto-appends `--db-path`)
4. Use assertion helpers for validation
5. If the test depends on a prior test, use `suite.checkpoint()` and `suite.failed_since()`

## Adding a New Distro

1. Create `containers/Containerfile.<distro-name>`
2. Add `[distros.<distro-name>]` section in `config.toml`
3. Add distro name to the `remove_default_repos` list in `[setup]`
4. Add to CI matrix in `.forgejo/workflows/integration.yaml` and `e2e.yaml`

## Gotchas

- `conary()` helper appends `--db-path` AFTER args (subcommand option, not global)
- Generation subcommands (`list`, `gc`, `switch`, `rollback`, `info`) don't accept
  `--db-path` -- use `no_db=True`
- `run_cmd()` strips ANSI escape codes from stdout (conary uses color output)
- Container runs as root (required for system operations)
- Test fixture CCS packages must be published to Remi before Phase 2 tests work
  (`scripts/publish-test-fixtures.sh`)
- The `system init` command adds default repos; tests remove them via
  `config.toml [setup] remove_default_repos` to avoid slow syncs
