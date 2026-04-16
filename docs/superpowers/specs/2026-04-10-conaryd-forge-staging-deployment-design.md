---
last_updated: 2026-04-16
revision: 3
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
- local-only verification on Forge via a checked-in Unix-socket health
  verifier against `/run/conary/conaryd.sock`
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
  lane instead of installing the exact `release-build` bundle artifact for the
  published tag
- widening existing Forge rollout groups in a way that silently starts
  deploying `conaryd` during unrelated control-plane operations

---

## Repository Context

The repo currently describes `conaryd` as a local system daemon:

- [README.md](../../README.md) describes it as "a local daemon" that provides a
  REST API over a Unix socket, with optional TCP for local use
- [apps/conaryd/src/daemon/mod.rs](../../apps/conaryd/src/daemon/mod.rs)
  defaults the TCP listener to `127.0.0.1:7890`
- [apps/conaryd/src/daemon/socket.rs](../../apps/conaryd/src/daemon/socket.rs)
  currently rejects `enable_tcp`; the daemon only accepts Unix-socket
  connections today
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

Observed Forge host facts captured on 2026-04-10:

- `systemctl cat conaryd.service` reported no installed `conaryd.service`
- no `conaryd` OS user exists today on Forge
- `/run/conary` does not exist yet
- `/var/lib/conary` already exists and already contains `conary.db`
- Forge is currently running with SELinux in `Permissive` mode

These facts matter for the design:

- the spec cannot assume a pre-existing service, runtime directory, or service
  user on Forge
- the first staging pass should not invent an implicit database
  initialization/reset flow for `/var/lib/conary/conary.db`
- the privilege model must be explicit instead of silently inheriting whatever a
  future unit file happens to default to

---

## Decision

Use **Forge-first, local-only staging deployment** as the first real `conaryd`
host model.

This means:

- `conaryd` is deployed to Forge, not Remi
- `conaryd` remains local-only on Forge
- verification is done on-host over SSH against a checked-in Unix-socket health
  verifier
- GitHub release deployment installs the exact `release-build` bundle artifact
  associated with the published tag on Forge
- the first Forge staging unit runs as an explicit root-owned system service,
  because `conaryd` is a local package-management daemon and Forge already
  carries a root-owned `/var/lib/conary/conary.db`
- the first Forge staging pass intentionally accepts root-only on-host package
  mutation access through `conaryd`; broader non-root Forge daemon UX is a
  separate hardening/design track
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

### 1. Forge Host Preflight

Implementation must start by capturing and honoring the actual Forge host state.

Required preflight inputs:

- confirm the documented SSH operator account (`peter`) can execute the
  narrowed `sudo -n` commands this deployment needs
- confirm there is still no checked-in or hand-created `conaryd.service` on
  Forge; if one exists unexpectedly, abort before mutation and require an
  explicit operator-reviewed migration/diff plan rather than silently replacing
  it
- confirm `/var/lib/conary/conary.db` still exists and is preserved by the
  design
- confirm Forge remains a local-only staging host for `conaryd`, with no public
  DNS/TLS/reverse-proxy expectations
- record the current SELinux mode; on 2026-04-10 this is `Permissive`, so this
  spec does not add SELinux-specific policy work for the first staging pass

The design must fail closed if those preflight assumptions are not true at
execution time.

These preflight checks must be encoded in a checked-in execution path, not left
as operator folklore. The helper's first phase, or a dedicated checked-in
preflight script it calls, must assert these conditions before any host
mutation.

### 2. Host Role And Topology

Forge becomes the first real `conaryd` host, but explicitly as staging.

Target runtime shape:

- host: Forge (`forge.conarylabs.com`)
- service class: systemd **system** unit
- binary path: `/usr/local/bin/conaryd`
- runtime user: `root` for the first staging pass, explicitly set in the unit
- Unix socket path: `/run/conary/conaryd.sock`
- DB path: `/var/lib/conary/conary.db`
- no TCP listener in the first staging pass
- exposure: local-only

The daemon is not public on Forge:

- no public DNS entry for `conaryd`
- no reverse-proxy exposure
- no external `VERIFY_URL`

Success is defined by on-host checks:

1. `systemctl is-active conaryd` returns active
2. `scripts/conaryd-health.sh --expected-version <version>` succeeds locally on
   Forge
3. the verifier proves the daemon reports the expected release version over the
   Unix socket

Because TCP listener support is not yet implemented in the current daemon, this
design does **not** depend on loopback TCP for the first truthful staging
deploy. If Forge-local TCP health checks are still wanted later, that requires
separate daemon implementation work and is out of scope here.

The first staging pass does **not** attempt a new least-privilege service-user
model. That would be a separate hardening effort and should not be quietly
invented inside the deployment spec.

Because the daemon runs as `root` in this first staging pass and its Unix-socket
API gate only admits `root` or the daemon's own UID for `/v1` operations, this
design should explicitly treat Forge-local package mutation through `conaryd` as
root-only for now. Health verification remains local-only, but broader non-root
operator UX on Forge is not part of this staging deployment.

### 3. Forge Service Definition

Add a checked-in systemd unit file:

- `deploy/systemd/conaryd.service`

The unit should explicitly include:

- `Type=notify`, because `conaryd` already speaks `sd_notify`
- `NotifyAccess=main`
- `TimeoutStartSec=180`
- `User=root`
- `Group=root`
- `RuntimeDirectory=conary`
- `StateDirectory=conary`
- installed on the host at `/etc/systemd/system/conaryd.service`
- `ExecStart=/usr/local/bin/conaryd --db /var/lib/conary/conary.db --socket /run/conary/conaryd.sock`
- `Restart=on-failure`

The unit must not pass `--tcp` in the first staging pass because TCP listener
support is not yet implemented in the current daemon.

The service must be valid for a host where `conaryd` does meaningful work but
is still intentionally non-public.

### 4. Local Health Contract

Add a checked-in local health verifier:

- `scripts/conaryd-health.sh`

This health contract must be concrete, not substring-based.

The verifier interface must also be concrete. It should accept the expected
release version explicitly, for example:

```bash
scripts/conaryd-health.sh --expected-version 0.6.0
```

The checked-in implementation should query the daemon through the Unix socket,
for example with:

```bash
curl --unix-socket /run/conary/conaryd.sock http://localhost/health
```

or an equivalent checked-in client path with the same contract.

The expected version passed to the verifier must come from
`metadata.json.version` for the exact `source_run` bundle being deployed. A
version mismatch must fail closed.

Verification succeeds only when all of the following are true:

1. `systemctl is-active conaryd` reports `active`
2. `GET /health` over `/run/conary/conaryd.sock` returns HTTP 200
3. the response parses as JSON
4. the JSON contains `status == "healthy"`
5. the JSON contains `version == <expected release version passed to the verifier>`

On failure, the verifier should print enough context to distinguish:

- service-not-running
- Unix-socket-unreachable
- malformed response
- version mismatch

This should be implemented as a checked-in script or equivalent checked-in
verification path, not as an underspecified "grep for the version somewhere in
the payload."

Because the first staging service runs as `root`, the verifier is expected to
run as `root` or via `sudo -n` on Forge.

`/health` is a staging liveness/version gate, not a full production-readiness
signal.

### 5. GitHub Release Deploy Lane

The GitHub `release-build` lane stays artifact-oriented and should continue to
publish the canonical `conaryd` tarball and metadata.

The `deploy-and-verify` lane for `conaryd` should change from:

- "copy tarball to an unspecified host and curl a secret-provided verify URL"

to:

- "download the exact `source_run` bundle artifact and metadata, stage the
  matching checked-in deploy assets from the correct trusted repo revision, copy
  them to Forge, install via a checked-in helper, and verify locally on-host"

Update:

- `.github/workflows/deploy-and-verify.yml`

Because the workflow needs checked-in helper/verifier/unit files and the pinned
Forge host-trust artifact from the repo, it should include a checkout step
before staging deploy assets.

Keep secrets:

- `CONARYD_SSH_KEY`
- `CONARYD_SSH_TARGET`

Remove the public verification abstraction:

- delete `CONARYD_VERIFY_URL`

The deploy lane should:

1. download the release-build artifact tarball from the exact `source_run`
2. read `tag_name` and `version` from the serialized metadata
3. fetch the checked-in deploy assets from the correct trusted repo revision
4. compute the expected SHA-256 of the tarball on the runner
5. SCP the tarball, expected hash, checked-in helper, checked-in health
   verifier, and checked-in unit file to Forge
6. invoke the checked-in remote install/restart helper on Forge over SSH,
   passing the expected version explicitly
7. have the helper recompute the tarball hash before install
8. verify `conaryd` on Forge with the checked-in localhost health contract

The workflow must install the exact `source_run` bundle artifact for the
published tag, not rebuild from source on Forge. Release deployment should
verify the shipped binary, not a fresh source build.

For releases cut **after** this design lands, the deploy assets should come from
the same tagged repo revision as the bundle being deployed.

There is one explicit bootstrap exception for the already-published
`conaryd-v0.6.0` bundle: because that tag predates
`scripts/install-conaryd-on-forge.sh`, `scripts/conaryd-health.sh`, and
`deploy/systemd/conaryd.service`, the one-time rerun against
`source_run=24273700060` may stage those deploy assets from the implementation
revision that first introduces them while still installing the exact historical
`v0.6.0` bundle. That bootstrap exception exists only to establish the first
truthful Forge staging deployment for the already-published bundle. It should
not remain in place for future releases.

The workflow path should call the dedicated Forge helper directly. It must not
depend on pre-existing `conary-test deploy rollout` support already being live
on Forge before the first truthful `conaryd` deploy can happen.

The workflow must also make the helper path executable in practice. A fresh
Forge host cannot be expected to already have `scripts/install-conaryd-on-forge.sh`,
`scripts/conaryd-health.sh`, or `deploy/systemd/conaryd.service` at the correct
revision unless the workflow stages those files explicitly.

### 6. Remote Install And Verify Helper

To keep the workflow readable and keep privileged host logic versioned in the
repo, add a checked-in helper script for the Forge-side install/restart flow.

Suggested path:

- `scripts/install-conaryd-on-forge.sh`

The workflow must copy the following checked-in files to Forge from the same
trusted repo revision it selected under Section 5:

- `scripts/install-conaryd-on-forge.sh`
- `scripts/conaryd-health.sh`
- `deploy/systemd/conaryd.service`

The helper should be responsible for:

- validating the uploaded tarball hash before unpack/install
- unpacking the tarball into a temp directory
- staging the new binary without rebuilding from source
- preserving the previous `/usr/local/bin/conaryd` binary before replacement
- preserving the previous live unit file at `/etc/systemd/system/conaryd.service`
  if one exists before replacement
- installing or refreshing the checked-in systemd unit if needed
- reloading systemd when the unit changes
- treating "no existing conaryd.service loaded" as an expected first-install
  condition rather than an automatic failure
- restarting `conaryd`
- running the checked-in local health verifier with the expected version passed
  explicitly, for example `scripts/conaryd-health.sh --expected-version "$VERSION"`

The helper must also define the failure path:

- if install or restart fails, exit non-zero
- if restart fails after the new binary or unit has been put in place, restore
  the previous binary and previous unit when available, reload systemd, attempt
  to restart the previous service, and then exit non-zero
- if local verification fails after the new binary is in place, restore the
  previous binary and previous unit when available, reload systemd, restart the
  previous service, and then exit non-zero
- if there is no previous binary because this is the first install, remove the
  newly staged binary/unit before exiting and report that no rollback target
  existed
- do **not** auto-initialize, wipe, or replace `/var/lib/conary/conary.db`
  during this first staging deploy; if the existing database shape blocks the
  daemon, fail the deploy rather than inventing a reset/migration story inside
  the helper

On any failure path, the helper should print enough context to debug the host
state, including `systemctl status --no-pager conaryd` after the failed start or
verification attempt.

If rollback restores a previous binary/unit and successfully restarts the old
service, the helper should rerun the verifier against the restored service and
print whether the restore recovered a healthy daemon before exiting non-zero.

The GitHub workflow should call that helper over SSH rather than embedding the
entire host mutation flow inline in YAML.

### 7. Forge Rollout Modeling Follow-Up

Represent `conaryd` in the Forge rollout manifest so the host's checked-in
deployment model remains coherent.

Update:

- `deploy/forge-rollouts.toml`

Add:

- a future `conaryd` rollout unit only after the rollout framework can model
  bundle installation and privileged system-unit execution safely
- a dedicated rollout group such as `conaryd_staging`

Do **not** silently add `conaryd` to existing groups like `control_plane` on
the first pass. Existing Forge deployments should not start mutating a new
system daemon as a side effect.

The rollout schema/orchestrator in `conary-test` is a distinct follow-up
workstream, not a one-line tweak and not part of the first truthful direct
deploy lane.

The current rollout framework must **not** be used to deploy `conaryd` by
teaching it only `systemd_system_unit` while still using
`build = { cargo_package = "conaryd" }`. That would reintroduce the exact
"rebuild from source on Forge" path this design rejects for release deployment.

If rollout-framework coherence work proceeds later, it should update at minimum:

- `apps/conary-test/src/deploy/manifest.rs`
- `apps/conary-test/src/deploy/plan.rs`
- `apps/conary-test/src/deploy/orchestrator.rs`
- `apps/conary-test/src/handlers.rs`
- `apps/conary-test/src/deploy/status.rs`
- tests in the same modules

That follow-up should learn **both** of the following, not just one:

- a bundle-install or external-helper deploy mode that can install the exact
  shipped artifact instead of cargo-building from source on Forge
- an explicit privileged execution handoff for systemd system-unit operations

This keeps the Forge rollout model honest:

- `conary-test` remains a user service
- `conaryd` becomes a system daemon

The rollout verification model should also learn a `conaryd`-appropriate local
health mode:

- `forge_smoke` for `conary-test`
- `conaryd_local_health` for `conaryd`

That verify mode should call the checked-in localhost health verifier and
should not depend on any public endpoint or secret-provided URL.

Until that follow-up lands, the direct GitHub helper path from Sections 5 and 6
remains the canonical deployment mechanism for `conaryd` on Forge.

### 8. Privilege Model

Forge is currently documented around the `peter` user, not raw root login.

The design should therefore assume:

- SSH target remains the documented Forge operator account
- privileged host mutations are performed through `sudo -n`, not raw root SSH
- the helper runs as `peter` and escalates only for the explicit filesystem and
  `systemctl` operations it needs
- Forge SSH host identity is pinned through a checked-in known-hosts artifact;
  the deploy lane must not rely on opportunistic `ssh-keyscan` plus
  `StrictHostKeyChecking=accept-new`

Host prerequisites for the deployment user:

- passwordless `sudo` for the specific `conaryd` install/restart operations the
  checked-in helper needs
- a checked-in `sudoers` snippet or equivalent documented bootstrap artifact
  that can be installed once on Forge by a root operator before the first live
  deploy

Suggested bootstrap artifact:

- `deploy/sudoers/conaryd-forge`
- `deploy/ssh/forge-known-hosts`

That one-time bootstrap is a host prerequisite for the truthful deploy lane. The
GitHub workflow should assume it is already installed; it should not try to edit
`sudoers` itself during deployment.

The workflow should consume the pinned host key via the checked-in file, for
example:

```bash
ssh -o UserKnownHostsFile=deploy/ssh/forge-known-hosts -o StrictHostKeyChecking=yes
```

If Forge presents a different host key, the workflow must fail before any SCP or
SSH mutation step.

The allowed command surface should stay narrow. It should cover only the
commands needed to:

- install or replace `/usr/local/bin/conaryd` and
  `/etc/systemd/system/conaryd.service`
- run `systemctl daemon-reload`, `systemctl restart`, `systemctl is-active`,
  and `systemctl status` for `conaryd`
- run the checked-in verifier as `root` or via a narrowly allowed wrapper so it
  can access `/run/conary/conaryd.sock`
- remove or restore the managed `conaryd` binary/unit during rollback when the
  helper detects a failed restart or failed verification

This is preferable to normalizing a root SSH target for the first staging pass.

### 9. Documentation Updates

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
- Forge staging verification does **not** mean `conaryd` now has production
  hosting, public DNS/TLS exposure, monitoring, or SLA commitments

---

## Execution Order

Implement the design in this order:

1. capture Forge host preflight facts and confirm the explicit root-owned
   system-service assumptions still hold
2. add the checked-in Forge `conaryd` systemd unit, checked-in Unix-socket
   health verifier, and checked-in deploy bootstrap artifacts
3. add the checked-in remote install/restart helper, including hash validation,
   explicit version passing, and rollback behavior for restart/verification
   failures
4. update `deploy-and-verify.yml` to target Forge, remove
   `CONARYD_VERIFY_URL`, stage the direct helper path, and enforce pinned Forge
   host-key trust
5. rerun `deploy-and-verify` for the existing published `conaryd-v0.6.0`
   release using `source_run=24273700060` and the one-time bootstrap exception
   defined in Section 5
6. decide separately whether to defer rollout-framework coherence entirely or to
   pursue it as a follow-up bundle-install design track
7. update Forge/infrastructure docs and release-hardening docs

---

## Success Criteria

This design is successful when all of the following are true:

- Forge host preflight is explicit and checked into the design, rather than
  assumed
- Forge has a checked-in, repeatable `conaryd` systemd unit
- GitHub `deploy-and-verify` no longer depends on `CONARYD_VERIFY_URL`
- GitHub `deploy-and-verify` uses pinned Forge host trust material instead of
  `ssh-keyscan`/`accept-new`
- the GitHub release deploy lane installs the exact `source_run` `conaryd`
  bundle for the published tag on Forge and verifies artifact integrity before
  install
- on-host verification confirms the expected version via the checked-in
  Unix-socket health verifier
- the remote helper defines and exercises a rollback path when install succeeds
  but restart or verification fails
- for future releases cut after this design lands, GitHub `deploy-and-verify`
  stages the helper, verifier, and unit from the same tagged repo revision as
  the bundle it is deploying
- the one-time bootstrap rerun against the published `conaryd-v0.6.0`
  tag/bundle succeeds on Forge under the explicit exception in Section 5
- the release-hardening checklist can move `conaryd` from
  "release published but deployment blocked" to
  "Forge staging deployment verified"
- Forge staging deployment verified is explicitly **not** treated as production
  readiness; the remaining gaps are still public hosting, DNS/TLS exposure,
  production monitoring/alerting, and a dedicated long-term host story

---

## Risks

- `conaryd` may need additional host setup on Forge beyond copying the binary,
  such as state directory ownership or policy-related runtime assumptions
- the systemd unit may expose path/permission mismatches that were never
  exercised in local development
- adding bundle-install plus privileged system-unit support to the rollout
  framework later would widen that framework's responsibility and needs careful
  test coverage
- `/health` can prove liveness and version but not the full correctness of all
  package-mutation paths
- the one-time `v0.6.0` bootstrap exception deliberately stages deploy assets
  from a newer repo revision than the bundle itself; that exception must remain
  bootstrap-only and not become the steady-state rule
- the root-owned first staging pass intentionally does not preserve non-root
  Forge operator UX through `conaryd`; that remains a separate hardening task
- the first staging pass intentionally keeps the existing root-owned database in
  place; if that database shape is incompatible, deployment should fail rather
  than trying to repair host state implicitly
- an over-eager rollout-group change could accidentally start deploying
  `conaryd` during existing Forge workflows
- a successful Forge-local deployment could be misread as a production-service
  readiness signal unless docs and workflow comments are explicit

---

## Open Questions

These are the only remaining planning-time questions that should need explicit
answers:

1. Which exact `sudoers` allowlist should back the narrowed helper command
   surface?
2. Should rollout-framework coherence be deferred entirely until a separate
   bundle-install design lands, or should this design reserve only a
   documentation/placeholder manifest step now?

---

## Recommended Next Step

Write an implementation plan for the Forge-first `conaryd` staging deployment
that:

- scopes the direct deploy-lane work separately from the rollout-framework
  coherence work
- defines the exact checked-in files to add or modify
- includes a rerun of `deploy-and-verify` against the already-published
  `conaryd-v0.6.0` release as the validation step
