# GitHub Actions Native Package Release Pipeline — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build RPM/DEB/Arch/CCS packages in GitHub Actions on `v*` tag push, create a GitHub Release with all artifacts, and deploy to Remi.

**Architecture:** Single new workflow `.github/workflows/release.yml` with matrix build jobs (one per package format) running in distro containers, followed by a release job that creates the GitHub Release and SSHes to Remi. Replaces the current raw-binary release job in `ci.yml`.

**Tech Stack:** GitHub Actions, softprops/action-gh-release, rustup, rpmbuild, dpkg-buildpackage, makepkg, SSH

---

### Task 1: Create the release workflow

**Files:**
- Create: `.github/workflows/release.yml`

**Step 1: Write the workflow file**

```yaml
# .github/workflows/release.yml
#
# Build native packages and publish a release on v* tag push.
# Builds RPM (Fedora 43), DEB (Ubuntu Noble), Arch, and CCS packages,
# creates a GitHub Release with all artifacts, and deploys to Remi.

name: Release

on:
  push:
    tags: ['v*']

env:
  CARGO_TERM_COLOR: always

jobs:
  # --- Build CCS package (needs conary binary to run `conary ccs build`) ---
  build-ccs:
    name: Build CCS
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-release-${{ hashFiles('**/Cargo.lock') }}

      - name: Build CCS package
        run: bash packaging/ccs/build.sh

      - name: Upload CCS artifact
        uses: actions/upload-artifact@v4
        with:
          name: ccs-package
          path: packaging/ccs/output/*.ccs
          retention-days: 5

  # --- Build RPM ---
  build-rpm:
    name: Build RPM
    runs-on: ubuntu-latest
    container:
      image: registry.fedoraproject.org/fedora:43
    steps:
      - name: Install build dependencies
        run: |
          dnf install -y \
            rpm-build openssl-devel xz-devel pkg-config gcc cmake perl curl git \
          && dnf clean all

      - name: Install Rust
        run: |
          curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
            sh -s -- -y --default-toolchain stable --profile minimal
          echo "$HOME/.cargo/bin" >> "$GITHUB_PATH"

      - uses: actions/checkout@v4

      - name: Build RPM
        run: bash packaging/rpm/build.sh

      - name: Upload RPM artifact
        uses: actions/upload-artifact@v4
        with:
          name: rpm-package
          path: packaging/rpm/output/*.rpm
          retention-days: 5

  # --- Build DEB ---
  build-deb:
    name: Build DEB
    runs-on: ubuntu-latest
    container:
      image: docker.io/library/ubuntu:24.04
    steps:
      - name: Install build dependencies
        env:
          DEBIAN_FRONTEND: noninteractive
        run: |
          apt-get update && apt-get install -y --no-install-recommends \
            build-essential dpkg-dev debhelper libssl-dev liblzma-dev \
            pkg-config cmake perl curl ca-certificates git \
          && rm -rf /var/lib/apt/lists/*

      - name: Install Rust
        run: |
          curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
            sh -s -- -y --default-toolchain stable --profile minimal
          echo "$HOME/.cargo/bin" >> "$GITHUB_PATH"

      - uses: actions/checkout@v4

      - name: Build DEB
        run: bash packaging/deb/build.sh

      - name: Upload DEB artifact
        uses: actions/upload-artifact@v4
        with:
          name: deb-package
          path: packaging/deb/output/*.deb
          retention-days: 5

  # --- Build Arch ---
  build-arch:
    name: Build Arch
    runs-on: ubuntu-latest
    container:
      image: docker.io/library/archlinux:latest
    steps:
      - name: Install build dependencies
        run: |
          pacman -Syu --noconfirm
          pacman -S --noconfirm base-devel openssl xz pkg-config cmake perl rustup git
          pacman -Scc --noconfirm

      - name: Set up build user
        run: |
          useradd -m builder
          echo 'builder ALL=(ALL) NOPASSWD: ALL' >> /etc/sudoers

      - name: Install Rust toolchain
        run: |
          su - builder -c 'rustup default stable'

      - uses: actions/checkout@v4

      - name: Fix ownership for makepkg
        run: chown -R builder:builder .

      - name: Build Arch package
        run: su - builder -c 'cd ${{ github.workspace }} && bash packaging/arch/build.sh'

      - name: Upload Arch artifact
        uses: actions/upload-artifact@v4
        with:
          name: arch-package
          path: packaging/arch/output/*.pkg.tar.zst
          retention-days: 5

  # --- Create release and deploy to Remi ---
  release:
    name: Release
    needs: [build-ccs, build-rpm, build-deb, build-arch]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4

      - name: Extract version from tag
        run: |
          VERSION="${GITHUB_REF#refs/tags/v}"
          echo "VERSION=$VERSION" >> "$GITHUB_ENV"
          echo "Releasing version: $VERSION"

      - name: Download all artifacts
        uses: actions/download-artifact@v4
        with:
          path: release-artifacts

      - name: Collect and checksum artifacts
        run: |
          mkdir -p release-packages
          find release-artifacts -type f \( -name '*.ccs' -o -name '*.rpm' -o -name '*.deb' -o -name '*.pkg.tar.zst' \) \
            ! -name '*debuginfo*' ! -name '*debugsource*' \
            -exec cp {} release-packages/ \;
          cd release-packages
          sha256sum -- * > SHA256SUMS
          echo "=== Release artifacts ==="
          ls -lh

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          generate_release_notes: true
          files: release-packages/*

      - name: Deploy to Remi
        env:
          REMI_SSH_KEY: ${{ secrets.REMI_SSH_KEY }}
        run: |
          # Set up SSH
          mkdir -p ~/.ssh
          echo "$REMI_SSH_KEY" > ~/.ssh/remi_key
          chmod 600 ~/.ssh/remi_key
          ssh-keyscan -H ssh.conary.io >> ~/.ssh/known_hosts 2>/dev/null

          REMI="root@ssh.conary.io"
          SSH_OPTS="-i ~/.ssh/remi_key -o StrictHostKeyChecking=accept-new"

          # Create release directory on Remi
          ssh $SSH_OPTS "$REMI" "mkdir -p /conary/releases/${VERSION} /conary/self-update"

          # Upload CCS to self-update directory
          CCS_FILE=$(find release-packages -name '*.ccs' | head -1)
          if [ -n "$CCS_FILE" ]; then
            scp $SSH_OPTS "$CCS_FILE" "${REMI}:/conary/self-update/conary-${VERSION}.ccs"
            echo "[OK] CCS -> /conary/self-update/conary-${VERSION}.ccs"
          fi

          # Upload all packages to release directory
          scp $SSH_OPTS release-packages/* "${REMI}:/conary/releases/${VERSION}/"
          echo "[OK] All artifacts -> /conary/releases/${VERSION}/"

          # Generate SHA256SUMS and update latest symlink
          ssh $SSH_OPTS "$REMI" bash -s "$VERSION" <<'REMOTE_EOF'
            set -euo pipefail
            VERSION="$1"
            cd "/conary/releases/${VERSION}"
            sha256sum -- * > SHA256SUMS
            ln -sfn "$VERSION" /conary/releases/latest
            echo "[OK] SHA256SUMS generated, latest -> $VERSION"
          REMOTE_EOF

      - name: Smoke test Remi
        run: |
          sleep 5
          response=$(curl -sf "https://packages.conary.io/v1/ccs/conary/latest" 2>/dev/null) || response=""
          if echo "$response" | grep -q "$VERSION"; then
            echo "[OK] Self-update API reports version $VERSION"
          else
            echo "[WARN] Version mismatch or endpoint unavailable"
            echo "Response: $response"
          fi
```

**Step 2: Verify the YAML is valid**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))"`
Expected: No errors

**Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "feat: add GitHub Actions release workflow for native packages"
```

---

### Task 2: Remove the old release job from ci.yml

**Files:**
- Modify: `.github/workflows/ci.yml:84-125`

**Step 1: Remove the `release:` job block**

Delete lines 84-125 (the entire `release:` job) from `.github/workflows/ci.yml`. The `test` and `security` jobs remain unchanged.

**Step 2: Remove the tag trigger**

In the `on.push.tags` field (line 6), remove `[ 'v*' ]` since CI doesn't need to trigger on tags anymore — the new `release.yml` handles that.

The resulting `on:` block should be:
```yaml
on:
  push:
    branches: [ main, develop ]
  pull_request:
    branches: [ main, develop ]
```

**Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "refactor: remove release job from CI, now handled by release.yml"
```

---

### Task 3: Simplify the Forgejo release workflow

**Files:**
- Modify: `.forgejo/workflows/release.yaml`

**Step 1: Replace the publish job with a verify-only job**

The Forgejo release workflow should no longer build anything. It just verifies that the GitHub-built release landed on Remi correctly. GitHub mirrors to Forgejo, so the `v*` tag will arrive here too.

Replace the entire file with:

```yaml
# .forgejo/workflows/release.yaml
# Verify that a GitHub-built release landed on Remi correctly.
# The actual build + publish happens in GitHub Actions (.github/workflows/release.yml).
name: Release Verify

on:
  push:
    tags: ['v*']

jobs:
  verify:
    runs-on: linux-native
    steps:
      - name: Extract version from tag
        run: |
          VERSION="${GITHUB_REF#refs/tags/v}"
          echo "VERSION=$VERSION" >> "$GITHUB_ENV"
          echo "Verifying release: v$VERSION"

      - name: Wait for GitHub Actions to finish
        run: sleep 120

      - name: Verify self-update endpoint
        run: |
          ENDPOINT="${REMI_ENDPOINT:-https://packages.conary.io}"
          response=$(curl -sf "$ENDPOINT/v1/ccs/conary/latest" 2>/dev/null) || response=""
          if echo "$response" | grep -q "$VERSION"; then
            echo "[OK] Self-update API reports version $VERSION"
          else
            echo "[FAILED] Version $VERSION not found on Remi after GitHub release"
            echo "Response: $response"
            exit 1
          fi

      - name: Verify release artifacts
        run: |
          ENDPOINT="${REMI_ENDPOINT:-https://packages.conary.io}"
          for ext in ccs rpm deb pkg.tar.zst; do
            url="$ENDPOINT/releases/$VERSION/"
            if curl -sf "$url" | grep -q "$ext"; then
              echo "[OK] Found .$ext in release directory"
            else
              echo "[WARN] No .$ext found in release directory"
            fi
          done
```

**Step 2: Commit**

```bash
git add .forgejo/workflows/release.yaml
git commit -m "refactor: simplify Forgejo release to verify-only (builds moved to GitHub)"
```

---

### Task 4: Retire publish-release.sh

**Files:**
- Delete: `scripts/publish-release.sh`

**Step 1: Delete the script**

```bash
git rm scripts/publish-release.sh
```

**Step 2: Commit**

```bash
git commit -m "chore: remove publish-release.sh, replaced by GitHub Actions release workflow"
```

---

### Task 5: Add REMI_SSH_KEY secret to GitHub (manual)

**This is a manual step for the user.**

**Step 1: Get the SSH private key**

The Remi SSH key is at `~/.ssh/remi_conary_io` (from MEMORY.md).

**Step 2: Add as GitHub secret**

Go to `https://github.com/ConaryLabs/Conary/settings/secrets/actions` and add:
- Name: `REMI_SSH_KEY`
- Value: Contents of `~/.ssh/remi_conary_io`

Or via CLI:
```bash
gh secret set REMI_SSH_KEY < ~/.ssh/remi_conary_io
```

---

### Task 6: Update documentation

**Files:**
- Modify: `.claude/rules/infrastructure.md`
- Modify: `CLAUDE.md` (if CI workflow table needs updating)

**Step 1: Update infrastructure.md CI Workflows table**

In `.claude/rules/infrastructure.md`, update the CI Workflows table to add the new release workflow and note that the old release job moved:

| Workflow | Trigger | Jobs | Duration |
|----------|---------|------|----------|
| `ci.yaml` (GH) | Push to main/develop, PR | test, security | ~5 min |
| `release.yml` (GH) | Push v* tag | build-ccs, build-rpm, build-deb, build-arch, release | ~15-20 min |
| `ci.yaml` (Forgejo) | Push to main, manual | build, test, clippy, remi-smoke | ~5 min |
| `release.yaml` (Forgejo) | Push v* tag | verify (waits for GH, checks Remi) | ~3 min |
| `integration.yaml` | Push to main, manual | 3-distro Podman matrix | ~15 min |
| `e2e.yaml` | Daily 06:00 UTC, manual | 3-distro Phase 1+2 | ~20-30 min |
| `remi-health.yaml` | Every 6 hours, manual | Full endpoint verification | ~60s |

**Step 2: Update Scripts table**

Remove `publish-release.sh` from the Scripts table. Add a note that release publishing is now handled by GitHub Actions.

**Step 3: Commit**

```bash
git add .claude/rules/infrastructure.md
git commit -m "docs: update infrastructure docs for GitHub Actions release pipeline"
```

---

### Task 7: Test with a release

**Step 1: Push pending commits to GitHub**

```bash
git push origin main
```

**Step 2: Create a test release**

Use `scripts/release.sh` to bump version and tag:
```bash
./scripts/release.sh conary --dry-run
```

If the dry run looks good:
```bash
./scripts/release.sh conary
git push origin main --tags
```

**Step 3: Monitor GitHub Actions**

Watch the release workflow at `https://github.com/ConaryLabs/Conary/actions`.

Verify:
- All 4 build jobs succeed (CCS, RPM, DEB, Arch)
- Release job creates GitHub Release with all artifacts
- Remi deployment succeeds
- Smoke test passes

**Step 4: Verify Forgejo**

After ~2 minutes, check that Forgejo's verify job confirms the release landed.
