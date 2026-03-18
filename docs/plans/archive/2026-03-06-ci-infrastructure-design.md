# CI & Validation Infrastructure Design

**Date:** 2026-03-06
**Status:** Approved

## Problem

1,800+ unit tests and 37 integration tests exist but run only manually. No CI triggers on push. No scheduled Remi health monitoring. No automated proof that features work on real systems.

## Existing Assets

- Integration test harness: `tests/integration/remi/` with 37 tests (T01-T37)
- Containerfiles for Fedora 43, Ubuntu Noble, Arch Linux
- Test runner with JSON result output and assertion library
- Forge server: `forge.conarylabs.com` (Fedora 43, 8GB RAM, 151GB disk, Podman installed)
- Remi server: `packages.conary.io` (production, serving packages)

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| CI platform | Forgejo (native install on Forge) | Self-hosted, Podman-native, full control |
| Runner type | Native on host | Direct access to Rust toolchain, Podman, full hardware |
| Repo sync | Forgejo built-in mirror from GitHub | Polls on schedule, no webhook setup needed |
| Workflow split | 3 workflows (ci, integration, remi-health) | Separation of concerns: fast gate, full matrix, scheduled monitoring |
| Remi monitoring | On-push smoke + scheduled full check | Catch both code regressions and production issues |

## Forgejo Server Setup

- Forgejo installed natively on `forge.conarylabs.com`
- GitHub repo mirrored via Forgejo's built-in mirror feature
- Forgejo Runner installed natively, registered with label `linux-native`
- Runner has access to: Rust 1.93, Podman, 8GB RAM, 151GB disk

## CI Workflows

### 1. ci.yaml -- On push to main (fast gate, ~5 min)

4 parallel jobs on `linux-native` runner:
- `build`: `cargo build`
- `test`: `cargo test`
- `clippy`: `cargo clippy -- -D warnings`
- `remi-smoke`: Quick curl checks against packages.conary.io (/health, sparse index)

### 2. integration.yaml -- On push to main (after ci passes, ~15 min)

3 parallel Podman-based integration test runs:
- `fedora`: `./tests/integration/remi/run.sh --build --distro fedora43`
- `ubuntu`: `./tests/integration/remi/run.sh --build --distro ubuntu-noble`
- `arch`: `./tests/integration/remi/run.sh --build --distro arch`

Each runs the full 37-test suite inside a container against live packages.conary.io.

### 3. remi-health.yaml -- Scheduled (every 6 hours)

Full Remi endpoint verification:
- `/health` endpoint
- `/v1/{distro}/metadata` for all 3 distros (fedora, ubuntu, arch)
- `/v1/packages/{name}` sparse index lookup
- `/v1/stats/overview`
- Test conversion: submit package, poll until complete, verify chunks

## Components to Build

| Component | File | Purpose |
|-----------|------|---------|
| CI fast gate | `.forgejo/workflows/ci.yaml` | Build, test, clippy, Remi smoke on push |
| Integration matrix | `.forgejo/workflows/integration.yaml` | 3-distro Podman test matrix on push |
| Remi health cron | `.forgejo/workflows/remi-health.yaml` | Scheduled Remi endpoint monitoring |
| Health script | `scripts/remi-health.sh` | Endpoint verification (used by ci.yaml and remi-health.yaml) |
| Forge setup script | `deploy/setup-forge.sh` | Install Forgejo + runner on Forge |
| Forge docs | `deploy/FORGE.md` | Setup instructions and troubleshooting |

## Out of Scope

- DNS/TLS for forge.conarylabs.com (manual setup)
- GitHub webhook for real-time mirroring (poll-based is sufficient)
- Notification integration (Slack/email on failure)
