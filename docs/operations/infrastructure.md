---
last_updated: 2026-04-09
revision: 3
summary: Non-secret infrastructure, MCP, and deployment guidance for Conary contributors and coding assistants
---

# Infrastructure Overview

## Host Roles

- Remi is the production package service behind `https://remi.conary.io`.
- Forge is the trusted GitHub runner host used for `conary-test` validation,
  test-harness service work, and source-sync validation.
- Forge also serves as the current local-only staging host for `conaryd`
  release deployment verification.
- Sensitive usernames, credentials, or workstation-only shortcuts belong in the
  ignored `docs/operations/LOCAL_ACCESS.md`, not in tracked docs.

## MCP-First Operations

Prefer MCP tools when they already cover the workflow:

- Remi admin and package-service operations
- `conary-test` run control, deploy/restart flows, image management, and fixture publishing

Use manual SSH, rsync, or curl only when the MCP surface does not cover the
task or when you are debugging the underlying service path itself.

## Safe Public And Admin Endpoints

- Public package service: `https://remi.conary.io`
- Public authenticated MCP endpoint: `https://remi.conary.io/mcp`
- Remi admin origin API: `https://localhost:8082` via SSH tunnel or direct
  origin access
- Remi OpenAPI spec: `https://localhost:8082/v1/admin/openapi.json` via SSH
  tunnel or direct origin access
- Forge-local `conary-test` health endpoint: `http://127.0.0.1:9090/v1/health`
- Forge-local `conary-test` deploy-status endpoint: `http://127.0.0.1:9090/v1/deploy/status`

## Source Deploy Patterns

### Forge

- Preferred deployment path is managed rollout orchestration through
  `conary-test deploy rollout`
- From an operator workstation, use
  `./scripts/deploy-forge.sh --group control_plane --ref main` for the trusted
  default path
- `--ref` is the normal supported source mode and resolves an exact GitHub ref
  on Forge before build/restart/verify
- `--path` remains available for debug/local-snapshot deploys; the wrapper keeps
  the rsync boundary by syncing directly over the active Forge checkout before
  invoking the managed rollout there
- Rollout groups live in `deploy/forge-rollouts.toml`
- `conary-test deploy status --json` now reports both live binary truth and the
  last successful managed rollout, including explicit drift flags
- For supported control-plane verification, run `bash scripts/forge-smoke.sh`
- Port resolution for CLI and smoke checks is `--port` > `CONARY_TEST_PORT` >
  `9090`
- `conaryd` is not yet a managed rollout unit here; its release deployment path
  is the GitHub `deploy-and-verify` workflow plus the checked-in Forge helper
  assets
- `conaryd` deployment verification is Forge-local over
  `scripts/conaryd-health.sh`, which probes `/run/conary/conaryd.sock` rather
  than a public network endpoint
- The tracked Forge bootstrap trust for that path lives in
  `deploy/ssh/forge-known-hosts` and `deploy/sudoers/conaryd-forge`

### Remi

- Use rsync to `/root/conary-src/`
- Exclude `target/`, `.git/`, and `.worktrees/`
- Build `remi`, stop the service before replacing the live binary, then restart
  and verify the local health endpoint
- The public frontends currently share the Remi host but deploy as two separate
  static sites:
  `conary.io` syncs to `/conary/site/`, while `remi.conary.io` syncs to
  `/conary/web/`
- The package frontend is the one wired into Remi's tracked config via
  `[web].root = "/conary/web"`; the main site remains a separate static root on
  the same host

Do not overwrite the live Remi binary while `remi.service` is still running the
old process. That can fail with `Text file busy`.

## Release Flow

- GitHub Actions is the only long-term CI/CD control plane.
- Run `./scripts/release.sh [conary|remi|conaryd|conary-test|all]` to inspect
  the current release baseline, bump owned versions, update release state, and
  create canonical tags
- The supported release tracks are:
  - `conary`
  - `remi`
  - `conaryd`
  - `conary-test`
- Canonical tag forms are:
  - `v*` for `conary`
  - `remi-v*` for `remi`
  - `conaryd-v*` for `conaryd`
  - `conary-test-v*` for `conary-test`
- Legacy tags are read for continuity only:
  - `server-v*` continues the historical `remi` line
  - `test-v*` continues the historical `conary-test` line
- New releases emit canonical tags only; legacy prefixes remain lookup-only
- Push the relevant canonical tags to trigger the GitHub release pipeline
- GitHub Actions builds release artifacts in `release-build` and serializes the
  resolved product metadata into the bundle
- `deploy-and-verify` consumes that serialized metadata instead of re-deriving
  product behavior locally
- `conary-test` is a supported build-and-release track in this phase, but it
  intentionally has no deployment lane
- `deploy-and-verify` performs protected deployment and verification only for
  deployable products (`conary`, `remi`, and `conaryd`)
- The `conaryd` lane deploys only to the Forge local-only staging daemon today;
  public production hosting for `conaryd` is still an open follow-up
- Release verification is a GitHub workflow concern, not a Forgejo or
  Forge-hosted control-plane concern

## Contributor Notes

- Prefer the tracked docs for stable roles and workflows, and keep local-only
  access details in `docs/operations/LOCAL_ACCESS.md`, using
  [`docs/operations/LOCAL_ACCESS.example.md`](LOCAL_ACCESS.example.md) as the
  starting template
- For suite layout, phase selection, and manifest-run behavior, use
  [`docs/INTEGRATION-TESTING.md`](../INTEGRATION-TESTING.md)
- For supported Forge smoke validation, prefer `scripts/forge-smoke.sh` over
  treating raw `cargo run -p conary-test -- run ...` as the main operator path
- For legacy historical context, use [`docs/llms/archive/claude-era-notes.md`](../llms/archive/claude-era-notes.md)
