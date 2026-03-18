# Documentation Feature Audit Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Update all public-facing docs and site to document 15 underdocumented features, with system generations as the headline story.

**Architecture:** 9 files across 7 tasks. Each task updates one or two related files, with a commit after each. No Rust code changes. Site must build cleanly after site tasks.

**Tech Stack:** Markdown (README, docs), SvelteKit/Svelte (site pages), CSS

---

### Task 1: README.md -- Major Restructure

**Files:**
- Modify: `README.md`

**Context:** The README currently opens with "cross-distribution Linux package manager" and lists generation/composefs under "What's Next." It needs to lead with the system generation story and document all CLI-ready features. The current README is ~374 lines. See the design doc at `docs/plans/2026-03-05-docs-feature-audit-design.md` for the full structure.

**Step 1: Rewrite the README**

Rewrite `README.md` with this structure (read the current file first):

1. **Title + badges** -- Keep existing badges. Change subtitle from "cross-distribution Linux package manager" to "A cross-distribution Linux system manager" -- add mention of atomic generations, content-addressable storage, and declarative system model.

2. **Why Conary** -- 5 pillars with descriptions and code examples:
   - **Immutable system generations.** EROFS images + composefs. Live-switch between system states without rebooting. Every generation is a complete, verified filesystem snapshot. Show `conary generation build/switch/list` commands.
   - **Atomic operations.** (Keep existing atomic transaction text but tighten.)
   - **Format-agnostic.** (Keep existing multi-format text.)
   - **Declarative state.** (Keep existing model text.)
   - **68,000+ packages on day one.** (Keep existing Remi text.)

3. **How It Compares** -- Add new rows to the existing comparison table:
   - Immutable generations: Conary=Yes, apt/dnf/pacman=No, Nix=Yes (generations)
   - System takeover: Conary=Yes, all others=No
   - Bootstrap from scratch: Conary=Yes, apt/dnf/pacman=No, Nix=Yes
   - Derived packages: Conary=Yes, all others=No
   - Keep all existing rows

4. **Quick Start** -- Keep existing quick start. Add a "System Generations" section after the basic install:
   ```bash
   # Build a generation from current system state
   conary generation build --summary "Initial setup"

   # List generations
   conary generation list

   # Switch to a generation (live, no reboot required)
   conary generation switch 1
   ```

5. **Features** -- Reorganize. These are top-level (NOT collapsed):

   - **System Generations** (NEW) -- EROFS images, composefs overlay, live switching, rollback. Show `conary generation build/list/switch/rollback/gc` commands. Note: requires Linux 6.2+ with composefs support.
   - **System Takeover** (NEW) -- Analyze, plan, and atomically adopt every package on an existing system. Show `conary generation takeover --dry-run` then `conary generation takeover`.
   - **Atomic Transactions** (existing -- keep)
   - **Multi-Format Support** (existing -- keep)
   - **Declarative System Model** (existing -- keep)
   - **Content-Addressable Storage** (existing -- keep)
   - **Dependency Resolution** (existing -- keep)
   - **Component Model** (existing -- keep)
   - **Bootstrap System** (NEW) -- Build a complete Conary-managed system from scratch. Staged pipeline: Stage 0 (cross-compiler), Stage 1 (self-hosted toolchain), Base (core packages), Image (bootable raw/qcow2/iso). Show commands: `conary bootstrap init --target x86_64`, `conary bootstrap stage0`, `conary bootstrap stage1`, `conary bootstrap base`, `conary bootstrap image --format qcow2`.
   - **Derived Packages** (NEW) -- Create custom variants of existing packages with patches and file overrides. Show `conary derive create my-nginx --from nginx`, `conary derive patch my-nginx fix.patch`, `conary derive build my-nginx`.

   These are collapsed (`<details>`):
   - **CCS Native Package Format** (existing -- keep)
   - **Recipe System and Hermetic Builds** (existing -- keep)
   - **Dev Shells** (existing -- keep)
   - **Collections** (existing -- keep)
   - **Labels and Federation** (existing -- keep)
   - **Sandboxed Scriptlets** (existing -- keep)
   - **Capability Enforcement** (existing -- keep)
   - **Configuration Management** (NEW) -- Track, diff, backup, and restore config files with noreplace support. Show `conary config list`, `conary config diff /etc/nginx/nginx.conf`, `conary config backup /etc/nginx/nginx.conf`, `conary config restore /etc/nginx/nginx.conf`.
   - **Package Provenance and SBOM** (existing -- expand to mention SLSA attestation)
   - **Trigger System** (NEW as collapsed) -- 10+ built-in triggers with DAG-ordered execution. List: ldconfig, systemd-reload, fc-cache, update-mime-database, gtk-update-icon-cache, depmod, etc.

6. **Architecture** -- Keep but update the description to mention "system manager" not "package manager"

7. **Remi Server** -- Keep existing

8. **conaryd Daemon** -- Keep existing

9. **CAS Federation** -- Keep existing

10. **Building** -- Keep existing

11. **Project Status** -- Update: mention generations, system takeover, bootstrap as implemented

12. **What's Next** -- Remove "Atomic filesystem updates using renameat2" (generations solve this). Remove anything else that's done. Keep:
    - Shell integration (direnv-style automatic environment activation)
    - P2P chunk distribution plugins (IPFS, BitTorrent DHT)
    - Multi-version package support (multiple kernel versions)
    - VFS component merging
    - Full repository server with version control

**Step 2: Verify the README reads well**

Read through the full README to check flow and consistency.

**Step 3: Commit**

```bash
git add README.md
git commit -m "docs: Restructure README with system generations as headline feature

Add system generations, system takeover, bootstrap, derived packages,
config management, and trigger system documentation. Move generations
from What's Next to a top-level feature. Reframe from package manager
to system manager."
```

---

### Task 2: RELEASE_NOTES.md -- Add Missing Features

**Files:**
- Modify: `RELEASE_NOTES.md`

**Context:** RELEASE_NOTES.md is 320 lines covering the 0.1.0 release but missing system generations, system takeover, bootstrap, derived packages, and config management.

**Step 1: Add missing feature sections**

Read `RELEASE_NOTES.md` first, then add these sections under "## New Features" (after existing features):

**System Generations** section:
```markdown
### System Generations

Atomic, immutable filesystem snapshots using EROFS images and Linux composefs. Build a generation from current system state and switch between generations live, without rebooting.

```bash
conary generation build --summary "Post-update snapshot"
conary generation list
conary generation switch 2
conary generation rollback
conary generation gc --keep 3
```

Requires Linux 6.2+ with composefs support (`CONFIG_EROFS_FS`).
```

**System Takeover** section:
```markdown
### System Takeover

Adopt an entire existing Linux system into Conary management. Analyzes all installed RPM/DEB/pacman packages, plans the adoption strategy, and atomically takes over the system with an initial generation.

```bash
conary generation takeover --dry-run    # Preview what will happen
conary generation takeover              # Execute takeover
```
```

**Bootstrap System** section:
```markdown
### Bootstrap System

Build a complete Conary-managed system from scratch using a staged pipeline. Supports x86_64, aarch64, and riscv64 targets.

```bash
conary bootstrap init --target x86_64
conary bootstrap check                  # Verify prerequisites
conary bootstrap stage0                 # Cross-compilation toolchain
conary bootstrap stage1                 # Self-hosted toolchain
conary bootstrap base                   # Core system packages
conary bootstrap image --format qcow2   # Bootable image
conary bootstrap status                 # Progress report
```
```

**Derived Packages** section:
```markdown
### Derived Packages

Create custom variants of existing packages with patches and file overrides. Derived packages track their parent and can be rebuilt when the parent updates.

```bash
conary derive create my-nginx --from nginx
conary derive patch my-nginx security-fix.patch
conary derive override my-nginx /etc/nginx/nginx.conf --source ./my-nginx.conf
conary derive build my-nginx
conary derive stale                     # List outdated derived packages
```
```

**Configuration Management** section:
```markdown
### Configuration Management

Track, diff, backup, and restore system configuration files. Honors `noreplace` flags from RPM/DEB to preserve user modifications during upgrades.

```bash
conary config list                       # Show modified configs
conary config diff /etc/nginx/nginx.conf # Compare against package version
conary config backup /path              # Create backup
conary config restore /path             # Restore from backup
conary config check                     # Status of all config files
```
```

Also update "## What's Next" at the bottom: remove "Atomic filesystem updates using `renameat2(RENAME_EXCHANGE)`" since generations handle this.

**Step 2: Commit**

```bash
git add RELEASE_NOTES.md
git commit -m "docs: Add generations, takeover, bootstrap, derived packages to release notes"
```

---

### Task 3: docs/ARCHITECTURE.md -- Add Generations and Bootstrap

**Files:**
- Modify: `docs/ARCHITECTURE.md`

**Context:** ARCHITECTURE.md has detailed module maps and data flows but no mention of system generations, composefs, EROFS, or bootstrap.

**Step 1: Add System Generations section**

Read `docs/ARCHITECTURE.md` first. Add a new section after "## Data Flow: Remi Server Request" called "## System Generations":

```markdown
## System Generations

Conary can manage the entire system filesystem as immutable, atomic
generations using EROFS images and Linux composefs.

### Architecture

```
Current System State
       |
  conary generation build
       |
  +----+----+
  | Snapshot |-- Capture all installed troves from SQLite
  +----+----+
       |
  +----+----+
  |  EROFS   |-- conary-erofs crate builds read-only filesystem image
  | Builder  |-- LZ4/LZMA compression, inline data, chunk references
  +----+----+
       |
  +----+----+
  | composefs|-- Linux 6.2+ overlay with fs-verity content verification
  | Mount    |-- CAS objects referenced by content hash
  +----+----+
       |
  Generation N (immutable, verified)
```

### Generation Lifecycle

1. **Build**: Snapshot current troves, construct EROFS image from CAS
2. **Store**: Save generation metadata (number, timestamp, summary, trove list)
3. **Switch**: Mount new generation via composefs, update boot entries
4. **Rollback**: Switch back to any previous generation
5. **GC**: Remove old generations, keeping N most recent

### conary-erofs Crate

Dedicated crate for building EROFS (Enhanced Read-Only File System)
images. Handles superblock construction, inode layout, directory
entries, data compression (LZ4, LZMA), extended attributes, and
chunk-based external file references to the CAS.

### composefs Integration

The composefs driver (Linux 6.2+, `CONFIG_EROFS_FS`) provides:
- Content-verified overlays using fs-verity
- Efficient sharing of identical files across generations via CAS
- Atomic generation switching without unmounting
```

**Step 2: Add Bootstrap section**

Add after the System Generations section:

```markdown
## Bootstrap Pipeline

Build a complete Conary-managed system from scratch:

```
Stage 0: Cross-Compiler
  crosstool-ng -> minimal GCC + binutils for target arch
       |
Stage 1: Self-Hosted Toolchain
  Use Stage 0 to build native GCC, binutils, glibc on target
       |
Base System
  Cook core packages: kernel, systemd, coreutils, bash, networking
  Install into sysroot, create initial generation
       |
Image Generation
  Build bootable image (raw, qcow2, or ISO) from sysroot
```

Supports x86_64, aarch64, and riscv64 targets. Each stage has
checkpoint/resume support for interrupted builds.
```

**Step 3: Add Derived Packages to Core Concepts**

Add after the "Label" concept entry:

```markdown
### Derived Package

A package created by modifying an existing package (the parent) with
patches and file overrides. Derived packages track their parent and
can be flagged as stale when the parent updates.
```

**Step 4: Update the system overview diagram**

In the ASCII diagram at the top, add `Generation` and `Bootstrap` as command branches alongside Install/Query/Model:

```
  Install       Query   Model   Generation  Bootstrap
  Remove        Search  Apply   Build       Stage0/1
  Update        SBOM    Diff    Switch      Base/Image
```

**Step 5: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: Add system generations, bootstrap, derived packages to architecture"
```

---

### Task 4: ROADMAP.md -- Update Status

**Files:**
- Modify: `ROADMAP.md`

**Context:** ROADMAP.md is comprehensive (588 lines) and most features are already marked [COMPLETE]. The main issues: (1) "Atomic Filesystem Updates" is listed under "Long-Term / Future" but is implemented via generations, (2) "What's Next" / Contributing section references items that are done.

**Step 1: Update Long-Term section**

Read `ROADMAP.md` first. In the "## Long-Term / Future Consideration" section:

Replace the "### Atomic Filesystem Updates (Inspired by Aeryn OS)" section. Change the unchecked items to checked and add a note:

```markdown
### Atomic Filesystem Updates (Inspired by Aeryn OS)

Implemented via system generations using EROFS + composefs.

- [COMPLETE] **EROFS Image Builder** - conary-erofs crate for building immutable filesystem images
- [COMPLETE] **composefs Integration** - Linux 6.2+ overlay with fs-verity content verification
- [COMPLETE] **Generation Build** - Snapshot system state into EROFS generation
- [COMPLETE] **Generation Switch** - Live-switch to any generation without reboot
- [COMPLETE] **Generation Rollback** - Switch back to previous generation
- [COMPLETE] **Generation GC** - Remove old generations
- [COMPLETE] **System Takeover** - Adopt entire existing system into generation management
- [ ] **renameat2 RENAME_EXCHANGE** - Atomic directory swap (alternative to composefs on older kernels)
```

**Step 2: Add System Generations to Completed Features**

Add a new "### System Generations" section under "## Completed Features" (after "### System Operations"):

```markdown
### System Generations

- [COMPLETE] **EROFS Image Builder** - conary-erofs crate builds immutable filesystem images (LZ4/LZMA)
- [COMPLETE] **composefs Integration** - Linux 6.2+ overlay with fs-verity verification
- [COMPLETE] **Generation Build** - Snapshot current state into numbered EROFS generation
- [COMPLETE] **Generation Switch** - Live-switch to any generation without reboot
- [COMPLETE] **Generation Rollback** - Switch back to previous generation
- [COMPLETE] **Generation GC** - Remove old generations (configurable keep count)
- [COMPLETE] **Generation Info** - Show detailed metadata for any generation
- [COMPLETE] **System Takeover** - Adopt entire existing system into generation management
- [COMPLETE] **CLI Commands** - generation list, build, switch, rollback, gc, info, takeover
```

**Step 3: Update Contributing priorities**

In the Contributing section at the bottom, update priority areas. Remove "Atomic filesystem updates" (done). Replace with:
1. Shell integration for dev shells (direnv-style)
2. P2P chunk distribution plugins
3. VFS component merging
4. Multi-version package support (kernels)
5. Web interface improvements

**Step 4: Commit**

```bash
git add ROADMAP.md
git commit -m "docs: Move generations/takeover to completed, update roadmap priorities"
```

---

### Task 5: Site -- Layout, Home Page, About, Compare

**Files:**
- Modify: `site/src/routes/+layout.svelte`
- Modify: `site/src/routes/+page.svelte`
- Modify: `site/src/routes/about/+page.svelte`
- Modify: `site/src/routes/compare/+page.svelte`

**Context:** Layout has 4 nav links (Home, Install, Compare, About) + external Packages link. Home page has 6 feature cards (none mention generations). Compare page has no rows for generations/takeover/bootstrap. About page architecture grid has 6 items (no generations/bootstrap).

**Step 1: Add Features nav link to layout**

Read `site/src/routes/+layout.svelte`. Add `{ href: '/features', label: 'Features' }` to the `navLinks` array after `{ href: '/', label: 'Home' }`. Also add a Features link to the footer links div.

Update the footer text from "the cross-distribution package manager" to "the cross-distribution system manager".

**Step 2: Update home page hero and features**

Read `site/src/routes/+page.svelte`. Make these changes:

- Update `<title>` meta: change "Package Manager" to "System Manager"
- Update hero tagline: "One system manager for every Linux distro"
- Update hero description: mention immutable generations alongside atomic transactions
- Replace the 6 feature cards with 9 cards in a 3x3 grid:
  1. **Immutable Generations** (NEW) -- EROFS + composefs. Build, switch, rollback entire system states without rebooting.
  2. **Cross-Distribution** (existing)
  3. **Atomic Transactions** (existing)
  4. **System Takeover** (NEW) -- Adopt an entire existing Linux system into Conary management with a single command.
  5. **Content-Addressable Storage** (existing)
  6. **Binary Deltas** (existing)
  7. **System Model** (existing)
  8. **Bootstrap** (NEW) -- Build a complete system from scratch. Staged pipeline from cross-compiler to bootable image.
  9. **On-Demand Conversion** (existing)
- Add "Immutable generations" row to the comparison teaser table (Conary=Yes, apt/dnf/pacman=No, Nix=Yes)
- Update the terminal demo to show a generation command after install:
  ```
  $ conary generation build --summary "Added nginx"
  Building generation 3...
  Generation 3 built (142 packages, 847 MB)
  ```

**Step 3: Update about page architecture grid**

Read `site/src/routes/about/+page.svelte`. Add two items to the `arch-grid`:
- **Generations** -- Immutable EROFS filesystem images with composefs overlay. Live-switch between system states.
- **Bootstrap** -- Staged pipeline to build a complete system from scratch. Stage 0 through bootable image.

Update the subtitle from "Reviving a visionary 2005 design" to "Reviving a visionary 2005 design -- now with immutable system generations" or similar that mentions the generation capability.

**Step 4: Update compare page**

Read `site/src/routes/compare/+page.svelte`. Add rows to the comparison table:
- "Immutable generations" -- Conary=Yes, apt=No, dnf=No, pacman=No, Nix=Yes
- "System takeover" -- Conary=Yes, all others=No
- "Bootstrap from scratch" -- Conary=Yes, apt=No, dnf=No, pacman=No, Nix=Yes

Add a new detail card after the "vs. Nix" section:

```
### vs. NixOS Generations

Both Conary and NixOS support immutable system generations, but the
approach differs. NixOS builds generations from Nix expressions in a
custom functional language. Conary builds generations from the actual
installed package set -- EROFS images verified by composefs and backed
by content-addressable storage. Conary generations work with existing
RPM/DEB/Arch packages; NixOS requires packages to be rewritten as Nix
derivations.
```

**Step 5: Verify site builds**

```bash
cd site && npm run build
```

Expected: builds successfully with no errors (warnings about unused CSS are OK).

**Step 6: Commit**

```bash
git add site/src/routes/+layout.svelte site/src/routes/+page.svelte site/src/routes/about/+page.svelte site/src/routes/compare/+page.svelte
git commit -m "site: Add generations, takeover, bootstrap to home/about/compare pages

Update hero messaging from package manager to system manager. Add
immutable generations as lead feature card. New comparison table
rows. Features nav link added."
```

---

### Task 6: Site -- New /features Page

**Files:**
- Create: `site/src/routes/features/+page.svelte`

**Context:** This is a new deep-dive page covering all CLI-ready features. It should follow the existing site design (dark theme, css variables like `--color-accent`, `--color-surface`, `--color-border`, `--font-display`, `--font-mono`, etc.). Use the about page and compare page as style references.

**Step 1: Create the features page**

Create `site/src/routes/features/+page.svelte` with this structure:

- `<svelte:head>` with title "Features - Conary" and description
- Page hero: "Features" title, subtitle "Everything Conary can do, with examples."
- Four category sections, each with a heading and feature cards:

**System Management:**
- **System Generations** -- EROFS + composefs, live switching. CLI: `conary generation build/list/switch/rollback/gc/info`. Note: requires Linux 6.2+.
- **System Takeover** -- Full-system adoption. CLI: `conary generation takeover [--dry-run]`
- **Bootstrap** -- Staged from-scratch system build. CLI: `conary bootstrap init/check/stage0/stage1/base/image/status/resume/clean`. Targets: x86_64, aarch64, riscv64.
- **Snapshots and Rollback** -- Numbered system state snapshots with diff and restore. CLI: `conary system state list/diff/restore/prune`

**Package Management:**
- **Multi-Format Install** -- RPM, DEB, Arch packages with unified metadata. CLI: `conary install ./package.rpm`, `.deb`, `.pkg.tar.zst`
- **SAT-Based Resolver** -- resolvo-based dependency resolution with typed deps (soname, python, pkgconfig, etc.). CLI: `conary deptree nginx`
- **Component Model** -- Automatic split into :runtime, :lib, :devel, :doc, :config, :debuginfo. CLI: `conary install nginx:runtime`
- **Derived Packages** -- Custom variants with patches and file overrides. CLI: `conary derive create/patch/override/build/stale`
- **Configuration Management** -- Track, diff, backup, restore config files with noreplace. CLI: `conary config list/diff/backup/restore/check`

**Build and Distribution:**
- **CCS Native Format** -- CBOR manifest, Merkle tree, Ed25519 signatures, FastCDC chunking. CLI: `conary ccs build/sign/verify/inspect`
- **Recipe System** -- TOML recipes, hermetic builds with namespace isolation. CLI: `conary cook recipe.toml [--hermetic]`
- **Dev Shells** -- Temporary environments without permanent install. CLI: `conary ccs shell python,nodejs`, `conary ccs run gcc -- make`
- **OCI Export** -- Export packages as container images. CLI: `conary ccs export nginx --format oci`
- **Declarative System Model** -- TOML-based system state with drift detection. CLI: `conary model diff/apply/check/snapshot`

**Infrastructure:**
- **Content-Addressable Storage** -- SHA-256 + XXH128 with automatic deduplication and FastCDC chunking. CLI: `conary system verify/restore/gc`
- **CAS Federation** -- Distributed chunk sharing with mDNS discovery and hierarchical routing. CLI: `conary federation status/peers/scan/stats`
- **Package Provenance (DNA)** -- Full provenance chain with SLSA attestation. CLI: `conary query sbom nginx`
- **Trigger System** -- 10+ built-in post-install triggers with DAG ordering: ldconfig, systemd-reload, fc-cache, depmod, etc.
- **Capability Enforcement** -- Landlock filesystem + seccomp-BPF syscall restrictions. CLI: `conary capability audit/enforce nginx`
- **Sandboxed Scriptlets** -- Namespace isolation (mount, PID, IPC, UTS) with resource limits. CLI: `conary install pkg --sandbox=always`

Each feature card should show:
- Feature name as h3 (styled with `--color-accent`, `--font-display`)
- 2-3 sentence description
- Code block with CLI examples (styled like the terminal on the home page but simpler -- just monospace background blocks)
- Any requirements note (e.g., "Requires Linux 6.2+" for generations)

Style: Use the existing card pattern from the about page (`background: var(--color-surface)`, `border: 1px solid var(--color-border)`, `border-radius: var(--radius-lg)`). Category headings use `--font-display` with accent color. The page should feel comprehensive but scannable.

**Step 2: Verify site builds**

```bash
cd site && npm run build
```

**Step 3: Commit**

```bash
git add site/src/routes/features/+page.svelte
git commit -m "site: Add comprehensive /features page with all CLI-ready features

Covers system management (generations, takeover, bootstrap, snapshots),
package management (multi-format, resolver, components, derived, config),
build and distribution (CCS, recipes, dev shells, OCI, model),
and infrastructure (CAS, federation, provenance, triggers, capabilities)."
```

---

### Task 7: Deploy and Verify

**Files:** None modified

**Step 1: Build site**

```bash
cd /home/peter/Conary/site && npm run build
```

**Step 2: Deploy**

```bash
cd /home/peter/Conary && ./deploy/deploy-sites.sh site
```

This deploys `site/build/` to `remi:/conary/site` (conary.io). It does NOT touch `/conary/web` (packages.conary.io).

**Step 3: Push all commits**

```bash
git push
```

**Step 4: Verify**

Confirm the site is live and all pages load correctly. The features page should be accessible at the `/features` path.
