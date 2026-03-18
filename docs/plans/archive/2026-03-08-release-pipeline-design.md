# Release Pipeline Design

## Problem

After `scripts/release.sh` tags a release, there is no automated path to build
distributable packages, upload them to Remi, and make them available for
`conary self-update` or `conary install conary`. Native distro packages
(RPM/DEB/Arch) in `packaging/` are stale at 0.1.0. The three integration test
containers cannot verify self-update against real published packages.

## Decision

Single Forgejo CI workflow on Forge, triggered by `v*` tags. Builds the CCS
self-update package and all three native distro packages, then publishes
everything to Remi. A manual script provides the same flow for ad-hoc use.

## Components

### 1. CI Workflow (`.forgejo/workflows/release.yaml`)

**Trigger:** Tag push matching `v*` (conary releases only -- server/erofs tags
don't produce distributable binaries).

**Jobs:**

1. **build-release** -- `cargo build --release`, cache binary as artifact.
2. **build-ccs** -- `packaging/ccs/build.sh`, produces `conary-{version}.ccs`.
3. **build-native** -- Matrix `[rpm, deb, arch]`, each runs
   `packaging/{format}/build.sh` via existing Podman Containerfiles.
4. **publish** -- Depends on build-ccs + build-native. SSHes to Remi:
   - Copy CCS to `/conary/self-update/conary-{version}.ccs`
   - POST CCS to `/v1/{distro}/packages` for fedora, ubuntu, arch
   - Copy all artifacts to `/conary/releases/{version}/`
   - Generate `SHA256SUMS`
   - Update `latest` symlink
   - Smoke test `/v1/ccs/conary/latest`

**Credentials:** Forgejo repository secret `REMI_SSH_KEY` for Forge->Remi SSH.

### 2. Manual Script (`scripts/publish-release.sh`)

Run on Forge for ad-hoc publishing or re-publishing failed releases.

```bash
cd ~/Conary
./scripts/publish-release.sh [--version 0.3.0] [--skip-build] [--dry-run]
```

**Steps:**

1. Determine version from `Cargo.toml` (or `--version` override).
2. Build release binary (`cargo build --release`).
3. Build CCS package (`packaging/ccs/build.sh`).
4. Build native packages via Podman (`packaging/{rpm,deb,arch}/build.sh`).
5. Upload CCS to Remi `self-update/` directory.
6. Publish CCS as regular package to Remi API for all 3 distros.
7. Copy native packages to Remi releases directory.
8. Smoke test: verify `/v1/ccs/conary/latest` returns correct version.

**Flags:**

- `--skip-build` -- use existing artifacts in `packaging/*/output/`
- `--dry-run` -- show what would be uploaded
- `--version` -- override version detection

### 3. Remi-side Package Serving

**Self-update (existing, no changes):** CCS files in
`/conary/self-update/conary-{version}.ccs` served by existing handler endpoints.

**CCS as regular package (existing API):** `POST /v1/{distro}/packages` accepts
CCS uploads. Users can `conary install conary` / `conary update conary`.

**Native packages (new):** Static directory served by Remi:

```
/conary/releases/
  latest -> 0.3.0/              # symlink
  0.3.0/
    conary-0.3.0.ccs
    conary-0.3.0-1.fc43.x86_64.rpm
    conary_0.3.0-1_amd64.deb
    conary-0.3.0-1-x86_64.pkg.tar.zst
    SHA256SUMS
```

### 4. Integration Test Verification

Existing self-update tests (T72-T76) cover `update-channel` and
`self-update --check`. With real packages on Remi, these tests now verify
against live data. No new tests needed -- `--check` validates the endpoint
returns the correct version without replacing the binary mid-test.

## Release Flow

```
Developer                    Forge CI                      Remi
────────                    ────────                      ────
1. scripts/release.sh
   - Bump versions
   - Update CHANGELOG
   - Commit + tag
   - git push --tags
                            2. release.yaml triggered
                               - cargo build --release
                               - Build CCS package
                               - Build RPM/DEB/Arch (Podman)
                            3. Publish to Remi via SSH
                               - self-update/ (CCS)
                               - /v1/{distro}/packages (CCS)
                               - /releases/{version}/ (all)
                               - SHA256SUMS + latest symlink
                               - Smoke test
                                                          4. Live:
                                                             - self-update serves new version
                                                             - conary install conary works
                                                             - /releases/latest/ has all pkgs
```

## What's New

- `scripts/publish-release.sh` -- manual publish script
- `.forgejo/workflows/release.yaml` -- automated CI workflow
- Forgejo secret `REMI_SSH_KEY` -- SSH credential

## What's Unchanged

- `scripts/release.sh` -- versioning and tagging
- `packaging/ccs/build.sh` -- CCS package build
- `packaging/{rpm,deb,arch}/build.sh` -- native package builds
- Remi self-update endpoints -- serve from `self-update/`
- Remi package API -- accepts CCS uploads

## Not In Scope

- Distro-native repos (yum/apt/pacman repositories) -- Phase 3
- Automatic rollback on publish failure -- manual re-run
- Multi-arch builds -- x86_64 only
- Server/erofs crate releases -- no distributable binaries
