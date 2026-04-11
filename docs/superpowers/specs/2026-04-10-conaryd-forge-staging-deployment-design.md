---
last_updated: 2026-04-10
revision: 1
summary: Forge-first local-only staging deployment design for conaryd
---

# Conaryd Forge Staging Deployment: Design Spec

**Date:** 2026-04-10  
**Status:** Draft for user review (design approved in conversation)  
**Goal:** Define the first truthful deployment target for `conaryd` by
deploying it to Forge as a local-only staging daemon, aligning the GitHub
release deploy lane with that host reality, and removing the false assumption
that `conaryd` already has a public production endpoint.

---

## Scope

This task covers the first real host deployment model for `conaryd`.

It includes:

- Forge as the initial `conaryd` deployment target
- a checked-in `conaryd` systemd system unit for Forge
- local-only verification on Forge via `127.0.0.1:7890/health`
- updates to the GitHub `deploy-and-verify` lane for `conaryd`
- updates to Forge rollout modeling so `conaryd` is represented as a managed
  Forge unit
- documentation updates that describe `conaryd` on Forge as staging, not
  production

It excludes:

- a public internet-facing `conaryd` service
- DNS, TLS, or reverse proxy exposure for `conaryd`
- moving `conaryd` onto the Remi host
- introducing a separate dedicated `conaryd` production host
- changing `conaryd` CLI or API behavior beyond what deployment verification
  needs

## Non-Goals

- treating Forge as production for `conaryd`
- claiming that a successful Forge deployment means `conaryd` now has a public
  service story
- rebuilding `conaryd` from source on Forge during the GitHub release deploy
  lane instead of installing the exact published release artifact
- widening existing Forge rollout groups in a way that silently starts
  deploying `conaryd` during unrelated control-plane operations

---

## Repository Context

The repo currently describes `conaryd` as a local system daemon:

- [README.md](../../README.md) describes it as "a local daemon" that provides a
  REST API over a Unix socket, with optional TCP for local use
- [apps/conaryd/src/daemon/mod.rs](../../apps/conaryd/src/daemon/mod.rs)
  defaults the TCP listener to `127.0.0.1:7890`
- the current GitHub deploy lane in
  [.github/workflows/deploy-and-verify.yml](../../.github/workflows/deploy-and-verify.yml)
  assumes a remote target plus a public or externally reachable
  `CONARYD_VERIFY_URL`

The release-hardening pass proved that this assumption is false today:

- `conaryd-v0.6.0` was built and published successfully
- the live `deploy-and-verify` run failed because `CONARYD_SSH_KEY`,
  `CONARYD_SSH_TARGET`, and `CONARYD_VERIFY_URL` were blank in workflow context
- local infrastructure docs and credentials did not identify any current live
  `conaryd` host
- manual inspection of the known Forge and Remi hosts did not find an existing
  `conaryd.service`

Forge is the least-wrong first deployment target because the tracked docs
already define it as the trusted validation and control-plane host:

- [deploy/FORGE.md](../../deploy/FORGE.md)
- [deploy/forge-rollouts.toml](../../deploy/forge-rollouts.toml)
- [scripts/deploy-forge.sh](../../scripts/deploy-forge.sh)

That makes Forge the right place to establish a truthful staging deployment for
`conaryd` before any broader production-host decision is made.

---

## Decision

Use **Forge-first, local-only staging deployment** as the first real `conaryd`
host model.

This means:

- `conaryd` is deployed to Forge, not Remi
- `conaryd` remains local-only on Forge
- verification is done on-host over SSH against `127.0.0.1:7890/health`
- GitHub release deployment installs the exact published artifact on Forge
- docs and workflow comments explicitly call this a staging deployment

Rejected alternatives:

- **Remi sidecar deployment**
  - rejected because it muddies the role of the package-service host and does
    not match the current host responsibilities in local credentials/docs
- **artifact-only release with no host deployment**
  - rejected as the default because it preserves the current false confidence:
    the repo would still claim a deploy lane without proving the daemon under
    systemd on any real box
- **public `conaryd` endpoint on Forge**
  - rejected because it invents an exposure model the daemon does not need yet
    and creates avoidable DNS/TLS/reverse-proxy work

---

## Design

### 1. Host Role And Topology

Forge becomes the first real `conaryd` host, but explicitly as staging.

Target runtime shape:

- host: Forge (`forge.conarylabs.com`)
- service class: systemd **system** unit
- binary path: `/usr/local/bin/conaryd`
- Unix socket path: `/run/conary/conaryd.sock`
- DB path: `/var/lib/conary/conary.db`
- TCP bind: `127.0.0.1:7890`
- exposure: local-only

The daemon is not public on Forge:

- no public DNS entry for `conaryd`
- no reverse-proxy exposure
- no external `VERIFY_URL`

Success is defined by on-host checks:

1. `systemctl is-active conaryd` returns active
2. `curl -fsS http://127.0.0.1:7890/health` succeeds
3. the health response reports the expected release version

### 2. Forge Service Definition

Add a checked-in systemd unit file:

- `deploy/systemd/conaryd.service`

The unit should:

- run as a system service
- install/start `conaryd` from `/usr/local/bin/conaryd`
- bind TCP only to `127.0.0.1:7890`
- create or assume the runtime/state directories needed by the daemon
- restart on failure

The design should prefer systemd-owned directories where practical so the
runtime shape stays explicit and repeatable on Forge.

The service must be valid for a host where `conaryd` does meaningful work but
is still intentionally non-public.

### 3. Forge Rollout Modeling

Represent `conaryd` in the Forge rollout manifest so the host's checked-in
deployment model remains coherent.

Update:

- `deploy/forge-rollouts.toml`

Add:

- a new rollout unit for `conaryd`
- a dedicated rollout group such as `conaryd_staging`

Do **not** silently add `conaryd` to existing groups like `control_plane` on
the first pass. Existing Forge deployments should not start mutating a new
system daemon as a side effect.

The rollout schema/orchestrator in `conary-test` should learn a second restart
mode:

- existing: `systemd_user_unit`
- new: `systemd_system_unit`

This keeps the Forge rollout model honest:

- `conary-test` remains a user service
- `conaryd` becomes a system daemon

The rollout verification model should also learn a `conaryd`-appropriate local
health mode, for example:

- `forge_smoke` for `conary-test`
- `conaryd_local_health` for `conaryd`

That verify mode should perform the localhost health/version check on Forge and
should not depend on any public endpoint.

### 4. GitHub Release Deploy Lane

The GitHub `release-build` lane stays artifact-oriented and should continue to
publish the canonical `conaryd` tarball and metadata.

The `deploy-and-verify` lane for `conaryd` should change from:

- "copy tarball to an unspecified host and curl a secret-provided verify URL"

to:

- "copy the exact release tarball to Forge, install it there, restart the
  systemd unit, and verify locally on-host"

Update:

- `.github/workflows/deploy-and-verify.yml`

Keep secrets:

- `CONARYD_SSH_KEY`
- `CONARYD_SSH_TARGET`

Remove the public verification abstraction:

- delete `CONARYD_VERIFY_URL`

The deploy lane should:

1. download the release-build artifact tarball
2. SCP it to Forge
3. invoke a checked-in remote install/restart helper on Forge over SSH
4. verify `conaryd` on Forge with localhost health and version checks

The workflow must install the exact published artifact, not rebuild from source
on Forge. Release deployment should verify the shipped binary, not a fresh
source build.

### 5. Remote Install And Verify Helper

To keep the workflow readable and keep privileged host logic versioned in the
repo, add a checked-in helper script for the Forge-side install/restart flow.

The helper should be responsible for:

- unpacking the tarball
- installing or updating `/usr/local/bin/conaryd`
- installing or refreshing the systemd unit if needed
- reloading systemd when the unit changes
- restarting `conaryd`
- verifying:
  - service active state
  - localhost health endpoint
  - expected version string in the health payload

The GitHub workflow should call that helper over SSH rather than embedding the
entire host mutation flow inline in YAML.

### 6. Privilege Model

Forge is currently documented around the `peter` user, not raw root login.

The design should therefore assume:

- SSH target remains the documented Forge operator account
- privileged host mutations are performed through `sudo`

Host prerequisites for the deployment user:

- passwordless `sudo` for the specific `conaryd` install/restart operations the
  checked-in helper needs

This is preferable to normalizing a root SSH target for the first staging pass.

### 7. Documentation Updates

Update the tracked docs so they stop implying `conaryd` has an undefined
production deployment story.

Update at least:

- `deploy/FORGE.md`
- `docs/operations/infrastructure.md`
- release-hardening/audit docs as needed

The documentation should clearly say:

- `conaryd` is currently deployed on Forge as a local-only staging daemon
- verification is local to the host
- this is not a public production service

---

## Execution Order

Implement the design in this order:

1. add the checked-in Forge `conaryd` systemd unit
2. add Forge rollout manifest support for `conaryd`
3. extend rollout schema/orchestrator for systemd system units and local
   `conaryd` health verification
4. add the checked-in remote install/restart helper
5. update `deploy-and-verify.yml` to target Forge and remove
   `CONARYD_VERIFY_URL`
6. update Forge/infrastructure docs
7. rerun `deploy-and-verify` for the existing published `conaryd-v0.6.0`
   release using `source_run=24273700060`

---

## Success Criteria

This design is successful when all of the following are true:

- Forge has a checked-in, repeatable `conaryd` systemd unit
- Forge rollout config includes a `conaryd` unit and dedicated staging target
- the rollout framework can model systemd system units in addition to user
  units
- GitHub `deploy-and-verify` no longer depends on `CONARYD_VERIFY_URL`
- the GitHub release deploy lane installs the exact published `conaryd` bundle
  on Forge
- on-host verification confirms the expected version via
  `127.0.0.1:7890/health`
- a rerun against published release `conaryd-v0.6.0` succeeds on Forge
- the release-hardening checklist can move `conaryd` from
  "release published but deployment blocked" to
  "Forge staging deployment verified"

---

## Risks

- `conaryd` may need additional host setup on Forge beyond copying the binary,
  such as state directory ownership or policy-related runtime assumptions
- the systemd unit may expose path/permission mismatches that were never
  exercised in local development
- adding `systemd_system_unit` support to the rollout framework widens that
  framework's responsibility and needs careful test coverage
- an over-eager rollout-group change could accidentally start deploying
  `conaryd` during existing Forge workflows
- a successful Forge-local deployment could be misread as a production-service
  readiness signal unless docs and workflow comments are explicit

---

## Open Questions

These do not block the design, but they should be answered during planning:

1. Which exact Forge-side privileged operations should be granted via
   passwordless `sudo` to the deployment user?
2. Should the remote helper install the systemd unit on every deploy, or only
   when the checked-in unit file changes?
3. Should the localhost verification parse the JSON payload structurally or use
   a simpler version/status substring assertion to avoid extra host
   dependencies?

---

## Recommended Next Step

Write an implementation plan for the Forge-first `conaryd` staging deployment
that:

- scopes the rollout-framework changes separately from the workflow changes
- defines the exact checked-in files to add or modify
- includes a rerun of `deploy-and-verify` against the already-published
  `conaryd-v0.6.0` release as the validation step
