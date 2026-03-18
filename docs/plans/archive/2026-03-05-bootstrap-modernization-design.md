# Design: Bootstrap Modernization

*2026-03-05*

## Problem

The bootstrap module (4,935 lines, 42 passing tests) was implemented and left
alone. Stages 0, 1, BaseSystem, and Image have code but use placeholder
checksums, hardcoded build order, inconsistent sandboxing, and outdated
toolchain versions. Stages 2, Boot, Networking, and Conary are stubs. No
actual recipe files exist.

## Approach

Modernize + Complete (Approach 1). Align with LFS 12.4, replace the image
builder with systemd-repart, implement all stub stages, wire up dependency-
resolved ordering via RecipeGraph, fix all TODOs/placeholders, write actual
recipe files, and sandbox Base builds consistently.

## Constraints

- Align with LFS 12.4 (Sept 2025): binutils 2.45, gcc 15.2.0, glibc 2.42, kernel 6.16.1
- crosstool-ng 1.28.0 for Stage 0
- systemd-repart for rootless image generation (fallback to sfdisk/mkfs if unavailable)
- All recipe files use real SHA-256 checksums, no placeholders
- Sandboxed builds via ContainerConfig::pristine_for_bootstrap() throughout

## Design

### Section 1: Toolchain Modernization (Stage 0 + Stage 1)

**Stage 0:**
- Update crosstool-ng config template to target ct-ng 1.28.0
- Update default component versions in BootstrapConfig to LFS 12.4 targets
- Implement seed caching: check `<work_dir>/downloads/` before re-fetching
- Implement glibc/binutils version detection (parse `--version` output)

**Stage 1:**
- Update 5-package build order to match LFS 12.4 methodology
- Write actual recipe files: `recipes/stage1/{linux-headers,binutils,gcc-pass1,glibc,gcc-pass2}.toml`
- Remove `"VERIFY_BEFORE_BUILD"` / `"FIXME"` checksum skip logic

### Section 2: Base System (Stage 2 + BaseSystem)

**Stage 2 (pure rebuild):**
- Rebuild Stage 1's 5 packages using Stage 1 compiler (not Stage 0 cross-compiler)
- Purpose: reproducibility verification -- toolchain builds itself
- Reuses Stage1Builder logic with different toolchain input
- Optional via `--skip-stage2` flag

**BaseSystem overhaul:**
- Replace hardcoded 60-package list with dependency-resolved ordering via RecipeGraph
- Write recipe files in `recipes/base/` for all packages (~80, LFS 12.4 versions)
- Existing 5 phases become tags/labels for progress reporting, not ordering constraints
- Add Sandbox isolation (currently bare `bash`)
- Real checksums on every recipe

**Recipe file structure:**
```
recipes/
  stage1/
    linux-headers.toml
    binutils.toml
    gcc-pass1.toml
    glibc.toml
    gcc-pass2.toml
  base/
    zlib.toml
    bzip2.toml
    xz.toml
    ... (~80 packages)
  conary/
    rust.toml
    conary.toml
```

### Section 3: Networking, Conary, and Boot Stages

**Boot and Networking** collapse into BaseSystem. Bootloader packages (GRUB,
dracut) and networking packages (iproute2, openssh, dhcpcd) are recipes in
`recipes/base/` with `boot` and `networking` tags. Built via the dependency
graph. The stage enum variants become checkpoints within BaseSystem.

**Conary stage** remains distinct because it requires Rust (large separate
toolchain). Pipeline: download rustc bootstrap binary, `./x.py build` targeting
the new sysroot, then `cargo build --release` for Conary. Optional via
`--skip-conary`.

**Revised pipeline:**
```
Stage0 -> Stage1 -> Stage2 (optional) -> BaseSystem -> Conary (optional) -> Image
```

### Section 4: Image Builder Modernization

**Replace sfdisk/mkfs/grub with systemd-repart:**
- Generate repart.d partition definition files, invoke systemd-repart
- Rootless, no loop devices needed
- Fall back to old sfdisk/mkfs method if systemd-repart unavailable

**Partition layout:**
- ESP: 512MB FAT32 (Discoverable Partitions Specification type)
- Root: remaining space ext4 (type per TargetArch)

**UKI support:**
- When `ukify` available, generate Unified Kernel Image (kernel + initrd + cmdline)
- Falls back to traditional kernel + initrd + bootloader

**Output formats:** Raw (primary), Qcow2 (via qemu-img), ISO (via xorriso).

**Tool detection:** Required: systemd-repart OR sfdisk+mkfs (fallback).
Optional: ukify, qemu-img, xorriso.

### Section 5: Cross-cutting Concerns

**Checksum enforcement:**
- Remove all placeholder skip logic
- Mandatory SHA-256 on every recipe
- `--skip-verify` escape hatch for dev/testing with loud warning

**Consistent sandboxing:**
- All stages use ContainerConfig::pristine_for_bootstrap()
- Network denied by default; Conary stage (Rust build) gets controlled access
- Bind mounts: sysroot, sources, build dirs, toolchain

**Kitchen/Cook integration:**
- Route builds through Cook for fetch/unpack/patch/build phases
- Gains: provenance capture, build caching, consistent error handling
- MakedependsResolver is a no-op during bootstrap (deps built in graph order)

**Error handling and resumability:**
- Keep existing stage-level checkpoint system
- Add per-package checkpoints within BaseSystem (resume from package N, not 0)
- Failed builds save logs to `<work_dir>/logs/<package>.log`

**Testing:**
- Unit tests: graph ordering, config parsing, recipe loading (expand existing)
- Integration test: dry-run mode validates full pipeline without building
- End-to-end: manual on forge server (too heavy for CI)

## Files Changed

| Area | Files |
|------|-------|
| Config | conary-core/src/bootstrap/config.rs |
| Toolchain | conary-core/src/bootstrap/toolchain.rs |
| Stage 0 | conary-core/src/bootstrap/stage0.rs |
| Stage 1 | conary-core/src/bootstrap/stage1.rs |
| Stage 2 | conary-core/src/bootstrap/stage2.rs (NEW) |
| Base | conary-core/src/bootstrap/base.rs |
| Conary stage | conary-core/src/bootstrap/conary_stage.rs (NEW) |
| Image | conary-core/src/bootstrap/image.rs |
| Orchestrator | conary-core/src/bootstrap/mod.rs |
| Stages | conary-core/src/bootstrap/stages.rs |
| CLI | src/commands/bootstrap/mod.rs |
| Recipes | recipes/stage1/*.toml, recipes/base/*.toml, recipes/conary/*.toml (NEW) |

## Success Criteria

- All recipe files have real SHA-256 checksums (no placeholders)
- RecipeGraph resolves full base system build order without manual phase ordering
- Sandbox isolation on all build stages
- systemd-repart image generation works rootless (with sfdisk fallback)
- Dry-run validation passes: recipes load, checksums parse, graph resolves
- All existing tests pass + new tests for Stage 2, Conary stage, per-package resume
- Pipeline is end-to-end testable on forge server
