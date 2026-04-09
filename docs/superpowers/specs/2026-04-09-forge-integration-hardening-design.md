---
last_updated: 2026-04-09
revision: 1
summary: Design for hardening Forge-backed integration validation through truthful conary-test deployment status, explicit supported operator validation flows, and a modest merge-validation control-plane smoke
---

# Forge Integration Hardening

## Context

Forge currently serves two related but not identical roles:

- it is the self-hosted GitHub Actions runner host for trusted integration
  validation
- it also runs the long-lived `conary-test` service used for operator and MCP
  workflows

Those two roles drifted enough that the operator story is no longer fully
truthful even though the trusted runner lane is still healthy.

Observed state on April 9, 2026:

- the latest `merge-validation` run for current `main` commit `6533e5dd`
  succeeded on GitHub Actions
- the latest `scheduled-ops` runs that completed successfully were still pinned
  to older Phase 2-era commit `76e68acd`
- `conary-test health --json` on Forge emitted a human "Local status" banner
  before the JSON object in the no-admin-creds path
- `conary-test deploy status --json` reported only `version`,
  `service_status`, `git_branch`, and `git_commit`, even though docs claim it
  shows uptime and WAL pending items
- the Forge checkout at `~/Conary` reported `git_commit = f72ca32`, which no
  longer matched current `main`
- a direct SSH-driven
  `cargo run -p conary-test -- run --suite phase1-core --distro fedora43 --phase 1`
  on Forge failed during the install step with composefs/EROFS loopback mount
  failure, even though the GitHub self-hosted runner path for `merge-validation`
  passed

Relevant current implementation seams:

- CLI-side deployment and health output lives in
  `apps/conary-test/src/handlers.rs`
- the service already tracks `start_time` and WAL state in
  `apps/conary-test/src/server/state.rs`
- the MCP `deploy_status` tool already reports uptime and WAL pending using
  that service state in `apps/conary-test/src/server/mcp.rs`
- GitHub workflow policy lives in `.github/workflows/merge-validation.yml` and
  `.github/workflows/scheduled-ops.yml`

## Goal

Make Forge-backed integration validation trustworthy and operable by:

- making `conary-test` deployment and health output honest and machine-readable
- making the relationship between Forge checkout state and running service state
  explicit instead of implied
- defining a supported Forge operator smoke path that is smaller and more
  reliable than ad hoc full-suite shell execution
- adding one modest additional on-merge check that hardens the Forge
  control-plane contract without redesigning the full CI topology

## Non-Goals

This design does not attempt to:

- redesign GitHub Actions workflow topology or move the full nightly matrix onto
  every push
- replace GitHub Actions with a separate Forge-native CI control plane
- add a new daemon, scheduler, agent, or long-lived reconciliation service on
  Forge
- change Remi admin API schema
- solve every possible host-specific failure mode of arbitrary raw shell
  commands launched directly over SSH
- add schema migrations

## Decision

Adopt a single-source-of-truth deployment status design centered on the
`conary-test` service.

The running `conary-test` service should become the authoritative source for:

- running binary provenance
- service uptime and start time
- WAL pending count
- active run count
- service health from the service's own point of view

The CLI should stop synthesizing "deployment status" solely from local checkout
state and `systemctl` output. Instead:

- the service exposes a local deployment-status endpoint
- the MCP tool and HTTP route share one status builder
- the CLI consumes that status and layers checkout/ref information on top when
  helpful

At the workflow level:

- `merge-validation` stays intentionally small
- one additional Forge control-plane smoke is added on merge
- deep Phase 1-4 matrix coverage remains in `scheduled-ops`

At the operator level:

- supported manual validation becomes an explicit, lightweight Forge
  control-plane smoke path
- raw SSH-driven full-suite execution remains available for debugging, but is
  no longer implied to be the primary trusted operator path

## Options Considered

### 1. Ops-only truthfulness fixes

Fix `health --json`, fix `deploy status`, and clean up docs, but leave merge
validation unchanged.

Pros:

- smallest change surface
- directly addresses the most obvious operator pain

Cons:

- current `main` would still only get a very shallow on-merge signal
- does not convert the control-plane fixes into any automated guardrail

### 2. Forge hardening plus one modest merge-gate bump

Fix the operator/control-plane truthfulness issues and add one lightweight
control-plane smoke to `merge-validation`.

Pros:

- directly addresses the observed gaps
- stays inside Conary's current shape
- improves confidence in both human and automated Forge validation
- avoids dragging nightly matrix behavior into every push

Cons:

- still relies on scheduled/manual deep validation for broader coverage

### 3. Full CI and Forge validation redesign

Reshape merge gates, nightly jobs, manual validation, and possibly runner
behavior together.

Pros:

- could yield the strongest long-term CI story

Cons:

- much larger scope
- easy to turn into platform work instead of product hardening
- not required to fix the specific observed failures

Recommended option: `2`.

## Proposed Design

### 1. Shared deployment status model

Introduce a shared deployment-status model in the `conary-test` service layer,
owned by `apps/conary-test/src/server/service.rs`, instead of duplicating
status logic across CLI and MCP.

The shared model should report:

- `binary`
  - `version`
  - `git_commit`
  - `commit_timestamp`
  - optional `build_timestamp` only when injected explicitly by CI or release
    tooling
- `runtime`
  - `started_at`
  - `uptime_seconds`
  - `uptime_human`
  - `wal_pending`
  - `active_runs`
- `service`
  - `status`
- `source`
  - optional checkout branch/commit when the caller is able to augment with
    local checkout state
- `drift`
  - optional booleans or explanatory notes when checkout state and running
    binary state differ

The service-owned portion of this model must not rely on local checkout git
state. Running binary provenance and runtime metadata should come from the
running service itself.

### 2. Running binary provenance

`conary-test` should embed lightweight build metadata at compile time so the
running service can report what binary is actually executing.

The required minimum metadata is:

- crate version
- git commit
- commit timestamp

Optional metadata:

- CI or release-time build timestamp, but only when provided explicitly through
  an environment variable or equivalent stable build input

This should be implemented with a small `build.rs` or equivalent generated
metadata path inside `apps/conary-test`, not a separate provenance service.

Important constraint: the implementation must not emit a fresh wall-clock
timestamp on every local Cargo invocation. Doing so would invalidate Cargo's
incremental build cache for `conary-test` and degrade local development
workflows. The stable default should be git-derived metadata such as the last
commit timestamp. A true build timestamp is acceptable only when injected by a
stable CI/release environment variable.

Preferred implementation shape:

- capture `git rev-parse HEAD`
- capture `git log -1 --format=%cd`
- avoid hand-parsing `.git` internals beyond coarse Cargo rerun watchers
- in normal checkouts, watch `.git/HEAD`, `.git/refs`, and optional
  `.git/packed-refs`
- in git worktrees, resolve both the worktree git dir and the common git dir:
  watch worktree `HEAD`, common `refs` / optional `packed-refs`, and the
  worktree `.git` indirection file when present

This is intentionally narrower than full release provenance. The goal here is
operator truthfulness, not supply-chain attestation.

### 3. Local deployment-status HTTP route

Add a local deployment-status route to the `conary-test` server, for example:

- `GET /v1/deploy/status`

This route should be built from the same shared deployment-status function used
by the MCP `deploy_status` tool.

The route should expose:

- service-owned runtime state
- running binary provenance
- WAL pending count
- active run count

It must not depend on checkout git state.

This route is intentionally local Forge infrastructure state, similar in spirit
to the existing local `health` endpoint.

This route should be public and unauthenticated, like `GET /v1/health`, because
it only exposes local runtime metadata needed by operators and the local CLI.

### 4. CLI `deploy status` behavior

`conary-test deploy status` should stop pretending that checkout git state is
the same thing as running service identity.

New behavior:

1. resolve the local service port using one documented precedence contract:
   - explicit `--port` flag on the relevant CLI commands wins
   - else `CONARY_TEST_PORT` when set
   - else default to `9090`, which matches the current service contract
2. query local `GET /v1/deploy/status` at `127.0.0.1:<port>` when the service is reachable
3. collect local checkout git branch/commit separately from the project
   directory
4. return a combined status object in CLI JSON/text output

The CLI should make the distinction explicit:

- `binary.git_commit` = the running service binary's build provenance
- `checkout.git_commit` = the current source checkout used by the operator
- `checkout_matches_binary` or equivalent drift note when they differ

If the service is unreachable, `deploy status` should still emit valid JSON and
clearly mark the result as degraded instead of silently collapsing to checkout
state only.

### 5. CLI `health` behavior

`conary-test health --json` must always emit valid JSON.

The JSON shape should be normalized into one documented envelope in both the
Remi-admin-configured path and the local fallback path.

Recommended top-level shape:

- `mode`
  - `remi`
  - `local`
- `deploy_status`
  - shared deployment-status object
- `remi`
  - optional proxied Remi health payload when available
- `reason`
  - optional explanation for local fallback or degraded mode

In the local fallback path:

- it must not print a human banner before the JSON
- it should populate the same envelope with `mode = "local"` and omit the
  `remi` field when no Remi payload is available

Text-mode output can remain human-friendly.

### 6. Supported Forge validation modes

This design explicitly separates three validation modes:

#### Trusted on-merge validation

GitHub Actions `merge-validation` remains the trusted merge gate on the
self-hosted Forge runner.

#### Deep scheduled validation

`scheduled-ops` remains the home for the broader Phase 1-4 matrix and QEMU
lanes.

#### Supported operator smoke

Add a lightweight Forge control-plane smoke path, implemented with existing
`conary-test` commands and local endpoints rather than a new service.

The recommended shape is a small wrapper script under `scripts/`, for example
`scripts/forge-smoke.sh`, that validates:

- local `conary-test` health endpoint reachability
- valid JSON from `conary-test health --json`
- valid JSON and required keys from `conary-test deploy status --json`
- manifest reload or suite listing sanity if cheap enough

The smoke script should use the same local service port resolution rules as the
CLI:

- explicit flag when provided
- else `CONARY_TEST_PORT`
- else `9090`

For operator use, the script should prefer a repo-local built binary when
available and fall back to `conary-test` on `$PATH` before failing. A supported
smoke path should not require a fresh local debug build if the service binary
is already installed on the host.

This script should be safe for manual operator use over SSH and cheap enough to
reuse from GitHub Actions.

Full raw-suite execution over SSH, such as directly invoking
`cargo run -p conary-test -- run ...`, should be documented as a debugging path
instead of the primary supported operator smoke.

### 7. Merge-validation hardening

Extend `.github/workflows/merge-validation.yml` with one additional Forge
control-plane smoke lane.

This should not be a full new distro matrix or a nightly-grade expansion.

The new merge-time check should validate the operator/control-plane contract,
not just package-manager Phase 1 behavior. The preferred implementation is to
run the shared lightweight Forge smoke script described above against a freshly
started server from the workflow's newly built `target/debug/conary-test`
binary on a dedicated test port such as `9099`.

Important constraint: the workflow must not point the smoke script at the
long-lived Forge daemon on `127.0.0.1:9090`, because that would validate the
previously deployed host service instead of the code under test in the current
workflow checkout.

Target outcome:

- current `phase1-core` package-manager smoke still runs
- Remi smoke still runs
- merge-time validation also fails if the Forge control-plane output contract is
  broken

This is intentionally a control-plane hardening step, not a substitute for the
scheduled deep-validation matrix.

### 8. Documentation updates

Update the docs so the supported story matches reality:

- `docs/INTEGRATION-TESTING.md`
  - describe trusted merge validation vs scheduled deep validation
  - describe supported operator smoke vs debug-only shell full-suite execution
  - document the actual `deploy status` and `health` JSON semantics
- `docs/operations/infrastructure.md`
  - clarify Forge's role as runner host plus `conary-test` service host
  - document the supported operator smoke path
- `apps/conary-test/README.md`
  - align command descriptions with actual status fields and supported usage
- `deploy/FORGE.md`
  - remove any implication that raw full-suite shell execution is the primary
    supported validation path if the implementation keeps it debug-only

## Error Handling

### Service unavailable

If the local `conary-test` service cannot be reached:

- `deploy status --json` must still emit valid JSON
- output must clearly mark the service as unreachable or degraded
- checkout state must not be mislabeled as running service state

### Missing build metadata

If build metadata cannot be determined at compile time:

- status should fail closed into explicit `"unknown"` or equivalent fields
- it must not silently substitute checkout git state for running binary
  provenance

### Missing Remi admin credentials

If `REMI_ADMIN_TOKEN` or `REMI_ADMIN_ENDPOINT` is not set:

- `health --json` must still return valid JSON in local mode
- text mode may still explain why the local path was used

## Testing Strategy

### Automated tests

Add focused tests for:

- shared deployment-status builder logic
- embedded build-metadata parsing/formatting
- `health --json` fallback returning pure JSON
- `health --json` using the same top-level schema in both Remi and local modes
- `deploy status --json` schema and drift reporting
- local port-resolution helper behavior
- local HTTP `GET /v1/deploy/status`
- any lightweight Forge smoke script logic that can be validated locally

### Workflow verification

After implementation:

- verify the updated `merge-validation` workflow on GitHub Actions
- confirm the new control-plane smoke runs on the self-hosted Forge runner

### Manual host verification

Because some of this behavior is inherently host-specific, perform one manual
Forge verification pass after implementation:

- local health endpoint
- `conary-test health --json`
- `conary-test deploy status --json`
- supported Forge smoke wrapper

## Implementation Shape

Expected primary change surface:

- `apps/conary-test/src/handlers.rs`
- `apps/conary-test/src/cli.rs`
- `apps/conary-test/src/server/service.rs`
- `apps/conary-test/src/server/handlers.rs`
- `apps/conary-test/src/server/routes.rs`
- `apps/conary-test/src/server/mcp.rs`
- `apps/conary-test/src/server/state.rs`
- `apps/conary-test/src/server/wal.rs`
- `apps/conary-test/build.rs` or equivalent build-metadata helper
- `.github/workflows/merge-validation.yml`
- `scripts/forge-smoke.sh` or equivalent wrapper
- `docs/INTEGRATION-TESTING.md`
- `docs/operations/infrastructure.md`
- `apps/conary-test/README.md`
- `deploy/FORGE.md`

## Success Criteria

This design is complete when:

- `conary-test health --json` is always valid JSON
- `conary-test deploy status` clearly distinguishes running binary state from
  checkout state
- the service itself can report uptime, WAL pending count, and running binary
  provenance through one shared status model
- Forge operator docs describe one supported smoke path and stop overpromising
  raw shell full-suite behavior
- `merge-validation` includes one additional Forge control-plane smoke beyond
  the current package-manager Phase 1 lane
- current `main` gets stronger merge-time signal without absorbing the nightly
  deep-validation matrix
