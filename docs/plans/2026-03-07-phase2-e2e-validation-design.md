# Phase 2: End-to-End Validation Design

## Overview

Deepen the existing Remi integration suite (T38+) to exercise full code paths
(dependency resolution, scriptlets, rollback, checksums), then broaden to cover
bootstrap, recipe, and Remi client flows. All tests run as root inside
disposable Podman containers across Fedora 43, Ubuntu Noble, and Arch.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Priority | Depth first, then breadth | Install flow with real dep resolution is the critical path everything else builds on |
| Runner architecture | Single test-runner.sh with `--phase2` flag | Reuse existing infrastructure, let CI choose speed vs depth |
| Container privileges | Always root | Containers are disposable, no reason to complicate with privilege splitting |
| Rollback verification | DB + filesystem + content checksums | Gold standard: prove CAS retrieval works correctly through rollback cycles |
| Delta testing | Purpose-built test fixtures on Remi | Controlled v1/v2 packages with known content for deterministic delta verification |
| Bootstrap scope | Dry-run + stage 0 | Proves pipeline actually runs without being a CI resource hog; full pipeline later as nightly |
| Recipe/build scope | Cook + PKGBUILD conversion + hermetic isolation | All three workflows tested; containers already have namespace capabilities |
| Remi integration scope | Client-side only | Federation needs multi-node setup, out of scope for container tests |

## Test Fixture Strategy

Publish `conary-test-fixture` to Remi for all 3 distros:

- **v1.0**: `/usr/share/conary-test/hello.txt` with content `"hello-v1"` (known SHA-256)
- **v2.0**: `/usr/share/conary-test/hello.txt` with content `"hello-v2"` (known SHA-256), plus `/usr/share/conary-test/added.txt`
- Both versions include a post-install scriptlet that touches `/var/lib/conary-test/installed`
- Known checksums hardcoded in tests for content verification

## Test Groups

### Group A: Deep Install Flow (T38-T50)

Removes training wheels from existing tests -- real dep resolution, scriptlets, checksums.

| ID | Test | Verifies |
|----|------|----------|
| T38 | Install fixture v1 with deps | `--dep-mode takeover` with transitive deps, no `--no-deps` |
| T39 | Verify dep files on disk | All resolved deps actually deployed to filesystem |
| T40 | Verify v1 content checksum | `/usr/share/conary-test/hello.txt` SHA-256 matches expected |
| T41 | Verify scriptlet ran | `/var/lib/conary-test/installed` exists (post-install touched it) |
| T42 | Remove with scriptlets | pre-remove / post-remove execute, marker file cleaned |
| T43 | Reinstall fixture v1 | Clean install for update chain |
| T44 | Update v1 -> v2 | Update to v2, verify new content checksum |
| T45 | Delta update verification | v1 -> v2 used delta path, verify binary diff applied |
| T46 | Verify v2 added file | `/usr/share/conary-test/added.txt` exists |
| T47 | Rollback after update | Rollback to v1, verify v1 checksums restored |
| T48 | Rollback filesystem check | v1 files restored, v2-only files gone |
| T49 | Pin blocks update | Pin v1, run update, verify still on v1 |
| T50 | Orphan detection | Install pkg with dep, remove pkg, verify orphan flagged |

### Group B: Generation Lifecycle (T51-T57)

| ID | Test | Verifies |
|----|------|----------|
| T51 | Build generation | `conary system generation build` succeeds from current state |
| T52 | Generation list | New entry visible with number, date, package count |
| T53 | Generation info | Metadata includes composefs/EROFS format |
| T54 | Switch generation | `conary system generation switch` to new generation |
| T55 | Rollback generation | Switch back to previous, verify state matches |
| T56 | GC old generation | Remove unreferenced generation, verify disk freed |
| T57 | System takeover full | Adopt all system packages -> build generation (not dry-run) |

### Group C: Bootstrap Pipeline (T58-T61)

| ID | Test | Verifies |
|----|------|----------|
| T58 | Bootstrap dry-run | Config parses, dep graph resolves, stages planned |
| T59 | Stage 0 runs | Host toolchain verified, bootstrap environment created |
| T60 | Stage 0 output valid | Expected artifacts exist with correct structure |
| T61 | Stage 1 starts | Toolchain build begins (timeout after proof-of-life) |

### Group D: Recipe & Build (T62-T66)

| ID | Test | Verifies |
|----|------|----------|
| T62 | Cook TOML recipe | Simple recipe produces CCS package output |
| T63 | CCS output valid | Package has expected metadata, files, checksums |
| T64 | PKGBUILD conversion | Known PKGBUILD -> valid Conary recipe |
| T65 | Converted recipe cooks | Round-trip: PKGBUILD -> recipe -> CCS |
| T66 | Hermetic build isolation | Build with network blocked, verify no external access |

### Group E: Remi Client (T67-T71)

| ID | Test | Verifies |
|----|------|----------|
| T67 | Sparse index fetch | Client retrieves and parses sparse index from packages.conary.io |
| T68 | Chunk-level install | Client has partial chunks, fetches only missing ones |
| T69 | OCI manifest valid | `/v2/` API returns parseable OCI manifest |
| T70 | OCI blob fetch | Pull a blob by digest, verify content |
| T71 | Stats endpoint | `/stats` returns valid JSON with expected fields |

## CI Integration

| Trigger | Suite | Duration |
|---------|-------|----------|
| Every push to main | T01-T37 (fast suite) | ~5 min |
| Daily schedule + manual | T01-T71 (`--phase2`) | ~20-30 min |

New workflow: `.forgejo/workflows/e2e.yaml` with `schedule` and `workflow_dispatch` triggers.

## Prerequisites

1. **Test fixture packages** -- build and publish `conary-test-fixture` v1.0 + v2.0 to Remi for all 3 distros
2. **Containers run as root** -- verify Containerfiles don't drop privileges
3. **Bootstrap disk** -- containers need ~2GB free for stage 0/1
4. **Network namespaces** -- Podman already supports this for hermetic build tests
