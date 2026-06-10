---
last_updated: 2026-05-24
revision: 8
summary: Non-secret infrastructure, agent-operations transport, release, Remi deploy, and Forge staging guidance for Conary contributors and coding assistants
---

# Infrastructure Overview

## Host Roles

- Remi is the production package service behind `https://remi.conary.io`.
- `https://packages.conary.io` remains the public compatibility alias and
  simple external health-check hostname for that same Remi service.
- Direct SSH access for the Remi host uses `ssh.conary.io`, not the proxied
  public HTTPS hostnames.
- Remi currently runs Arch Linux on the Hetzner origin. Host-level
  package-manager notes should assume `pacman` unless a future migration
  updates this document. The Remi host OS is independent of the public client
  distro support matrix, which is Fedora 44, Ubuntu 26.04 LTS, and Arch Linux
  for the limited preview.
- Forge remote validation is temporarily paused. The old VPS runner is being
  retired because it did not expose `/dev/kvm`, which made it unusable for
  scheduled QEMU release evidence.
- Until a replacement KVM-capable runner is registered, hosted CI keeps Remi
  health/audit/build/list checks active, and QEMU release evidence comes from
  `scripts/local-qemu-validation.sh` on a local development machine with
  `/dev/kvm`.
- Forge-local `conaryd` staging deployment is also paused while there is no
  active Forge host.
- Sensitive usernames, credentials, or workstation-only shortcuts belong in the
  ignored `docs/operations/LOCAL_ACCESS.md`, not in tracked docs.

## Agent Operations And MCP

Today, the live Remi MCP endpoint and the legacy `conary-test` `/mcp` endpoint
are session-based, tool-only surfaces. `conary-test` also exposes
`/mcp/stateless` as a draft stateless preview route with `server/discover`,
`resources/list`, and `resources/read` for
`conary-local://bootstrap/status` and `conary-test://suites`. Those resources
are read-only local bootstrap and suite-manifest state. The stateless preview
does not expose live tools, prompts, resource templates, subscriptions, SSE
streaming, mutations, or smoke execution.

Prefer MCP resources for read-only state inspection and MCP tools for audited
mutations. MCP is the adapter, not the durable product contract:

The first LLM-native operations milestone may define prompt catalogs in
`conary-agent-contract`, but it must not register new live MCP prompts until
the stateless MCP adapter decision is satisfied.
The transport-neutral contract lives in `crates/conary-agent-contract`;
`crates/conary-mcp` remains MCP-specific adapter glue.

Remi and legacy MCP endpoints remain session-based until stateless support is
intentionally expanded for those services.

- Remi admin and package-service operations
- `conary-test` run control, deploy/restart flows, image management, and fixture publishing

Use manual SSH, rsync, or curl only when the structured operation surface does
not cover the task or when you are debugging the underlying service path itself.

## Safe Public And Admin Endpoints

- Public package web UI and authenticated MCP endpoint:
  `https://remi.conary.io`
- Public package API and compatibility health alias:
  `https://packages.conary.io`
- Direct SSH hostname for the Remi origin host: `ssh.conary.io`
- Remi admin origin API: `http://localhost:8082` via SSH tunnel or direct
  origin access
- Remi OpenAPI spec: `http://localhost:8082/v1/admin/openapi.json` via SSH
  tunnel or direct origin access
- Forge-local `conary-test` health endpoint, when a replacement runner exists:
  `http://127.0.0.1:9090/v1/health`
- Forge-local `conary-test` deploy-status endpoint, when a replacement runner
  exists: `http://127.0.0.1:9090/v1/deploy/status`

## Source Deploy Patterns

### Forge

- **Paused:** these commands describe the next Forge runner, not an active host.
  Do not treat them as release evidence until a KVM-capable runner with
  `/dev/kvm` is registered.
- Preferred deployment path is managed rollout orchestration through
  `conary-test deploy rollout`
- From an operator workstation, use
  `FORGE_HOST=peter@replacement.example ./scripts/deploy-forge.sh --group control_plane --ref main`
  for the trusted default path after a replacement host exists
- `scripts/deploy-forge.sh` currently requires `FORGE_HOST`; it has no default
  while the old Forge host is retired
- `--ref` is the normal supported source mode and resolves an exact GitHub ref
  on Forge before build/restart/verify
- `--path` remains available for debug/local-snapshot deploys; the wrapper keeps
  the rsync boundary by syncing directly over the active Forge checkout before
  invoking the managed rollout there
- Rollout groups live in `deploy/forge-rollouts.toml`
- `conary-test deploy status --json` now reports both live binary truth and the
  last successful managed rollout, including explicit drift flags
- For supported control-plane verification, run `bash scripts/forge-smoke.sh`
- For trusted-runner runtime verification, run
  `bash scripts/forge-preflight.sh --mode container` before container suites
  and `bash scripts/forge-preflight.sh --mode qemu` before QEMU suites.
  QEMU mode requires `/dev/kvm`; Forge runners without exposed KVM are infra
  blockers for scheduled QEMU validation, not product pass evidence.
- Container-heavy Forge validation should reclaim inactive rootless Podman
  storage with `bash scripts/forge-container-cleanup.sh`; scheduled deep/QEMU
  CI does this before starting the matrix.
- Port resolution for CLI and smoke checks is `--port` > `CONARY_TEST_PORT` >
  `9090`
- Forge runtime repair should use
  `sudo bash /home/peter/Conary/deploy/repair-forge-runtime.sh`; this refreshes
  Podman/QEMU tooling and the rootless Podman socket without re-registering the
  GitHub Actions runner
- `conaryd` staging deployment is paused while Forge is retired. The release
  matrix marks `conaryd` as `deploy_mode=none` until a replacement staging host
  is available.
- The dormant Forge-local verifier is `scripts/conaryd-health.sh`, which probes
  `/run/conary/conaryd.sock` rather than a public network endpoint.
- The tracked Forge bootstrap trust for that path lives in
  `deploy/ssh/forge-known-hosts` and `deploy/sudoers/conaryd-forge`

### Remi

- Use the direct origin hostname `ssh.conary.io` for SSH and rsync.
- Use the normal admin account (`peter@ssh.conary.io`) plus passwordless,
  least-privilege `sudo`; root SSH login is not part of the supported deploy
  path.
- Exclude `target/`, `.git/`, and `.worktrees/`
- The durable deploy entry point is the root-owned helper installed at
  `/usr/local/sbin/conary-remi-deploy`, with the sudo policy tracked in
  `deploy/sudoers/remi`. The helper owns privileged actions for publishing
  Conary release artifacts, replacing the Remi binary, and applying operational
  Remi concurrency config.
- Normal Remi binary replacement is driven by GitHub Actions
  `release-build` -> `deploy-and-verify`. The workflow stages the built bundle
  on the host, then calls `/usr/local/sbin/conary-remi-deploy deploy-remi`.
- When the workflow updates Remi conversion concurrency during a binary deploy,
  it calls `configure-concurrency ... --skip-restart` before `deploy-remi` so
  the rollout performs one service restart and one health check.
- Conary release artifact publication through the same helper verifies the
  CI-produced `SHA256SUMS` file from the staging directory before installing
  files into `/conary/releases/<version>`. The helper copies that verified
  checksum file as release evidence, refuses symlinked trust inputs, and
  requires `<artifact>.ccs.sig` whenever a staged `.ccs` artifact is present.
- Bootstrap or repair deploy access once from an existing privileged shell with
  `sudo scripts/install-remi-deploy-access.sh`. It installs
  `deploy/remi-deploy-helper.sh` to `/usr/local/sbin/conary-remi-deploy`,
  installs `deploy/sudoers/remi` to `/etc/sudoers.d/remi`, and validates the
  sudoers file with `visudo -cf`.
- After bootstrap, `ssh peter@ssh.conary.io 'sudo -n /usr/local/sbin/conary-remi-deploy verify-access'`
  should succeed without prompting for a password.
- `scripts/rebuild-remi.sh` is retired for production deploys. It now fails
  closed and points operators back to the GitHub release/deploy flow and the
  root-owned helper.
- Host-local credential files such as ignored `deploy/.credentials.toml` are not
  canonical deployment instructions; tracked operations docs and deploy helpers
  are the source of truth.
- The public frontends currently share the Remi host but deploy as two separate
  static sites:
  `conary.io` syncs to `/conary/site/`, while `remi.conary.io` syncs to
  `/conary/web/`
- The package frontend is the one wired into Remi's tracked config via
  `[web].root = "/conary/web"`; the main site remains a separate static root on
  the same host
- `packages.conary.io` should be treated as the public compatibility alias for
  the same Remi origin, not as a separate host or deployment target

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
  deployable products (`conary` and `remi`)
- The `conaryd` release track is build-and-release only until a replacement
  staging host exists; its release matrix entry remains `deploy_mode=none`
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
