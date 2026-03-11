---
last_updated: 2026-03-11
revision: 2
summary: Phase 3 adversarial and production hardening test suite design (post-review)
---

# Phase 3: Adversarial & Production Hardening Test Suite

## Overview

82 new integration tests across 8 groups, built on the `conary-test` Rust engine with TOML manifests. Phase 3 focuses on verifying Conary behaves correctly under adversity — corrupted data, interrupted operations, hostile inputs, real-world package operations, and actual boot scenarios.

Phase 1 (T01-T37) + Phase 2 (T38-T76) total 76 tests, verified complete. Phase 3 begins at T77.

## Goals

- Verify data integrity enforcement at every layer (packages, repos, CAS, DB)
- Confirm graceful recovery from crashes, disk-full, and network failures
- Validate security boundaries (path traversal, symlink attacks, sandbox enforcement)
- Test dependency resolution under complex/adversarial constraint graphs
- Ensure real-world package operations work end-to-end against live repos
- Verify kernel and bootloader operations through actual QEMU boot tests

## Prerequisites

- `conary verify` command exists (`cmd_verify` in `src/commands/system.rs`)
- TUF rollback protection is implemented (`verify_version_increase` in `trust/verify.rs`)
- Bootstrap module supports QEMU orchestration (`bootstrap/orchestrator.rs`)

## Test Groups

### Group G: Data Integrity (T77-T86, 10 tests)

Verify that Conary detects and rejects corrupted data at every layer.

| Test | Description |
|------|-------------|
| T77 | Install CCS with bad SHA-256 in manifest — expect rejection |
| T78 | Install CCS with truncated chunk — expect rejection |
| T79 | Install package with tampered file contents after signing — signature mismatch detected |
| T80 | `repo-sync` with corrupted metadata JSON — sync fails, previous good metadata preserved |
| T81 | Install DEB/RPM with checksum mismatch — expect rejection |
| T82 | Verify CAS store integrity after install — all files match content-address hashes |
| T83 | Corrupt CAS file, run `conary verify` — corruption detected |
| T84 | Install package claiming 1GB file containing 100 bytes — graceful rejection, no OOM |
| T85 | Server returns truncated HTTP response during download — retry or clean failure |
| T86 | Install unsigned CCS or native package without `--allow-unsigned` — rejection |

**Fixtures**: corrupted CCS packages (bad checksum, truncated chunk, modified-after-signing), corrupted repo metadata, size-lying archive. All fixtures are CCS format; T81 additionally uses a corrupted native package for the container's distro format.

### Group H: Error Recovery (T87-T96, 10 tests)

Verify Conary recovers gracefully from failures mid-operation.

| Test | Description |
|------|-------------|
| T87 | SIGKILL mid-install (after first file deployed) — restart, verify DB and filesystem consistent |
| T88 | Disk-full during install (tmpfs limit) — expect clean rollback, no partial state |
| T89 | Corrupt SQLite WAL file, run any command — detection and meaningful error |
| T90 | Delete CAS directory mid-install — failure with rollback |
| T91 | Kill between DB commit and file deploy — journal-based recovery on next run |
| T92 | `repo-sync` against HTTP 500 — retry then clean failure, existing metadata untouched |
| T93 | Scriptlet exits non-zero — rollback of file deployment |
| T94 | Two concurrent installs of same package — one succeeds, one fails cleanly (uses DB lock to synchronize, not timing) |
| T95 | SIGKILL mid-rollback — system recovers to consistent state on next run |
| T96 | SIGKILL mid-remove — recovery leaves fully removed or fully intact |

**Notes on flaky tests**: T87, T91, T95, T96 use `kill_after_log` (kill on specific log pattern) rather than timing delays for determinism. T94 uses explicit DB lock contention rather than race conditions. All SIGKILL tests run 3 times in CI with majority-pass logic.

### Group I: Security Boundaries (T97-T106, 10 tests)

Verify security enforcement at extraction and execution boundaries.

| Test | Description |
|------|-------------|
| T97 | Package with `../../etc/shadow` path — path traversal blocked |
| T98 | Symlink to `/etc/passwd` followed by overwrite file — symlink attack blocked |
| T99 | Package with setuid binary — setuid bit stripped unless explicitly allowed |
| T100 | Scriptlet accesses `/proc/self/environ` — blocked by sandbox |
| T101 | Scriptlet attempts network access (curl) — blocked by sandbox |
| T102 | Scriptlet writes outside target root — Landlock restriction |
| T103 | Package with expired GPG signature — rejection with meaningful error |
| T104 | Package with capabilities (CAP_NET_RAW) installed without root — enforcement |
| T105 | Package capabilities exceed policy allowance — rejection |
| T106 | Install a package with a decompression bomb (tiny archive, huge output) — size limit enforced |

**Fixtures**: packages with path traversal entries, symlink attacks, setuid binaries, hostile scriptlets, expired signatures, over-privileged capability declarations, decompression bomb.

### Group J: Dependency Edge Cases (T107-T117, 11 tests)

Verify the resolver handles non-trivial dependency graphs.

| Test | Description |
|------|-------------|
| T107 | Circular dependency (A depends on B, B depends on A) — detection and clear error |
| T108 | Two deps require conflicting versions of a third — conflict reported clearly |
| T109 | Package with 20+ transitive deps — all resolved and installed in order |
| T110 | Install B that conflicts with installed A — conflict reported, A untouched |
| T111 | Dependency on virtual provider (`mail-transport-agent`) — any provider satisfies |
| T112 | Remove with `--autoremove` — orphaned deps cleaned, shared deps remain |
| T113 | Version range dependency (`>= 1.0, < 3.0`) — constraint satisfaction |
| T114 | Remove shared-dep provider, autoremove again — fixed-point convergence |
| T115 | Remove package that others depend on — refusal with dependent list |
| T116 | OR dependency (`foo | bar`) — one selected, preference ordering respected |
| T117 | Install package with unresolvable dependency — clear error, no partial state |

**Fixtures**: 10-15 small interdependent CCS fixture packages with circular, conflicting, virtual, ranged, OR, and unresolvable dependency relationships. Built by `tests/fixtures/adversarial/build-deps.sh`. Each fixture is versioned (`dep-chain-a-v1.ccs`) for reproducibility.

### Group K: Server Resilience (T118-T127, 10 tests)

Verify client behavior when the repository misbehaves. All tests use a mock HTTP server inside the container.

| Test | Description |
|------|-------------|
| T118 | Repo returns expired metadata — rejection, previous good metadata preserved |
| T119 | Server drops connection mid-transfer — retry, then clean failure |
| T120 | One mirror 404, another has it — transparent failover |
| T121 | Valid JSON but wrong schema version — meaningful error, no crash |
| T122 | HTTP 429 rate limiting — backoff and retry |
| T123 | Server returns different file than metadata describes (MITM) — checksum rejection |
| T124 | TUF metadata with lower version than cached (rollback attack) — rejection per TUF spec |
| T125 | TLS certificate hostname mismatch — connection refused |
| T126 | Extremely large metadata (100MB+) — size limit enforcement, no OOM |
| T127 | Concurrent `repo-sync` on same repo — locking, no corruption |

### Group L: Lifecycle Robustness (T128-T137, 10 tests)

Self-update, generation management, and bootstrap recovery. Group K tests server-side failures; Group L tests client-side lifecycle given working or mock servers.

| Test | Description |
|------|-------------|
| T128 | Self-update with bad checksum download — rejection, current binary untouched |
| T129 | Self-update server returns older version — "already up to date", no downgrade |
| T130 | Truncated self-update download — clean failure, current binary untouched |
| T131 | Build 3 generations, switch between them — each has correct package state |
| T132 | Build 5 generations, GC with keep=2 — oldest 3 removed, active + 1 preserved |
| T133 | Rollback to previous generation — system state matches generation snapshot |
| T134 | Corrupt generation metadata, attempt switch — detection and refusal |
| T135 | Build generation while another process modifies package DB — locking, no corruption |
| T136 | Bootstrap dry-run — plan output valid, no files modified |
| T137 | Bootstrap stage 0 — output artifacts exist and are valid ELF binaries |

### Group M: Real-World Operations (T138-T149, 12 tests)

Real packages against live repos. The "does the product actually work" group.

| Test | Description |
|------|-------------|
| T138 | Sync Remi, install real CCS package, verify files deployed and binary works |
| T139 | Sync distro repo, install small real package (`tree`), verify binary runs |
| T140 | Install package with 3+ transitive deps from distro repo, verify all installed |
| T141 | Install package, update to newer version, verify files changed |
| T142 | Install two packages sharing a dep, remove one, verify shared dep remains |
| T143 | Sync all repos, check updates, apply them, verify system consistent |
| T144 | Install, pin, full update — pinned package untouched, others updated |
| T145 | Install then remove — all files cleaned up, DB state consistent |
| T146 | Adopt system package already installed by distro PM — Conary tracks it |
| T147 | Install CCS, update, rollback — original version restored |
| T148 | Install from each format in one session (CCS + native) — coexistence |
| T149 | Full lifecycle: sync, install, use, update, pin, unpin, remove, verify |

**Config**: Uses `[distro_overrides]` in manifest for per-distro package names. Depends on live network access to Remi and distro mirrors.

### Group N: Kernel & Boot Verification (T150-T159, 10 tests)

Two tiers: container-based file verification (T150-T155, 6 tests) and QEMU boot tests (T156-T159, 4 tests).

#### Container tests (T150-T155)

| Test | Description |
|------|-------------|
| T150 | Install kernel — vmlinuz, initramfs, modules all at correct paths |
| T151 | Install kernel — BLS entry created at `/boot/loader/entries/` with correct options |
| T152 | Update kernel — new generation with new kernel, old generation untouched |
| T153 | Install + rollback — BLS entries reflect rollback, old kernel default |
| T154 | Install bootloader (systemd-boot/GRUB) — config files deployed correctly |
| T155 | Install kernel + bootloader, `conary verify` — all files pass integrity check |

#### QEMU boot tests (T156-T159)

| Test | Description |
|------|-------------|
| T156 | Boot minimal system image, verify it reaches login prompt |
| T157 | Boot Gen A (kernel 1), switch to Gen B (kernel 2), reboot, verify Gen B active |
| T158 | Boot, install kernel update, reboot, verify new kernel running (`uname -r`) |
| T159 | Boot with corrupted generation metadata — falls back to previous known-good |

## Engine Extensions

The `conary-test` Rust engine needs the following additions.

### New step types and implementation strategy

| Spec Capability | Implementation | Effort | Notes |
|------|---------|---------|---------|
| Expect failure | `assert { exit_code = N }` | None | Existing assertion system |
| Expect stderr content | `assert { stderr_contains = "..." }` | None | Existing assertion system |
| `corrupt_file` | `run { cmd = "dd if=/dev/urandom ..." }` | Minimal | Shell command in container |
| `truncate_file` | `run { cmd = "truncate -s N file" }` | Minimal | Shell command in container |
| `remove_file` | `run { cmd = "rm path" }` | Minimal | Shell command in container |
| `kill_after_log` | **New step type** | High | Bollard exec + log stream monitor, sends SIGKILL on pattern match |
| `mock_server` | **New step type** | High | Configurable HTTP server (see below) |
| `qemu_boot` | **New step type** | High | VM lifecycle via existing orchestrator (see below) |

Only 3 truly new step types are needed. File operations use existing `run` steps with shell commands. Failure assertions use the existing `assert` mechanism.

### Mock HTTP server

A lightweight mock server started inside the container, configured declaratively in TOML:

```toml
[[test.setup]]
type = "mock_server"
port = 8888

[[test.setup.routes]]
path = "/v1/metadata"
status = 200
body_file = "fixtures/valid-metadata.json"

[[test.setup.routes]]
path = "/v1/packages/foo.ccs"
status = 429
headers = { "Retry-After" = "1" }

[[test.setup.routes]]
path = "/v1/packages/bar.ccs"
status = 200
body_file = "fixtures/bar.ccs"
truncate_at_bytes = 1024  # simulate partial response

[[test.setup.routes]]
path = "/v1/packages/evil.ccs"
status = 200
body_file = "fixtures/wrong-file.ccs"  # MITM: serve wrong content
```

Implementation: a small Rust HTTP server binary compiled into the test container image. Started as a background process before test steps run, killed after test completes.

### QEMU boot step

```toml
[[test.step]]
type = "qemu_boot"
image = "minimal-boot-v1"      # fetched from Remi test artifacts
memory_mb = 1024
timeout_seconds = 300
ssh_port = 2222
commands = [
    "uname -r",
    "conary system generation list",
]
expect_output = ["6.18", "Generation 1"]
```

Uses the existing `bootstrap/orchestrator.rs` QEMU orchestration. Image is a qcow2 stored at `https://packages.conary.io/test-artifacts/minimal-boot-{version}.qcow2`, downloaded and cached locally on the CI runner. If image download fails, QEMU tests are skipped (not failed) with a warning.

### Container resource constraints

Per-test resource constraints in manifests, applied via bollard `HostConfig`:

```toml
[[test]]
id = "T88"
name = "disk_full_during_install"

[test.resources]
tmpfs_size_mb = 50       # small tmpfs to trigger disk-full
memory_limit_mb = 512    # prevent OOM from masking the real failure
network_isolated = false
```

Engine maps these to bollard's `create_container` options:
- `tmpfs_size_mb` → `Tmpfs { "/conary": "size=50m" }`
- `memory_limit_mb` → `Memory: 512 * 1024 * 1024`
- `network_isolated` → `NetworkMode: "none"`

### Distro overrides

Per-distro variable substitution in manifests:

```toml
[distro_overrides.fedora43]
small_package = "tree"
dep_heavy_package = "vim-enhanced"
kernel_package = "kernel"

[distro_overrides.ubuntu-noble]
small_package = "tree"
dep_heavy_package = "vim"
kernel_package = "linux-image-generic"

[distro_overrides.arch]
small_package = "tree"
dep_heavy_package = "vim"
kernel_package = "linux"
```

Test steps reference these via `${small_package}` syntax. The engine resolves overrides at runtime based on the `--distro` flag.

## Fixture Infrastructure

```
tests/fixtures/adversarial/
  build-all.sh              # Master build script (calls sub-scripts)
  build-deps.sh             # Builds 10-15 interdependent CCS packages
  build-boot-image.sh       # Builds minimal QEMU-bootable qcow2 image
  corrupted/
    bad-checksum/            # CCS with wrong SHA-256 in manifest
      ccs.toml, stage/
    truncated/               # CCS with chunk file truncated at 50%
      ccs.toml, stage/
    tampered/                # CCS with file modified after signing
      ccs.toml, stage/
    size-lie/                # Archive claiming 1GB, containing 100B
      ccs.toml, stage/
  malicious/
    path-traversal/          # CCS with ../../etc/shadow entry
      ccs.toml, stage/
    symlink-attack/          # Symlink → /etc/passwd + overwrite
      ccs.toml, stage/
    setuid/                  # Binary with setuid bit
      ccs.toml, stage/
    hostile-scriptlet/       # Scriptlet that tries to escape sandbox
      ccs.toml, stage/
    decompression-bomb/      # Tiny archive, huge output
      ccs.toml, stage/
  deps/
    chain-a-v{1,2}.ccs      # 10-15 packages with various dep patterns
    ...
  large/                     # Generated by build-all.sh, not checked in
    10k-files.ccs
    deep-tree.ccs
```

All fixtures are CCS format, versioned as `{name}-v{N}.ccs`. Published to Remi at `https://packages.conary.io/test-fixtures/adversarial/` via extended `scripts/publish-test-fixtures.sh`. Checksums stored in `tests/fixtures/adversarial/SHA256SUMS`.

### QEMU boot image management

- Built by `build-boot-image.sh` using bootstrap stage output
- Stored at `https://packages.conary.io/test-artifacts/minimal-boot-v{N}.qcow2` (~500MB)
- Built manually (not per-release); updated when bootstrap output changes
- CI runners download and cache locally at `~/.cache/conary-test/`
- If download fails or image missing, QEMU tests skip with warning

## CI Integration

| Workflow | Trigger | Phases | Duration |
|----------|---------|--------|----------|
| `integration.yaml` | Push to main | Phase 1 only | ~15 min |
| `e2e.yaml` | Daily 06:00 UTC + manual | Phase 1 + 2 + 3 (Groups G-M) | ~40-50 min |
| `e2e.yaml` (QEMU) | Daily + manual | Group N (T156-T159) | ~10-20 min |

**QEMU tests are informational only** — they do not gate releases. Groups G-M gate the daily E2E.

**Flaky test handling**: Tests marked `flaky = true` in manifests get 3 retries with majority-pass logic. This applies to SIGKILL tests (T87, T91, T95, T96) and concurrent install (T94).

## Implementation Order

1. **Engine extensions** — `kill_after_log`, `mock_server`, `qemu_boot` step types; resource constraints; distro overrides
2. **Fixture infrastructure** — build scripts, corrupted/malicious/dependency fixtures, publish to Remi
3. **Group M** — real-world operations first (highest value, validates the product works)
4. **Group G** — data integrity (catches silent corruption)
5. **Group H** — error recovery (crash safety is critical for a package manager)
6. **Group I** — security boundaries (must-have before production)
7. **Group J** — dependency edge cases
8. **Group K** — server resilience (needs mock server)
9. **Group L** — lifecycle robustness
10. **Group N** — kernel & boot (most infrastructure, highest blast radius)
