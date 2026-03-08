# Infrastructure & CI

Two servers, one CI system.

## Remi (Production)

- **SSH:** `ssh remi` or `ssh root@ssh.conary.io`
- **URL:** `https://packages.conary.io` (Cloudflare proxied)
- **Hardware:** Hetzner, 12 cores, 64GB RAM, 2x 1TB NVMe RAID 0
- **OS:** Ubuntu 24.04, Rust 1.93
- **Storage:** 1.7TB at `/conary` (ext4 on LVM)
- **Deploy:** `rsync` source, `cargo build --release --features server`, copy binary, `systemctl restart remi`
- **Sites:** `./deploy/deploy-sites.sh [site|packages|both]` — NEVER deploy `site/build/` to `/conary/web/`
- **Note:** `remi.conary.io` does NOT work through Cloudflare; use `packages.conary.io`

## Forge (CI/Test)

- **SSH:** `ssh peter@forge.conarylabs.com`
- **Hardware:** 8GB RAM, 151GB disk
- **OS:** Fedora 43, Rust 1.93, Podman 5.7
- **Forgejo:** v14.0.2 at `http://forge.conarylabs.com:3000`
- **Runner:** v12.7.1, label `linux-native`, host executor (not Docker)
- **Mirror:** GitHub `ConaryLabs/Conary` synced every 10 minutes
- **Source:** `/home/peter/Conary/`
- **Setup script:** `deploy/setup-forge.sh` (full automated install)
- **Setup docs:** `deploy/FORGE.md`

## CI Workflows (`.forgejo/workflows/`)

| Workflow | Trigger | Jobs | Duration |
|----------|---------|------|----------|
| `ci.yaml` | Push to main, manual | build, test, clippy, remi-smoke | ~5 min |
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
| `scripts/release.sh [conary\|erofs\|server\|all]` | Auto-version bump from conventional commits |
| `deploy/setup-forge.sh` | Install Forgejo + Runner on Forge |
| `deploy/deploy-sites.sh` | Deploy web content to Remi |
| `scripts/publish-test-fixtures.sh` | Publish test fixture CCS packages to Remi |

## Integration Tests

- **Location:** `tests/integration/remi/` (76 tests: T01-T37 Phase 1, T38-T76 Phase 2)
- **Runner:** Python 3.11+ (`runner/test_runner.py`), stdlib-only
- **Config:** `tests/integration/remi/config.toml` (single source of truth)
- **Run on Forge:** `./tests/integration/remi/run.sh --build --distro fedora43`
- **Phase 2:** `./tests/integration/remi/run.sh --build --distro fedora43 --phase2`
- **Containers:** `tests/integration/remi/containers/Containerfile.{fedora43,ubuntu-noble,arch}`
- **Output:** JSON results in `tests/integration/remi/results/`
- **Full docs:** See `.claude/rules/integration-tests.md` and `docs/INTEGRATION-TESTING.md`

## Version Groups

Three independent version tracks with tag prefixes:

| Group | Tag prefix | Crates | Cargo.toml locations |
|-------|-----------|--------|---------------------|
| conary | `v` | conary + conary-core | `Cargo.toml`, `conary-core/Cargo.toml` |
| erofs | `erofs-v` | conary-erofs | `conary-erofs/Cargo.toml` |
| server | `server-v` | conary-server | `conary-server/Cargo.toml` |
