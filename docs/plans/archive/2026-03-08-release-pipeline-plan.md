# Release Pipeline Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Automate building and publishing CCS + native distro packages to Remi after each release tag, so users can `conary self-update` and `conary install conary` on all 3 distros.

**Architecture:** A Forgejo CI workflow on Forge builds a release binary, packages it as CCS and 3 native formats (RPM/DEB/Arch) via Podman, then SSHes to Remi to deploy artifacts. A manual script provides the same flow.

**Tech Stack:** Bash, Podman, Forgejo Actions, SSH/rsync, Remi HTTP API

---

### Task 1: Fix stale packaging version in ccs.toml

The CCS manifest is hardcoded to 0.2.1 but the project is at 0.3.0. The `release.sh` script already updates this file, but the current state is stale.

**Files:**
- Modify: `packaging/ccs/ccs.toml:8`

**Step 1: Update the version**

Change line 8 from:
```toml
version = "0.2.1"
```
to:
```toml
version = "0.3.0"
```

**Step 2: Verify release.sh handles this file**

Run: `grep -n 'ccs.toml' scripts/release.sh`
Expected: Line ~208 shows `packaging/ccs/ccs.toml` is updated by `update_packaging_versions`.

**Step 3: Commit**

```bash
git add packaging/ccs/ccs.toml
git commit -m "chore: sync ccs.toml version to 0.3.0"
```

---

### Task 2: Create the manual publish script

**Files:**
- Create: `scripts/publish-release.sh`

**Step 1: Write the script**

```bash
#!/usr/bin/env bash
# scripts/publish-release.sh
#
# Build and publish a Conary release to Remi.
# Builds CCS + native packages, uploads to self-update + package API + releases dir.
#
# Usage:
#   ./scripts/publish-release.sh                    # Build and publish current version
#   ./scripts/publish-release.sh --version 0.3.0    # Override version
#   ./scripts/publish-release.sh --skip-build       # Use existing artifacts
#   ./scripts/publish-release.sh --dry-run           # Show what would happen
#
# Run on Forge (needs Podman for native builds, SSH access to Remi).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REMI_HOST="${REMI_HOST:-remi}"
REMI_ENDPOINT="${REMI_ENDPOINT:-https://packages.conary.io}"

# ── Parse arguments ──────────────────────────────────────────────────────────

DRY_RUN=false
SKIP_BUILD=false
VERSION=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run)    DRY_RUN=true; shift ;;
        --skip-build) SKIP_BUILD=true; shift ;;
        --version)    VERSION="$2"; shift 2 ;;
        --help)
            sed -n '3,/^$/s/^# //p' "$0"
            exit 0
            ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# ── Determine version ───────────────────────────────────────────────────────

if [[ -z "$VERSION" ]]; then
    VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
fi

echo "=== Publishing Conary v${VERSION} ==="
echo "  Remi host: $REMI_HOST"
echo "  Remi API:  $REMI_ENDPOINT"
echo "  Dry run:   $DRY_RUN"
echo ""

# ── Build artifacts ──────────────────────────────────────────────────────────

CCS_PKG="$REPO_ROOT/packaging/ccs/output/conary-${VERSION}.ccs"
RPM_PKG="$REPO_ROOT/packaging/rpm/output/conary-${VERSION}-1.fc43.x86_64.rpm"
DEB_PKG="$REPO_ROOT/packaging/deb/output/conary_${VERSION}-1_amd64.deb"
ARCH_PKG="$REPO_ROOT/packaging/arch/output/conary-${VERSION}-1-x86_64.pkg.tar.zst"

if ! $SKIP_BUILD; then
    echo "[1/6] Building release binary..."
    cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml"

    echo "[2/6] Building CCS package..."
    bash "$REPO_ROOT/packaging/ccs/build.sh"

    echo "[3/6] Building RPM package..."
    bash "$REPO_ROOT/packaging/rpm/build.sh" --podman

    echo "[4/6] Building DEB package..."
    bash "$REPO_ROOT/packaging/deb/build.sh" --podman

    echo "[5/6] Building Arch package..."
    bash "$REPO_ROOT/packaging/arch/build.sh" --podman
else
    echo "[1-5/6] Skipping builds (--skip-build)"
fi

# ── Verify artifacts exist ───────────────────────────────────────────────────

MISSING=0
for pkg in "$CCS_PKG"; do
    if [[ ! -f "$pkg" ]]; then
        echo "FATAL: Missing required artifact: $pkg" >&2
        MISSING=1
    fi
done

# Native packages are optional (best-effort)
for pkg in "$RPM_PKG" "$DEB_PKG" "$ARCH_PKG"; do
    if [[ ! -f "$pkg" ]]; then
        echo "WARN: Missing native package: $pkg (skipping)"
    fi
done

if [[ $MISSING -eq 1 ]]; then
    exit 1
fi

# ── Publish ──────────────────────────────────────────────────────────────────

echo "[6/6] Publishing to Remi..."

RELEASES_DIR="/conary/releases/${VERSION}"
SELF_UPDATE_DIR="/conary/self-update"

if $DRY_RUN; then
    echo "  [DRY RUN] Would create $RELEASES_DIR on $REMI_HOST"
    echo "  [DRY RUN] Would copy CCS to $SELF_UPDATE_DIR/conary-${VERSION}.ccs"
    echo "  [DRY RUN] Would POST CCS to $REMI_ENDPOINT/v1/{fedora,ubuntu,arch}/packages"
    for pkg in "$CCS_PKG" "$RPM_PKG" "$DEB_PKG" "$ARCH_PKG"; do
        [[ -f "$pkg" ]] && echo "  [DRY RUN] Would copy $(basename "$pkg") to $RELEASES_DIR/"
    done
    echo "  [DRY RUN] Would generate SHA256SUMS"
    echo "  [DRY RUN] Would update /conary/releases/latest symlink"
    echo ""
    echo "=== Dry run complete ==="
    exit 0
fi

# Create releases directory and copy artifacts
ssh "$REMI_HOST" "mkdir -p $RELEASES_DIR"

# Copy CCS to self-update directory
scp "$CCS_PKG" "$REMI_HOST:$SELF_UPDATE_DIR/conary-${VERSION}.ccs"
echo "  Uploaded CCS to self-update/"

# Copy all artifacts to releases directory
for pkg in "$CCS_PKG" "$RPM_PKG" "$DEB_PKG" "$ARCH_PKG"; do
    if [[ -f "$pkg" ]]; then
        scp "$pkg" "$REMI_HOST:$RELEASES_DIR/"
        echo "  Uploaded $(basename "$pkg") to releases/${VERSION}/"
    fi
done

# Generate SHA256SUMS on Remi
ssh "$REMI_HOST" "cd $RELEASES_DIR && sha256sum * > SHA256SUMS 2>/dev/null || true"
echo "  Generated SHA256SUMS"

# Update latest symlink
ssh "$REMI_HOST" "ln -sfn $RELEASES_DIR /conary/releases/latest"
echo "  Updated latest symlink -> ${VERSION}"

# Publish CCS as regular package to Remi API for each distro
for distro in fedora ubuntu arch; do
    printf "  Publishing CCS to %s... " "$distro"
    HTTP_CODE=$(curl -sf -o /dev/null -w '%{http_code}' \
        -X POST "$REMI_ENDPOINT/v1/$distro/packages" \
        -F "package=@$CCS_PKG" -F "format=ccs" 2>/dev/null || echo "000")
    if [[ "$HTTP_CODE" == "200" || "$HTTP_CODE" == "201" || "$HTTP_CODE" == "409" ]]; then
        echo "OK (HTTP $HTTP_CODE)"
    else
        echo "WARN (HTTP $HTTP_CODE)"
    fi
done

# ── Smoke test ───────────────────────────────────────────────────────────────

echo ""
echo "Smoke testing..."
LATEST=$(curl -sf "$REMI_ENDPOINT/v1/ccs/conary/latest" 2>/dev/null | grep -o '"version":"[^"]*"' | cut -d'"' -f4)
if [[ "$LATEST" == "$VERSION" ]]; then
    echo "  Self-update endpoint reports v${LATEST} [OK]"
else
    echo "  WARN: Self-update endpoint reports v${LATEST}, expected v${VERSION}"
fi

echo ""
echo "=== Release v${VERSION} published ==="
```

**Step 2: Make executable**

```bash
chmod +x scripts/publish-release.sh
```

**Step 3: Test dry-run locally**

Run: `./scripts/publish-release.sh --dry-run`
Expected: Shows version 0.3.0, lists what would be uploaded, exits cleanly.

**Step 4: Commit**

```bash
git add scripts/publish-release.sh
git commit -m "feat: add publish-release.sh for Remi package publishing"
```

---

### Task 3: Create the Forgejo release workflow

**Files:**
- Create: `.forgejo/workflows/release.yaml`

**Step 1: Write the workflow**

```yaml
# .forgejo/workflows/release.yaml
# Build and publish packages on version tag push
name: Release

on:
  push:
    tags:
      - 'v*'

jobs:
  publish:
    runs-on: linux-native
    steps:
      - uses: actions/checkout@v4

      - name: Extract version from tag
        run: |
          TAG="${GITHUB_REF#refs/tags/v}"
          echo "VERSION=$TAG" >> "$GITHUB_ENV"
          echo "Publishing version: $TAG"

      - name: Build and publish
        run: ./scripts/publish-release.sh --version "$VERSION"

      - name: Verify self-update endpoint
        run: |
          LATEST=$(curl -sf https://packages.conary.io/v1/ccs/conary/latest | grep -o '"version":"[^"]*"' | cut -d'"' -f4)
          echo "Self-update reports: v${LATEST}"
          if [ "$LATEST" != "$VERSION" ]; then
            echo "WARN: Version mismatch (expected $VERSION, got $LATEST)"
          fi
```

**Step 2: Commit**

```bash
git add .forgejo/workflows/release.yaml
git commit -m "feat: add Forgejo release workflow for automated package publishing"
```

---

### Task 4: Set up SSH credentials on Forge

This is a manual step -- cannot be automated via code.

**Step 1: Verify Forge can SSH to Remi**

SSH into Forge and test:
```bash
ssh peter@forge.conarylabs.com
ssh remi "echo OK"
```

The Forge runner runs as `peter` with host executor, so it uses `peter`'s SSH config and keys. The `remi` SSH alias should already work (defined in `~/.ssh/config` using the `~/.ssh/remi_conary_io` key from memory).

**Step 2: Verify from runner context**

If SSH works for `peter` interactively, it works for the runner (host executor = same user). No Forgejo secrets needed -- the runner inherits the user's SSH config.

**Step 3: Create /conary/releases/ on Remi**

```bash
ssh remi "mkdir -p /conary/releases /conary/self-update"
```

**Step 4: Document in infrastructure rules**

Add to `.claude/rules/infrastructure.md` under the Remi section:
```
- **Releases:** `/conary/releases/{version}/` (CCS + native packages, SHA256SUMS, `latest` symlink)
- **Self-update:** `/conary/self-update/conary-{version}.ccs` (served by Remi handler)
```

---

### Task 5: Update CI clippy command to match GitHub

The Forge `ci.yaml` runs `cargo clippy -- -D warnings` but GitHub runs `cargo clippy --workspace --all-targets -- -D warnings`. These should match to catch the same lints.

**Files:**
- Modify: `.forgejo/workflows/ci.yaml:30`

**Step 1: Update clippy command**

Change:
```yaml
      - name: Clippy
        run: cargo clippy -- -D warnings
```
to:
```yaml
      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings
```

**Step 2: Commit**

```bash
git add .forgejo/workflows/ci.yaml
git commit -m "fix: align Forge clippy flags with GitHub CI (--workspace --all-targets)"
```

---

### Task 6: End-to-end test of publish script on Forge

**Step 1: Push all changes to GitHub**

```bash
git push origin main
```

Wait for Forge mirror sync (up to 10 minutes), or force sync:
```bash
curl -X POST http://forge.conarylabs.com:3000/api/v1/repos/peter/Conary/mirror-sync
```

**Step 2: SSH to Forge and run dry-run**

```bash
ssh peter@forge.conarylabs.com
cd ~/Conary
git pull  # or wait for mirror sync
./scripts/publish-release.sh --dry-run
```

Expected: Shows version 0.3.0, lists all artifacts, no errors.

**Step 3: Run real publish**

```bash
./scripts/publish-release.sh
```

Expected:
- Release binary builds (~5 min)
- CCS package builds
- RPM builds in Podman container
- DEB builds in Podman container
- Arch builds in Podman container
- All artifacts uploaded to Remi
- Smoke test passes: self-update endpoint reports 0.3.0

**Step 4: Verify from a client**

```bash
curl -sf https://packages.conary.io/v1/ccs/conary/latest | python3 -m json.tool
```

Expected: `"version": "0.3.0"`, valid `download_url`, `sha256`, `size`.

**Step 5: Verify releases directory**

```bash
ssh remi "ls -la /conary/releases/0.3.0/"
ssh remi "ls -la /conary/releases/latest"
ssh remi "cat /conary/releases/0.3.0/SHA256SUMS"
```

---

### Task 7: Update CLAUDE.md and infrastructure docs

**Files:**
- Modify: `CLAUDE.md` (add publish-release.sh to scripts)
- Modify: `.claude/rules/infrastructure.md` (add release pipeline section)

**Step 1: Add to CLAUDE.md release section**

After the existing `Release:` line, add:
```
**Publish:** Run `./scripts/publish-release.sh` on Forge to build and upload packages to Remi. Use `--dry-run` to preview.
```

**Step 2: Add to infrastructure.md Scripts table**

Add row:
```
| `scripts/publish-release.sh` | Build CCS + native packages, publish to Remi |
```

Add to infrastructure.md under Remi section:
```
- **Releases:** `/conary/releases/{version}/` with CCS, RPM, DEB, Arch packages + SHA256SUMS
- **Self-update:** `/conary/self-update/conary-{version}.ccs` (served by self-update handler)
- **Publish:** Automated via `.forgejo/workflows/release.yaml` on `v*` tags, or manual via `scripts/publish-release.sh` on Forge
```

**Step 3: Commit**

```bash
git add CLAUDE.md .claude/rules/infrastructure.md
git commit -m "docs: add release pipeline to CLAUDE.md and infrastructure rules"
```
