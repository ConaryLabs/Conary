---
last_updated: 2026-08-13
revision: 25
summary: Non-secret infrastructure, agent-operations transport, release, Remi deploy, TLS renewal, remote development, and retired Forge staging guidance for Conary contributors and coding assistants
---

# Infrastructure Overview

## Host Roles

- Remi is the production package service behind `https://remi.conary.io`.
- Direct SSH access for the Remi host uses `ssh.conary.io`, not the proxied
  public HTTPS hostnames.
- Remi currently runs Arch Linux on the Hetzner origin. Host-level
  package-manager notes should assume `pacman` unless a future migration
  updates this document. The Remi host OS is independent of the public client
  distro support matrix, which is Fedora 44, Ubuntu 26.04 LTS, and Arch Linux
  for the limited preview.
- Forge remote validation and Forge-local staging deployment are decommissioned.
  The old VPS runner did not expose `/dev/kvm`, and no replacement Forge host
  or conary-test deployment path is supported.
- Hosted CI keeps Remi health/audit/build/list checks active. QEMU release
  evidence comes from `scripts/local-qemu-validation.sh` on a local
  development machine with `/dev/kvm`.
- Sensitive usernames, credentials, or workstation-only shortcuts belong in the
  ignored `docs/operations/LOCAL_ACCESS.md`, not in tracked docs.

## Agent Operations And MCP

Remi's `/mcp` is the live MCP surface and is modern-only stateless Streamable
HTTP. `conary-test` is a local CLI and integration-test engine; it no longer
binds an HTTP or MCP listener. The framework-neutral compliance and raw-adapter
proof remains in `crates/conary-mcp`, while Remi owns live MCP behavior.

Prefer MCP resources for read-only state inspection and MCP tools for audited
mutations. MCP is the adapter, not the durable product contract:

The first LLM-native operations milestone may define prompt catalogs in
`conary-agent-contract`, but it must not register new live MCP prompts until
the stateless MCP adapter decision is satisfied.
The transport-neutral contract lives in `crates/conary-agent-contract`;
`crates/conary-mcp` remains MCP-specific adapter glue.

- Remi admin and package-service operations
- `conary-test` local run control, deploy/restart flows, image management, and fixture publishing

### Local Packaging MCP

`conary mcp packaging` starts the local stdio MCP server for packaging agent
workflows. It does not open a network listener. The first mutation contract is
confirmed static artifact publish through `conary.packaging.publish.plan` and
`conary.packaging.publish.apply`; Remi publish apply and project-form publish
apply are intentionally unsupported in this slice.

Use manual SSH, rsync, or curl only when the structured operation surface does
not cover the task or when you are debugging the underlying service path itself.

## Safe Public And Admin Endpoints

- Public package web UI and authenticated MCP endpoint:
  `https://remi.conary.io`
- Direct SSH hostname for the Remi origin host: `ssh.conary.io`
- Remi admin origin API: `http://localhost:8082` via SSH tunnel or direct
  origin access
- Remi OpenAPI spec: `http://localhost:8082/v1/admin/openapi.json` via SSH
  tunnel or direct origin access
- `conary-test` has no network endpoint. Use its local `health` and `deploy
  status` CLI commands for local/Remi checks.

## Source Deploy Patterns

### Forge

Forge staging, conary-test deployment, and the managed rollout path are
decommissioned. No Forge host, remote test-service deployment, or rollout
command is supported. Use the local QEMU/KVM validation gate and hosted Remi
checks for current evidence; historical Forge artifacts are not an active host
workflow.

### Remi

- Use the direct origin hostname `ssh.conary.io` for SSH and rsync.
- Use the normal admin account (`peter@ssh.conary.io`) plus passwordless,
  least-privilege `sudo`; root SSH login is not part of the supported deploy
  path.
- Exclude `target/`, `.git/`, and `.worktrees/`
- The durable deploy entry point is the root-owned helper installed at
  `/usr/local/sbin/conary-remi-deploy`, with the sudo policy tracked in
  `deploy/sudoers/remi`. The helper owns privileged actions for publishing
  Conary release artifacts and performing recoverable Remi service transitions.
- Normal Remi binary replacement is driven by GitHub Actions
  `release-build` -> `deploy-and-verify`. The workflow stages the built bundle
  and exact-tag repository manifest on the host, atomically self-updates the
  helper by SHA-256, then calls
  `/usr/local/sbin/conary-remi-deploy deploy-remi`.
- A bounded pre-release hard-cut sequence that explicitly forbids an
  intermediate release uses `deploy-remi-candidate` instead. Its required
  full commit SHA must already be an ancestor of `origin/main`; the protected
  production environment builds that exact tree, records the binary digest,
  and uses the same recoverable helper, source manifest, repopulation proof,
  and public readiness checks. It creates no tag or release and is not a path
  for deploying an unmerged pull-request head.
- The candidate Remi binary owns config/schema preparation. It type-checks the
  current config and source manifest, installs exact parser authority,
  snapshots a current SQLite epoch or moves a retired epoch plus WAL/SHM into
  `/conary/deployment-backups/`, and emits the transition manifest used for
  automatic rollback. The pre-deploy database remains recoverable; retired
  schemas are not migrated in place.
- The helper creates `/conary/repository-keys` as a `conary:conary` mode-0700
  durable authority root before candidate preparation. The candidate
  atomically creates one complete targets/snapshot/timestamp key set under
  each exact manifest profile. Repeat deployments preserve those bytes.
  Existing wrong ownership or modes, partial or mismatched role pairs,
  symlinks, unexpected entries, and route-slug aliases fail before service
  activation. This directory is deliberately outside release rollback and
  deletion paths.
- After health succeeds, the deployment job polls
  `conary-remi-deploy inspect-remi --require-repopulated`. Success requires all
  configured sources to contain metadata, a complete signing role set for
  every exact source profile, and at least one validated converted artifact for
  every configured public profile; dispatch or a green health probe alone is
  not deployment proof.
- Conary release artifact publication through the same helper verifies the
  CI-produced `SHA256SUMS` file from the staging directory before installing
  files into `/conary/releases/<version>`. The helper copies that verified
  checksum file as release evidence, refuses symlinked trust inputs, and
  requires `<artifact>.ccs.sig` whenever a staged `.ccs` artifact is present.
- Large QEMU fixtures use the helper's bounded
  `publish-test-artifact <filename> <sha256> <staged-file>` operation after
  authenticated SSH staging under `/tmp`. It accepts only a plain basename and
  regular file, enforces Remi's 8 GiB limit, verifies the caller-pinned digest
  before publication, and creates an immutable `/conary/test-artifacts/`
  target atomically. Repeating the exact publication is idempotent; a
  same-name, different-digest replacement fails closed.
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
  static sites. `deploy/deploy-sites.sh` builds locally, stages the build output
  under `/tmp` on `peter@ssh.conary.io`, then asks
  `/usr/local/sbin/conary-remi-deploy deploy-site` to publish it into
  `/conary/site/` for `conary.io` or `/conary/web/` for `remi.conary.io`.
- Post-release public-frontend updates deploy from the exact `main` commit
  selected by the manually dispatched `deploy-site` workflow. Its required
  `target` choice publishes `site`, `packages`, or `both` through the
  repository-held production key. The workflow runs the relevant frontend
  checks, verifies the selected public home content and Remi API when
  applicable, and always verifies the branded status-aware 404 after a main
  site deployment.
- `deploy/configure-site-routing.sh` owns the host-side transition from the old
  SPA fallback to static routing. It requires one unambiguous nginx server for
  `conary.io` rooted at `/conary/site`, backs up the config, changes missing
  paths to `=404` with `/404.html` as the error body, validates nginx, and
  restores the backup if validation or reload fails.
- The package frontend is the one wired into Remi's tracked config via
  `[web].root = "/conary/web"`; the main site remains a separate static root on
  the same host
- The production certificate currently uses Certbot's standalone authenticator,
  so `/etc/letsencrypt/renewal-hooks/pre/10-nginx-stop` must stop nginx before
  an attempted renewal and
  `/etc/letsencrypt/renewal-hooks/post/90-nginx-start` must start it afterward,
  including after a failed attempt. Validate this host-local contract with
  `sudo certbot renew --dry-run --cert-name remi.conary.io --non-interactive --no-random-sleep-on-renew`;
  the 2026-07-16 production repair passed that simulation and restored public
  health for all three certificate names.

#### Remi Remote Development Workbench

The Remi host may also carry an isolated development workbench for Conary, but
that workbench is not part of the production service contract. Keep development
state under `/conary/dev`, use the unprivileged `conary-dev` account for
day-to-day work, and keep production service paths such as `/conary/web`,
`/conary/site`, `/conary/releases`, and systemd-owned Remi state out of dev
workflows.

When rebuilding the workbench from a privileged Remi shell, the non-secret
baseline is:

```bash
sudo pacman -S --needed base-devel git rustup clang mold nodejs npm fd github-cli bubblewrap tmux mosh
sudo useradd -m -d /conary/dev/home/conary-dev -s /bin/bash conary-dev
sudo install -d -o conary-dev -g conary-dev /conary/dev/src
sudo install -d -o conary-dev -g conary-dev /conary/dev/cache/cargo /conary/dev/cache/rustup /conary/dev/cache/npm /conary/dev/cache/target
sudo loginctl enable-linger conary-dev
```

After the account exists, clone the repository as `conary-dev` into
`/conary/dev/src/Conary`, set `CARGO_HOME`, `RUSTUP_HOME`, npm cache, and target
cache paths under `/conary/dev/cache`, install Rust through rustup, and install
the assistant CLIs without version pinning:

```bash
rustup toolchain install 1.97.1 --profile default
rustup default 1.97.1
npm install -g @openai/codex @anthropic-ai/claude-code
```

The durable interactive entry point is a `dev` wrapper in
`/conary/dev/home/conary-dev/.local/bin/dev`. It should attach to a tmux session
named `conary` in `/conary/dev/src/Conary`, creating the session when absent.
Install `/usr/local/bin/dev` as a root-owned symlink or wrapper only after the
user-owned script exists. Enable tmux history and mouse support in the
`conary-dev` home directory rather than relying on workstation defaults.

Use `ssh.conary.io` for SSH transport. Workstation-specific aliases such as
`remi-dev`, `remi-work`, or mosh wrappers belong in the ignored
`docs/operations/LOCAL_ACCESS.md`; do not commit private key paths, access
tokens, recent-session history, or assistant cache directories. It is fine to
copy minimal assistant auth/config after reviewing it, but do not copy local
conversation history or package build artifacts wholesale. The remote Codex
GitHub MCP token, when present, belongs in a private env file such as
`/conary/dev/home/conary-dev/.config/codex/env`; Cloudflare MCP login remains an
interactive `codex mcp login cloudflare-api` step unless a future tracked helper
defines a safer bootstrap.

After a laptop rebuild, restore the SSH private key locally, recreate the
ignored SSH aliases from `docs/operations/LOCAL_ACCESS.md`, install `mosh` if
the workstation should use it, and connect with the tmux-attaching alias. If the
remote workbench itself is lost, rebuild the host packages, account, cache
directories, rustup toolchain, assistant CLIs, and `dev` wrapper before copying
any private auth material.

Do not overwrite the live Remi binary while `remi.service` is still running the
old process. That can fail with `Text file busy`.

## Release Flow

- GitHub Actions is the only long-term CI/CD control plane.
- The eight Cargo packages are code-ownership boundaries. The four artifact
  products are Conary, Remi, conaryd, and conary-test. One suite release owns
  their shared root `[workspace.package]` version, reviewed commit, tag, and
  GitHub release. All members inherit `publish = false`; there is no parallel
  crates.io release track.
- Run `./scripts/release.sh suite --dry-run` to inspect the next version, or
  pass an exact decision as `--target MAJOR.MINOR.PATCH`. The target must be an
  increasing `MAJOR.MINOR.PATCH` version for the complete suite.
- Run `./scripts/release.sh suite --prepare-only --target VERSION` on the
  issue-linked release branch. Preparation updates the root version, inherited
  workspace lock state, Conary native/CCS packaging, generated man page, and
  suite changelog, but creates no commit or tag.
- After exact-head CI and review complete, merge the preparation PR and prove
  local `main`, `origin/main`, and remote `main` agree. Only then create the
  annotated `vMAJOR.MINOR.PATCH` tag at that reviewed commit and push it. The
  active `Protect suite tags` ruleset permits creation of `v*` tags but rejects
  their update or deletion. Live release construction rejects a tag commit
  that is not reachable from `origin/main` and revalidates the remote tag
  immediately before draft mutation and publication.
- Product-prefixed tags remain immutable historical evidence for their exact
  trees. They are not current baselines, version inputs, or workflow routes.
- `release-build` constructs all four products from the exact suite tag,
  serializes their deployment modes in one schema-v1 metadata document with a
  typed rehearsal boolean, verifies raw and tar identities plus binary
  versions, generates one complete checksum set, and publishes one GitHub
  release only after every product bundle succeeds. Repository release
  immutability is enabled, so publishing the completed draft locks its tag and
  assets and creates a GitHub release attestation. Released-artifact proof must
  reject any draft or mutable release; closeout independently runs
  `gh release verify` and `gh release verify-asset`.
- `merge-validation` proves the current source tree through deterministic
  source, build, policy, and test checks. It must not probe mutable production
  endpoints, because production continues to serve the previously deployed
  release until the candidate passes this gate and is tagged.
- `deploy-and-verify` consumes that serialized metadata instead of re-deriving
  product behavior locally. It deploys and proves Remi first, then stages and
  proves Conary release assets and static sites from the same suite bundle.
- Within GitHub Actions, `deploy-and-verify` owns live contract proof after it
  deploys the exact tagged artifact. For Remi this includes both liveness and
  structured fail-closed readiness; independent production verification still
  follows the terminal workflow result.
- conaryd and conary-test are first-class suite artifacts with explicit
  `deploy_mode=none`; no runtime deployment job may start for either product.
- Native builders explicitly disable distro-default debug split packages
  because the suite defines no debug artifact product. Upload and bundle steps
  do not filter unexpected native outputs; an extra package fails exact asset
  validation. The RPM spec also opts out of Fedora's automatic debug-oriented
  Rust flags, manually reapplies the distro's frame-pointer, package-note, and
  native dependency flags, and leaves release codegen and stripping to the
  workspace Cargo profile.
- Exact-main `deploy-remi-candidate` remains available between suite releases
  for bounded hard cuts. It creates no tag or release and does not change suite
  version authority.
- Release verification is a GitHub workflow concern, not a Forgejo or
  Forge-hosted control-plane concern

## Contributor Notes

- Prefer the tracked docs for stable roles and workflows, and keep local-only
  access details in `docs/operations/LOCAL_ACCESS.md`, using
  [`docs/operations/LOCAL_ACCESS.example.md`](LOCAL_ACCESS.example.md) as the
  starting template
- For suite layout, phase selection, and manifest-run behavior, use
  [`docs/INTEGRATION-TESTING.md`](../INTEGRATION-TESTING.md)
- For conary-test validation, use `cargo run -p conary-test -- list` and the
  focused `run` commands in `docs/INTEGRATION-TESTING.md`.
- For assistant and contributor routing, use `AGENTS.md` and
  [`docs/llms/README.md`](../llms/README.md); Git history remains available for
  retired tool-specific context
