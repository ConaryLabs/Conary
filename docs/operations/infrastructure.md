---
last_updated: 2026-08-26
revision: 34
summary: Non-secret infrastructure, agent operations, release, typed and causally inspectable Remi deployment completion, exact native-oracle input export, and current remote development tooling
---

# Infrastructure Overview

## Host Roles

- Remi is the production package service behind `https://remi.conary.io`.
- Direct SSH access for the Remi host uses `ssh.conary.io`, not the proxied
  public HTTPS hostnames.
- Remi runs Ubuntu 26.04 LTS on the Hetzner origin. The host OS is independent
  of the public client distro support matrix, which is Fedora 44, Ubuntu 26.04
  LTS, and Arch Linux for the limited preview. The destructive host procedure,
  storage contract, recovery boundary, and completion proof live in
  [`remi-host-rebuild.md`](remi-host-rebuild.md).
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
  and uses the same recoverable helper and source manifest. Dispatch must choose
  either `private-candidates` or `active-repopulation` completion. It creates no
  tag or release and is not a path for deploying an unmerged pull-request head.
- Candidate deployment retains exactly one final typed inspection instead of
  emitting every incomplete poll. Each canonical public profile includes its
  current candidate plus the exact latest fenced refresh run, typed state and
  failure stage/category, member progress, raw-evidence SHA-256, and a bounded
  redacted diagnostic copy. The protected job uploads that public-sanitized
  JSON and writes a concise typed summary even when completion fails. It does
  not expose service logs, generic shell access, credentials, bearer tokens,
  private-key paths, or host-local paths, and diagnostics do not satisfy the
  candidate publication predicate.
- The candidate Remi binary owns config/schema preparation. It type-checks the
  current config and source manifest, installs exact parser authority,
  snapshots a current SQLite epoch or moves a retired epoch plus WAL/SHM into
  `/conary/deployment-backups/`, and emits the transition manifest used for
  automatic rollback. The pre-deploy database remains recoverable; retired
  schemas are not migrated in place. The helper stops Remi before preparation,
  and the candidate independently enforces that quiescence: prepare acquires
  the same kernel-backed canonical runtime-root lock as the server before its
  first mutation. Transition-manifest schema 2 records that exact canonical
  root, and rollback reacquires it before restoring config, repository
  authority, or SQLite state. Live ownership fails immediately; service names,
  PIDs, lock-file contents, timestamps, and stale-file cleanup are not recovery
  authority. `deployment inspect` is read-only evidence and does not establish
  quiescence. Before invoking the root-run candidate, the helper creates the
  lock file as `conary:conary` mode 0600, or verifies an existing plain file has
  that exact access contract, so first deployment cannot strand a root-owned
  lock that the `User=conary` service cannot open.
- The helper creates `/conary/repository-keys` as a `conary:conary` mode-0700
  durable authority root before candidate preparation. The candidate
  atomically creates one complete targets/snapshot/timestamp key set under
  each exact manifest profile. Repeat deployments preserve those bytes.
  Existing wrong ownership or modes, partial or mismatched role pairs,
  symlinks, unexpected entries, and route-slug aliases fail before service
  activation. This directory is deliberately outside release rollback and
  deletion paths.
- After liveness succeeds, the deployment job polls the predicate selected by
  its explicit completion mode. `private-candidates` calls
  `conary-remi-deploy inspect-remi --require-private-candidates`; success means
  every configured public profile has an exact current, durable, nonempty
  private candidate whose immutable bundle and fenced repository bindings were
  reopened and revalidated. It proves no active pointer and accepts structured
  public readiness as either ready or intentionally unavailable.
  `active-repopulation` calls `inspect-remi --require-repopulated` and requires
  all configured public profiles to have populated active immutable catalogs,
  a complete signing role set, a fresh signed universe naming the exact same
  profile revisions, and at least one validated converted artifact pinned to
  every current revision. Mutable `repository_packages` rows are not evidence
  for either mode; dispatch or a green liveness probe alone is not deployment
  proof.
- Exact production native-oracle inputs use the root-owned helper operation
  `export-native-oracle-inputs <export-id> <fedora-sha256> <ubuntu-sha256>
  <arch-sha256>`. The helper fixes canonical public-profile order, invokes the
  typed `remi native-oracle-input` command as the service user, retains the
  durable independently reopened directory below
  `/conary/evidence/native-oracle-inputs/`, and stages a mode-0600 transport tar
  under `/tmp` for the authenticated caller. It grants no generic path,
  candidate-tier profile, native comparison, conversion, proof, activation, or
  pointer-mutation authority.
- Production R2 inventory and backfill use the manually dispatched
  `remi-r2-durability` workflow after its exact `commit_sha` is merged into
  `main` and deployed. The protected job enters through the normal Remi SSH
  boundary and calls the typed operation on the loopback-only admin listener;
  it does not copy an admin bearer token off the host. `plan` is read-only.
  `apply` fails unless a fresh post-upload R2 listing proves complete, and the
  retained artifact is aggregate `public-sanitized` evidence with diagnostic
  samples removed.
- After completeness is established, `[r2].enabled = true` is the single
  authority switch: startup requires usable R2 configuration, public chunk
  reads use presigned redirects, and local chunks are an R2-verified LRU cache
  bounded by `storage.max_cache_size`. Missing durable objects fail closed;
  operators do not restore retired redirect, write-through, threshold, or age
  flags.
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
  should succeed without prompting for a password. This operation verifies
  root execution and the installed Remi configuration only; it deliberately
  works before the first Remi binary or service start so a clean host does not
  have a circular bootstrap dependency.
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

The Remi host may also carry an isolated multi-project development workbench,
but that workbench is not part of the production service contract. Use the
unprivileged `dev` account for all interactive development. Keep production
service paths such as `/conary/web`, `/conary/site`, `/conary/releases`, and
systemd-owned Remi state out of development workflows. `conary` remains the
dedicated non-login service identity; do not recreate the retired
`conary-dev` or `signed-dev` interactive accounts.

When rebuilding the workbench from a privileged Remi shell, the non-secret
baseline is:

```bash
sudo apt-get update
sudo apt-get install -y build-essential git clang mold nodejs npm fd-find ripgrep gh bubblewrap tmux mosh
sudo useradd -m -d /data/dev/home -s /bin/bash dev
sudo install -d -o dev -g dev /data/dev/src
sudo install -d -o dev -g dev /data/dev/cache/cargo /data/dev/cache/rustup /data/dev/cache/npm /data/dev/cache/target
sudo loginctl enable-linger dev
```

After the account exists, clone repositories as `dev` under `/data/dev/src`,
set `CARGO_HOME`, `RUSTUP_HOME`, npm cache, and target cache paths under
`/data/dev/cache`, install Rust through rustup, and install
the assistant CLIs without version pinning:

```bash
rustup toolchain install 1.98.0 --profile default
rustup default 1.98.0
npm install -g @openai/codex @anthropic-ai/claude-code
```

Google agent work uses the supported Antigravity CLI through `agy`. Install it
from Google's current
[Antigravity distribution](https://antigravity.google/download), authenticate
interactively, and verify it with `agy --version`; do not add a second Google
CLI compatibility path to the repository. Workspace guidance for Antigravity
lives in `.agents/rules/conary.md` and routes back to the shared `AGENTS.md`
contract.

After Codex authentication, bootstrap its managed app-server for durable remote
control and automatic CLI updates:

```bash
codex app-server daemon bootstrap --remote-control
codex remote-control start --json
codex app-server daemon version
```

The upstream pid-backed daemon and updater survive logout but not a host reboot.
Install `deploy/systemd/codex-app-server-bootstrap.service` into `dev`'s user
manager so the idempotent upstream bootstrap runs at every boot:

```bash
runtime_dir="/run/user/$(id -u dev)"
sudo install -d -o dev -g dev -m 0755 \
  /data/dev/home/.config/systemd/user/default.target.wants
sudo install -o dev -g dev -m 0644 \
  deploy/systemd/codex-app-server-bootstrap.service \
  /data/dev/home/.config/systemd/user/codex-app-server-bootstrap.service
sudo -H -u dev env XDG_RUNTIME_DIR="$runtime_dir" systemctl --user daemon-reload
sudo -H -u dev env XDG_RUNTIME_DIR="$runtime_dir" \
  systemctl --user enable --now codex-app-server-bootstrap.service
```

`loginctl enable-linger dev` is part of the baseline above and is required for
the user manager to start without an interactive login. After a host reboot,
verify the unit is active, both managed Codex processes are owned by `dev`'s
user manager, and `codex remote-control start --json` reports `connected`
before treating Remote as recovered.

Register the clean Conary, Nomos, and The Mortal Estate checkout roots as Codex
projects through the running app-server's experimental project API. Project
registration is explicit; do not create dummy model turns merely to seed recent
working directories. Pair trusted clients with a short-lived
`codex remote-control pair` code and verify the project list from the paired
client. The rpm-rs fork remains a pinned Conary dependency while its upstream
work is pending, but it is not a Remi development project or persistent
checkout.

Claude Code Remote Control is directory-scoped rather than backed by a global
project registry. Authenticate `dev` with a full-scope `claude.ai` subscription
login, then run `claude` interactively once in each project root to accept the
workspace trust gate. Install the tracked template and enable one named server
instance per supported project:

```bash
runtime_dir="/run/user/$(id -u dev)"
sudo install -d -o dev -g dev -m 0755 \
  /data/dev/home/.config/systemd/user/default.target.wants
sudo install -o dev -g dev -m 0644 \
  deploy/systemd/claude-remote-control@.service \
  /data/dev/home/.config/systemd/user/claude-remote-control@.service
sudo -H -u dev env XDG_RUNTIME_DIR="$runtime_dir" systemctl --user daemon-reload
sudo -H -u dev env XDG_RUNTIME_DIR="$runtime_dir" systemctl --user enable --now \
  claude-remote-control@Conary.service \
  claude-remote-control@nomos.service \
  claude-remote-control@the-mortal-estate.service
```

Each instance uses Claude's worktree spawn mode so concurrent on-demand
sessions do not edit the same checkout, with a four-session capacity per
project. The pre-created session remains in the clean project root. Verify all
three services are active and that `Remi Conary`, `Remi nomos`, and
`Remi the-mortal-estate` appear in `claude.ai/code` before treating the remote
workbench as recovered.

Native Claude Code installations download updates in the background, but a new
version takes effect only when the process next starts. A host reboot starts the
template instances from the current binary. To apply an update sooner, finish
or detach active Remote Control work and explicitly restart the three service
instances; do not interrupt active sessions from an automatic update hook.

The durable interactive entry point is a `dev` wrapper in
`/data/dev/home/.local/bin/dev`. It should attach to a selected project tmux
session under `/data/dev/src`, creating the session when absent.
Install `/usr/local/bin/dev` as a root-owned symlink or wrapper only after the
user-owned script exists. Enable tmux history and mouse support in the
`dev` home directory rather than relying on workstation defaults.

Use `ssh.conary.io` for SSH transport. Workstation-specific aliases such as
`remi-dev`, `remi-work`, or mosh wrappers belong in the ignored
`docs/operations/LOCAL_ACCESS.md`; do not commit private key paths, access
tokens, recent-session history, or assistant cache directories. It is fine to
copy minimal assistant auth/config after reviewing it, but do not copy local
conversation history or package build artifacts wholesale. The remote Codex
GitHub MCP token, when present, belongs in a private env file such as
`/data/dev/home/.config/codex/env`; Cloudflare MCP login remains an
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
- A protected tag whose release construction fails remains reserved evidence;
  correct the cause in an issue-linked reviewed commit, prepare a strictly
  higher suite version, and create a new tag. Never move or reuse the failed
  tag.
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
- Conary's release bundle also owns `conary-bootstrap-v1.manifest` and its
  detached signature. The manifest binds the canonical tag/version plus exact
  Fedora 44 RPM, Ubuntu 26.04 DEB, and Arch x86_64 package basenames, sizes,
  and SHA-256 values. The public `/install-conary-preview.sh` endpoint embeds
  the matching release public key, verifies signed authority before host
  selection, defaults to preview, and requires `--apply --yes` for the native
  package transaction. The exact-tag released-artifact workflow proves that
  path inside a clean container for every supported host.
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
