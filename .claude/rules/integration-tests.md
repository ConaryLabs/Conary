# Integration Test Infrastructure

Podman-based integration tests that verify conary works end-to-end on real Linux
distros (Fedora 43, Ubuntu Noble, Arch). Rust test engine (conary-test) with
TOML manifests, bollard container management, JSON results.

## Rust Test Engine (conary-test)

Declarative test engine using TOML manifests and bollard for container management.

### Quick Reference

```bash
cargo run -p conary-test -- run --distro fedora43 --phase 1
cargo run -p conary-test -- run --suite phase1-core --distro fedora43 --phase 1
cargo run -p conary-test -- serve --port 9090
cargo run -p conary-test -- list
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
```

## File Layout

```
tests/integration/remi/
  config.toml                    # Single source of truth (endpoints, distros, fixtures)
  manifests/                     # TOML test manifests (phase1-*, phase2-*, phase3-*)
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

## Test Phases

**Phase 1 (T01-T37):** Core Remi integration -- always runs
- Health check, repo ops, install/remove/update, adopt, pin, deps, generations

**Phase 2 (T38-T76):** Deep E2E
- Group A (T38-T50): Deep install flow with fixture packages
- Group B (T51-T57): Generation lifecycle (build/switch/rollback/gc)
- Group C (T58-T61): Bootstrap pipeline (dry-run, stage 0)
- Group D (T62-T66): Recipe & build (cook, PKGBUILD, hermetic)
- Group E (T67-T71): Remi client (sparse index, chunk fetch, OCI)
- Group F (T72-T76): Self-update (channel get/set/reset, check, mock server)

**Phase 3:** Adversarial tests (groups G-N)

## CI Workflows

| Workflow | File | Trigger | What |
|----------|------|---------|------|
| CI | `.forgejo/workflows/ci.yaml` | Push to main | Build + test + clippy + smoke |
| Integration | `.forgejo/workflows/integration.yaml` | Push to main | 3-distro Phase 1 matrix |
| E2E | `.forgejo/workflows/e2e.yaml` | Daily 06:00 UTC + manual | 3-distro Phase 1+2+3 |
| Remi Health | `.forgejo/workflows/remi-health.yaml` | Every 6 hours | Endpoint verification |

## Adding a New Test

1. Create or edit a TOML manifest in `tests/integration/remi/manifests/`
2. Define test steps using the manifest schema (run, assert, mock_server, etc.)
3. Run with `cargo run -p conary-test -- run --suite <manifest> --distro <distro> --phase <N>`

## Adding a New Distro

1. Create `containers/Containerfile.<distro-name>`
2. Add `[distros.<distro-name>]` section in `config.toml`
3. Add distro name to the `remove_default_repos` list in `[setup]`
4. Add to CI matrix in `.forgejo/workflows/integration.yaml` and `e2e.yaml`

## Gotchas

- Container runs as root (required for system operations)
- Test fixture CCS packages must be published to Remi before Phase 2 tests work
  (`scripts/publish-test-fixtures.sh`)
- The `system init` command adds default repos; tests remove them via
  `config.toml [setup] remove_default_repos` to avoid slow syncs
- Mock server tests require python3 inside the container (installed in all Containerfiles)
