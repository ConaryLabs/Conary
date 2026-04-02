# Infrastructure & CI

Two servers, one CI system.

## Remi (Production)

- **SSH:** `ssh remi` or `ssh root@ssh.conary.io`
- **URL:** `https://packages.conary.io` (Cloudflare proxied)
- **Hardware:** Hetzner, 12 cores, 64GB RAM, 2x 1TB NVMe RAID 0
- **OS:** Ubuntu 24.04, Rust 1.94
- **Storage:** 1.7TB at `/conary` (ext4 on LVM)
- **Deploy:** Auto-deployed by Forgejo CI on push to main (after tests pass). Manual: `scripts/rebuild-remi.sh` on Remi, or `scripts/deploy-forge.sh` locally for Forge.
- **Sites:** `./deploy/deploy-sites.sh [site|packages|both]` — NEVER deploy `site/build/` to `/conary/web/`
- **Admin API:** `:8082` (external, bearer token auth) -- token management, CI proxy, SSE events, MCP endpoint
- **MCP endpoint:** `https://packages.conary.io:8082/mcp` (Streamable HTTP transport for LLM agents)
- **OpenAPI spec:** `https://packages.conary.io:8082/v1/admin/openapi.json`
- **Releases:** `/conary/releases/{version}/` with CCS, RPM, DEB, Arch packages + SHA256SUMS, `latest` symlink
- **Self-update:** `/conary/self-update/conary-{version}.ccs` (served by self-update handler)
- **Note:** Both `remi.conary.io` and `packages.conary.io` work through Cloudflare and point to the same origin

## Forge (CI/Test)

- **SSH:** `ssh peter@forge.conarylabs.com`
- **Hardware:** 8GB RAM, 151GB disk
- **OS:** Fedora 43, Rust 1.94, Podman 5.7
- **Forgejo:** v14.0.2 at `http://forge.conarylabs.com:3000`
- **Runner:** v12.7.1, label `linux-native`, host executor (not Docker)
- **Mirror:** GitHub `ConaryLabs/Conary` synced every 10 minutes
- **Source:** `/home/peter/Conary/`
- **Setup script:** `deploy/setup-forge.sh` (full automated install)
- **Setup docs:** `deploy/FORGE.md`

## CI Workflows

### GitHub Actions (`.github/workflows/`)

| Workflow | Trigger | Jobs | Duration |
|----------|---------|------|----------|
| `ci.yml` | Push to main/develop, PR | test, security | ~5 min |
| `release.yml` | Push `v*` tag | build-ccs, build-rpm, build-deb, build-arch, release + Remi deploy | ~15-20 min |

### Forgejo (`.forgejo/workflows/`)

| Workflow | Trigger | Jobs | Duration |
|----------|---------|------|----------|
| `ci.yaml` | Push to main, manual | build, test, clippy, remi-smoke, deploy-remi | ~5 min |
| `release.yaml` | Push `v*` tag | Verify release landed on Remi (waits for GH Actions) | ~3 min |
| `integration.yaml` | Push to main, manual | 3-distro Podman matrix (fedora43, ubuntu-noble, arch) | ~15 min |
| `e2e.yaml` | Daily 06:00 UTC, manual | 3-distro Phase 1+2 deep E2E | ~20-30 min |
| `remi-health.yaml` | Every 6 hours, manual | Full endpoint verification | ~60s |

**Trigger manually:** Forgejo API `POST /api/v1/repos/peter/Conary/actions/workflows/{name}/dispatches` with `{"ref":"main"}`.

**Force mirror sync:** `POST /api/v1/repos/peter/Conary/mirror-sync` (otherwise polls every 10m).

## Scripts

| Script | Purpose |
|--------|---------|
| `scripts/remi-health.sh --smoke` | Quick Remi check (5 endpoints) |
| `scripts/remi-health.sh --full` | Comprehensive Remi check (includes conversion) |
| `scripts/release.sh [conary\|remi\|conaryd\|conary-test\|all]` | Auto-version bump from conventional commits |
| `deploy/setup-forge.sh` | Install Forgejo + Runner on Forge |
| `deploy/deploy-sites.sh` | Deploy web content to Remi |
| `scripts/publish-test-fixtures.sh` | Publish test fixture CCS packages to Remi |
| `scripts/rebuild-remi.sh` | Pull, build, restart Remi server (runs on Remi) |
| `scripts/deploy-forge.sh` | Rsync source to Forge for testing |

## Manual Source Deploys

Use this when you want the latest source running on Forge or Remi without doing a tagged release.

Prefer the MCP deployment tools when they are available in-session:
- Forge: `deploy_source`, `rebuild_binary`, `restart_service`, `deploy_status`
- Remi: `ci_dispatch` / other admin tools when they cover the change you need

If MCP is unavailable, the current fallback playbook is manual rsync plus rebuild.

### Forge source deploy

Sync the local checkout to Forge:

```bash
./scripts/deploy-forge.sh
```

Then rebuild both binaries and restart the `conary-test` user service:

```bash
ssh peter@forge.conarylabs.com '
  set -euo pipefail
  cd ~/Conary
  cargo build -p conary-test
  cargo build
  systemctl --user restart conary-test
  sleep 2
  systemctl --user is-active conary-test
  curl -fsS http://127.0.0.1:9090/v1/health
'
```

Notes:
- This is a source refresh, not a release publish.
- Rsyncing over Forge's mirrored git checkout can leave `~/Conary` dirty. That is acceptable for ad hoc service refreshes.

### Remi source deploy

Remi uses `/root/conary-src/` as an rsync'd source tree. Sync the local checkout:

```bash
rsync -az --delete \
  --exclude target/ \
  --exclude '.git/' \
  --exclude '.worktrees/' \
  /home/peter/Conary/ remi:/root/conary-src/
```

Then build and replace the live binary:

```bash
ssh remi '
  set -euo pipefail
  if [ -f /root/.cargo/env ]; then . /root/.cargo/env; fi
  cd /root/conary-src
  cargo build --release -p remi
  systemctl stop remi
  install -m 755 target/release/remi /usr/local/bin/remi
  systemctl start remi
  sleep 3
  systemctl is-active remi
  curl -fsS http://127.0.0.1:8081/health
'
```

Important:
- Do not copy over `/usr/local/bin/remi` while `remi.service` is still running the old binary; that can fail with `Text file busy`.
- This updates the live server binary only. Public release artifacts still require the tagged release flow below.

## Release Pipeline

End-to-end flow: `release.sh` bumps versions and tags, GitHub Actions builds and deploys.

### Steps

1. **Bump:** `./scripts/release.sh conary` -- analyzes conventional commits, bumps all version files, updates CHANGELOG.md, commits, tags
2. **Push:** `git push origin main --tags` -- triggers `.github/workflows/release.yml`
3. **Build:** GitHub Actions builds 4 packages in parallel containers:
   - CCS (ubuntu-latest, `packaging/ccs/build.sh`)
   - RPM (Fedora 43 container, `packaging/rpm/build.sh`)
   - DEB (Ubuntu 24.04 container, `packaging/deb/build.sh`)
   - Arch (Arch Linux container, `packaging/arch/build.sh`)
4. **Release:** Creates GitHub Release with all artifacts + SHA256SUMS
5. **Deploy:** SSHes to Remi, uploads CCS to `/conary/self-update/`, all packages to `/conary/releases/{version}/`, updates `latest` symlink
6. **Verify:** Forgejo `release.yaml` waits 120s, then checks Remi self-update API and release directory

### Version Files (all bumped by `release.sh conary`)

| File | Field |
|------|-------|
| `apps/conary/Cargo.toml` | `version` |
| `crates/conary-core/Cargo.toml` | `version` |
| `Cargo.lock` | regenerated via `cargo generate-lockfile` |
| `packaging/rpm/conary.spec` | `Version:` |
| `packaging/arch/PKGBUILD` | `pkgver=` |
| `packaging/deb/debian/changelog` | prepends new entry |
| `packaging/ccs/ccs.toml` | `version` |
| `CHANGELOG.md` | prepends new section |

### Secrets

- `REMI_SSH_KEY` -- GitHub Actions secret, SSH private key for `root@ssh.conary.io`

## Integration Tests

- **Location:** `tests/integration/remi/` (T01-T76 Phase 1+2, Phase 3 adversarial)
- **Runner:** `conary-test` crate with TOML manifests in `tests/integration/remi/manifests/`
- **Config:** `tests/integration/remi/config.toml` (single source of truth)
- **Run:** `cargo run -p conary-test -- run --distro fedora43 --phase 1`
- **Run suite:** `cargo run -p conary-test -- run --suite phase1-core --distro fedora43 --phase 1`
- **Containers:** `tests/integration/remi/containers/Containerfile.{fedora43,ubuntu-noble,arch}`
- **Output:** JSON results in `tests/integration/remi/results/`
- **Full docs:** See `.claude/rules/integration-tests.md` and `docs/INTEGRATION-TESTING.md`

## Version Groups

Four independent version tracks with tag prefixes:

| Group | Tag prefix | Packages | Cargo.toml locations |
|-------|-----------|----------|---------------------|
| conary | `v` | conary + conary-core | `apps/conary/Cargo.toml`, `crates/conary-core/Cargo.toml` |
| remi | `remi-v` | remi | `apps/remi/Cargo.toml` |
| conaryd | `conaryd-v` | conaryd | `apps/conaryd/Cargo.toml` |
| conary-test | `test-v` | conary-test | `apps/conary-test/Cargo.toml` |
