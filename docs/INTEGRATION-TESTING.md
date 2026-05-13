---
last_updated: 2026-05-13
revision: 17
summary: Record composefs modernization validation and the QEMU fixture refresh gap
---

# Integration Testing

Conary uses Podman containers to run integration tests on real Linux distributions. Tests exercise the full install/remove/update/adopt/generation lifecycle against a live Remi server.

Run these commands from the repository root, which is now a virtual Cargo
workspace. The product entrypoints live under `apps/`, with the shared package
domain in `crates/conary-core`.

## Prerequisites

- **Podman** (rootless works, but tests run as root inside containers)
- **Network access** to `remi.conary.io` (Remi server)
- A built conary binary (`cargo build -p conary`)
- The conary-test app crate (`cargo build -p conary-test`)

## Running Tests

```bash
# Run Phase 1 core tests on Fedora 44
cargo run -p conary-test -- run --suite phase1-core --distro fedora44 --phase 1

# Run all Phase 1 tests
cargo run -p conary-test -- run --suite phase1-core --distro fedora44 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro fedora44 --phase 1

# Run Phase 2 (deep E2E) tests
cargo run -p conary-test -- run --suite phase2-group-a --distro fedora44 --phase 2

# Run generation artifact export QEMU validation
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3

# Run focused composefs atomic modernization QEMU validation
cargo run -p conary-test -- run --suite phase3-composefs-modernization --distro fedora44 --phase 3

# Run all tests for a phase
cargo run -p conary-test -- run --distro fedora44 --phase 1

# List available suites
cargo run -p conary-test -- list
```

The generation artifact export QEMU suite is the operational proof for both
the 2026-04-22 generation-export unification slice and the later
self-contained installed-runtime export slice. The historical Fedora 43 run on
2026-04-30 covered `TGE01` and `TGE02`. The active Fedora 44 suite now covers:

- `TGE01`: metadata-only installed generations fail closed before artifact publication
- `TGE03`: CAS-backed installed generations fail closed when an included CAS object is missing
- `TGE04`: full CAS-backed installed runtime generation exports to qcow2 and boots under UEFI
- `TGE02`: bootstrap-run generation artifact exports to qcow2 and boots under UEFI

Keep this suite in the Phase 3 rotation for regressions in generation artifact
export, QEMU fixture copying, scratch-disk handling, CAS integrity checks,
guest SSH access, and exported-image boot. The source QEMU image for Groups N
and O must already include the runtime generation toolchain (`cpio`, `dracut`,
`depmod`, `systemd-repart`, `qemu-img`, FAT/ext4 mkfs tools, and composefs
inspection tools as needed). The manifests intentionally do not install those
helpers through Conary on a partial live root before the system is
generation-owned.

The focused composefs atomic modernization suite covers the stricter runtime
contract added on 2026-05-13:

- `TCM01`: OCI export and generation switching reject a generation artifact
  after `root.erofs` is removed
- `TCM02`: rollback fails before mutating state when no active composefs
  generation exists

`scripts/local-qemu-validation.sh` runs this focused suite before the broader
Group N and Group O gates so fail-closed composefs behavior is recorded even
when a source-image fixture preflight blocks the longer boot/export suites.

## CLI Subcommands

Every MCP tool has a CLI equivalent for human use:

| Command | Purpose |
|---------|---------|
| `conary-test deploy rollout (--unit <name> \| --group <name>) [--ref <git-ref> \| --path <path>]` | Managed Forge deploy flow; trusted default source is a GitHub ref |
| `conary-test run --suite <name> --distro <distro> --phase <N>` | Execute a test suite |
| `conary-test deploy source [--ref <git-ref>]` | Deploy source and rebuild |
| `conary-test deploy restart` | Restart the test service |
| `conary-test deploy status [--port <port>]` | Show running-binary status separately from local checkout state and drift |
| `conary-test fixtures build [--groups all]` | Build test fixture CCS packages |
| `conary-test fixtures publish` | Publish fixtures to Remi |
| `conary-test logs <test-id> [--run <id>] [--step <N>]` | Retrieve test logs |
| `conary-test health [--port <port>]` | Normalized health envelope with `mode`, `deploy_status`, optional `remi`, and optional `reason` |
| `conary-test images prune [--keep <N>]` | Remove old container images |
| `conary-test images info <image>` | Inspect container image |
| `conary-test manifests reload` | Reload TOML manifests without restart |

Add `--json` to any command for machine-readable output.

Remote Forge control-plane validation is temporarily paused while Conary
replaces the old VPS runner with a KVM-capable host. The Forge scripts remain
checked in for the next runner, but they are not active release evidence today.

For the temporary local QEMU release gate, run this on a development machine
with `/dev/kvm`:

```bash
scripts/local-qemu-validation.sh
```

Historical local release evidence is
`target/local-validation/qemu-blocker-fix-20260509-201100`, recorded on
2026-05-09. That run predates the stricter composefs install and activation
contract. It passed Group N (`T150`, `T151`, `T153`, `T154`, `T156`) and
Group O (`TGE01`, `TGE02`, `TGE03`, `TGE04`) with 0 failures and 0 skipped
results, emitted the required boot/export markers, and finished with
`[local-qemu-validation] ok`.

Current composefs modernization evidence from 2026-05-13:

- `cargo run -p conary-test -- list`: passed; includes
  `phase3-composefs-modernization`
- `cargo run -p conary-test -- run --suite phase3-composefs-modernization --distro fedora44 --phase 3`:
  passed `TCM01` and `TCM02`, 2 passed / 0 failed / 0 skipped
- `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`:
  passed `TGE01`, then failed `TGE03`, `TGE04`, and `TGE02` because
  the fixture preflight reports `generation-builder-ready QEMU fixture missing
  cpio`; `minimal-boot-v2` lacks the toolchain now required by the forward
  composefs model, so do not treat the May 9 Group O pass as current release
  evidence until the QEMU source image is refreshed and the suite is rerun

Fast workspace verification from 2026-05-13:

- `cargo fmt --check`: passed
- `cargo test -p conary-core`: passed
- `cargo test -p conary`: passed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed
- `git diff --check`: passed

For supported Forge control-plane validation after a new runner is registered,
prefer:

```bash
bash scripts/forge-smoke.sh
```

That path validates the local `conary-test` service contract (`/v1/health`,
`/v1/deploy/status`, `health --json`, and `deploy status --json`) without
pretending to be a full integration suite.

For managed Forge deployments from an operator workstation, prefer:

```bash
FORGE_HOST=peter@replacement.example ./scripts/deploy-forge.sh --group control_plane --ref main
```

## Validation Modes

- `merge-validation` is the trusted on-merge lane. It now runs the Forge
  control-plane smoke against a freshly started `conary-test` server on a
  dedicated test port before the package-manager smoke and Remi smoke. This
  remote path is paused until a new KVM-capable runner is available; the
  workflow currently runs hosted build/list/Remi smoke checks instead.
- `scheduled-ops` keeps hosted Remi health, audit, and manifest-inventory
  checks active. Forge-backed Phase 1-3 and QEMU jobs are paused rather than
  queued against a missing runner.
- Raw `cargo run -p conary-test -- run ...` from an SSH shell is still useful
  for debugging, but it is no longer the main supported Forge control-plane
  check.

### Available Distros

| Distro | Container | Base |
|--------|-----------|------|
| `fedora44` | `Containerfile.fedora44` | Fedora 44 |
| `ubuntu-26.04` | `Containerfile.ubuntu-26.04` | Ubuntu 26.04 LTS |
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
| B | T51-T57 | Generation lifecycle (build, list, switch, rollback, GC, ready-to-activate takeover) |
| C | T58-T61 | Bootstrap pipeline (dry-run, stage 0) |
| D | T62-T66 | Recipe & build (cook, PKGBUILD convert, hermetic build) |
| E | T67-T71 | Remi client (sparse index, chunk fetch, OCI manifests) |
| F | T72-T76 | Self-update (channel get/set/reset, version check, mock server) |

### Phase 3: Adversarial (Groups G-O)

Adversarial and stress tests.

| Group | Category |
|-------|----------|
| G-M | Container-based adversarial tests |
| N (container) | Container-based adversarial tests |
| N (QEMU) | Kernel and boot QEMU tests |
| O (QEMU) | Generation artifact export QEMU tests |
| Composefs modernization (QEMU) | Focused atomic-generation fail-closed checks |

### Phase 4: Feature Validation (Groups A-E)

Validates the active, user-facing command surface and checks that claimed
features still match the current binary. Where a flow is intentionally
preview-only or not yet implemented, the manifest asserts that it fails
cleanly with an explicit message rather than pretending it is production-ready.

| Group | Tests | Category |
|-------|-------|----------|
| A | T160-T176 | Config, distro, canonical, groups, registry |
| B | T177-T195 | Label, model, collection, derive |
| C | T196-T220 | CCS ops, query, repo management |
| D | T221-T255 | Provenance, capability, trust, system ops, federation, automation |
| E | T230-T251 | Cross-distro compatibility overlay: native package parity, distro policy, replatform, and takeover |

Phase 4 is intentionally mixed:

- Positive-path coverage proves real flows such as tracked-config backup/restore,
  the `conary distro` command family, label mutation, trigger mutation,
  `ccs shell`, `ccs run`, selective CCS component installs, native local
  RPM/DEB/Arch installs, TUF bootstrap with a signed test root, provenance
  diff, pinned-fingerprint federation peers, model-driven replatform apply,
  ready-to-activate takeover, and the cross-distro takeover ownership ladder.
- Preview-only flows are still exercised, but the assertions check for the
  expected explanatory output. Current examples are automation history and
  persisting automation configuration changes.

Group E is intentionally richer than a simple “portability” smoke test. It
covers canonical mapping, distro pinning and mixing behavior, source-policy
replatform planning and apply flows, takeover across distro boundaries, and
native-format package handling on the host distro.

In addition to the container-backed suites, `apps/conary/tests/bootstrap_workflow.rs`
exercises the `conary` binary directly for manifest-run record loading,
`bootstrap verify-convergence`, and `bootstrap diff-seeds` using synthetic
completed-run metadata. Those tests do not replace the container suites, but
they do keep the command contracts green even when a container runtime is not
available.

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

QEMU images are downloaded from `https://remi.conary.io/test-artifacts/` and
cached locally. Plain `conary-test` runs can report QEMU skips when host tools
or remote images are unavailable, but `scripts/local-qemu-validation.sh` treats
any skipped QEMU test result as a failed release gate and separately requires
boot/export markers in the logs. Keep that wrapper pointed only at published or
generated fixtures that must be reproducible on a KVM-capable development host.

Generated-image suites can attach a scratch disk, copy files into or out of a
guest, and then boot a host-local qcow2 produced by an earlier step:

```toml
[[test.step]]
[test.step.qemu_boot]
image = "minimal-boot-v2"
scratch_disk_mb = 65536
local_image_path = "/tmp/conary-generation-export/generated.qcow2"
copy_to_guest = [
  { source = "/tmp/input.txt", dest = "/tmp/input.txt" },
]
copy_from_guest = [
  { source = "/tmp/out.qcow2", dest = "/tmp/conary-generation-export/out.qcow2" },
]
commands = ["true"]
```

When `local_image_path` is set, `image` remains a required logical name but the
engine uses the local path directly instead of downloading a cached Remi
artifact. `scratch_disk_mb` adds a virtio scratch disk for large exports, and
`copy_from_guest.dest` parent directories are created automatically.

## Configuration

All test parameters live in `apps/conary/tests/integration/remi/config.toml`:

```toml
[remi]
endpoint = "https://remi.conary.io"

[paths]
db = "/var/lib/conary/conary.db"
conary_bin = "/usr/bin/conary"
results_dir = "/results"
fixture_dir = "/opt/remi-tests/fixtures"

[distros.fedora44]
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

For admin-backed operations such as result streaming, log queries, and fixture
publishing, also set:

| Variable | Purpose |
|----------|---------|
| `REMI_ADMIN_ENDPOINT` | Base URL for the Remi admin REST API |
| `REMI_ADMIN_TOKEN` | Bearer token for the Remi admin API |

## Results

Test results are written as JSON under
`apps/conary/tests/integration/remi/results/`, using filenames such as
`<distro>-phase<N>.json`:

```json
{
  "distro": "fedora44",
  "endpoint": "https://remi.conary.io",
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

Trusted integration validation now belongs to GitHub Actions, with Forge used
as execution capacity rather than as an independent control plane.

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `merge-validation` | Every push to `main` + manual dispatch | Trusted on-merge smoke validation for `conary`, `remi`, `conaryd`, and `conary-test` |
| `scheduled-ops` | Nightly/scheduled + manual dispatch | Deep validation, health checks, and scheduled operational audits |

`conary-test deploy status` is internal infrastructure state, not a product
release identity. Operators should read it as commit/ref/build provenance for
the harness that is currently running on Forge.

Current JSON semantics:

- `conary-test deploy status --json` separates running binary state from local
  checkout branch/commit and marks degraded output explicitly when the local
  service is unreachable.
- `conary-test health --json` always returns valid JSON. The top-level shape is
  normalized to `mode`, `deploy_status`, optional `remi`, and optional
  `reason`.

## Adding Tests

1. Create or edit a TOML manifest in `apps/conary/tests/integration/remi/manifests/`
2. Define test steps using the manifest schema (run, assert, mock_server, etc.)
3. For supported Forge control-plane validation, run `bash scripts/forge-smoke.sh`
4. For deeper manual debugging, run `cargo run -p conary-test -- run --suite <manifest> --distro <distro> --phase <N>`

## Adding Distros

1. Create `apps/conary/tests/integration/remi/containers/Containerfile.<name>`
2. Add `[distros.<name>]` section to `config.toml`
3. Add to CI workflow matrices

## Troubleshooting

**"cannot start a transaction within a transaction"** during repo sync:
Fixed in commit 942c4b2. If seen again, check that `batch_insert()` doesn't nest transactions.

**"unexpected argument '--db-path'":**
The subcommand doesn't accept `--db-path`. Check `apps/conary/src/cli/` to see which subcommands have `DbArgs`.

**Phase 2 tests fail with "package not found":**
Test fixture packages need to be published to Remi first:
```bash
./scripts/publish-test-fixtures.sh
```
