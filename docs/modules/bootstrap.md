---
last_updated: 2026-03-31
revision: 3
summary: Document the current bootstrap command surface, manifest pipeline artifacts, and comparison commands
---

# Bootstrap Module (conary-core/src/bootstrap/)

6-phase bootstrap pipeline for building a complete Conary system from scratch
without an existing package manager. Aligned with Linux From Scratch 13
(binutils 2.45, gcc 15.2.0, glibc 2.42, kernel 6.16.1) through cross-compilation,
temporary tools, final system, configuration, imaging, and Tier 2 extension.

## Data Flow: Bootstrap Pipeline

```
Host System (any Linux with gcc)
     |
  CrossToolsBuilder -- Phase 1: Cross-toolchain (LFS Ch5)
     |                  Produces: $LFS/tools/
     |                  Cross binutils, cross-GCC, glibc, libstdc++
     |
  TempToolsBuilder -- Phase 2: Temporary tools (LFS Ch6-7)
     |                 17 cross-compiled + 6 chroot packages
     |
  FinalSystemBuilder -- Phase 3: Final system (LFS Ch8)
     |                   77 packages -- complete Linux system
     |                   Built inside chroot via ChrootEnv
     |
  configure_system() -- Phase 4: System configuration (LFS Ch9)
     |                   Network, fstab, kernel, bootloader
     |
  ImageBuilder -- Phase 5: Bootable image (LFS Ch10)
     |             systemd-repart (fallback: sfdisk/mkfs)
     |             GPT: 512MB ESP (FAT32) + root (ext4)
     |             Output: raw, qcow2, ISO, EROFS
     |
  Tier2Builder -- Phase 6: BLFS + Conary
     |             PAM, OpenSSH, curl, Rust, Conary self-hosting
     |
  StageManager -- JSON checkpoint file (bootstrap-state.json)
                   Per-stage completion + per-package checkpointing
```

## Key Types

| Type | File | Purpose |
|------|------|---------|
| `BootstrapConfig` | config.rs | Toolchain versions, paths, target arch, parallelism |
| `TargetArch` | config.rs | Enum: X86_64, Aarch64, Riscv64 (with triples) |
| `BootstrapStage` | stages.rs | Enum: CrossTools, TempTools, FinalSystem, SystemConfig, BootableImage, Tier2 |
| `StageManager` | stages.rs | Progress tracker with JSON persistence and resume |
| `StageState` | stages.rs | Per-stage completion, timestamps, package checkpoints |
| `CrossToolsBuilder` | cross_tools.rs | Phase 1: cross-toolchain build |
| `TempToolsBuilder` | temp_tools.rs | Phase 2: temporary tools (cross + chroot packages) |
| `FinalSystemBuilder` | final_system.rs | Phase 3: complete system build (SYSTEM_BUILD_ORDER) |
| `configure_system()` | system_config.rs | Phase 4: system configuration |
| `ImageBuilder` | image.rs | Phase 5: disk image generation (raw, qcow2, ISO, EROFS) |
| `ImageFormat` | image.rs | Enum: Raw, Qcow2, Iso |
| `ImageSize` | image.rs | Parsed size specification for disk images |
| `ImageTools` | image.rs | Host tool availability check for imaging |
| `ImageResult` | image.rs | Build result with path and metadata |
| `Tier2Builder` | tier2.rs | Phase 6: BLFS + Conary self-hosting |
| `PackageBuildRunner` | build_runner.rs | Source fetch, verify, extract, build for individual packages |
| `BuildContext` | build_runner.rs | Enum: build context type |
| `ChrootEnv` | chroot_env.rs | Chroot environment setup for Phase 3 builds |
| `RepartDefinition` | repart.rs | systemd-repart partition config (ESP, root) |
| `Toolchain` | toolchain.rs | Resolved toolchain path, kind, and version detection |
| `ToolchainKind` | toolchain.rs | Enum: toolchain type discriminant |

## Files

15 files in `conary-core/src/bootstrap/`:

- `mod.rs` -- module root, re-exports public types
- `config.rs` -- `BootstrapConfig`, `TargetArch`
- `stages.rs` -- `BootstrapStage` enum (6 variants), `StageManager`, `StageState`
- `cross_tools.rs` -- `CrossToolsBuilder` (Phase 1)
- `temp_tools.rs` -- `TempToolsBuilder` (Phase 2)
- `final_system.rs` -- `FinalSystemBuilder`, `SYSTEM_BUILD_ORDER` (Phase 3)
- `system_config.rs` -- `configure_system()` (Phase 4)
- `image.rs` -- `ImageBuilder`, `ImageFormat`, `ImageSize`, `ImageTools`, `ImageResult` (Phase 5)
- `tier2.rs` -- `Tier2Builder` (Phase 6)
- `build_runner.rs` -- `PackageBuildRunner` (source fetch/verify/extract/build)
- `build_helpers.rs` -- shared build helper functions
- `chroot_env.rs` -- `ChrootEnv` for Phase 3 chroot setup
- `toolchain.rs` -- `Toolchain`, `ToolchainKind`, version detection
- `repart.rs` -- `RepartDefinition` for systemd-repart partition configs
- `adopt_seed.rs` -- seed adoption for bootstrapping from existing packages

## Checkpointing and Resume

`StageManager` persists to `bootstrap-state.json` after every state change.
Stage-level checkpointing tracks which of the 6 phases are complete.
Per-package checkpointing within a stage (via `mark_package_complete()`)
allows resumed builds to skip already-built packages. Calling `reset_from()`
on a stage clears it and all subsequent stages.

## Image Generation

`ImageBuilder` prefers systemd-repart for rootless GPT image creation.
`RepartDefinition` generates `repart.d/*.conf` files for ESP and root
partitions with architecture-aware type GUIDs. Falls back to sfdisk/mkfs
when systemd-repart is unavailable. Supports raw, qcow2 (via qemu-img),
hybrid ISO output, and EROFS generation images (CAS + EROFS + DB for
composefs-native boot).

## Architecture Context

The bootstrap module uses TOML recipes from the recipe module, building
them with `RecipeGraph` for dependency ordering. Phase 3 builds run
inside chroot environments via `ChrootEnv`. Completed bootstrap images
can be booted directly or used as the foundation for a Conary-managed
system.

Supports x86_64, aarch64, and riscv64 targets.

## Canonical CLI

The active user-facing bootstrap commands are:

```bash
conary bootstrap init --target x86_64
conary bootstrap cross-tools
conary bootstrap temp-tools
conary bootstrap system
conary bootstrap config
conary bootstrap image --format qcow2
conary bootstrap tier2
conary bootstrap run conaryos.toml --seed ./seed
conary bootstrap verify-convergence --run-a ./bootstrap-a --run-b ./bootstrap-b
conary bootstrap diff-seeds ./seed-a ./seed-b
```

Older phase-style bootstrap names are no longer the public CLI and should not
appear in active docs or tests.

## Manifest Pipeline Artifacts

`conary bootstrap run` now writes operation-scoped artifacts under:

```text
<work_dir>/
  operations/
    latest.json
    <op-id>.json
    <op-id>/
      derivations.db
      logs/
      pipeline/
      output/
        generations/1/
        current -> generations/1
  output/
    current -> ../operations/<op-id>/output/current
```

The per-run record stores the manifest path, recipe directory, seed ID,
requested filters (`--up-to`, `--only`, `--cascade`), the operation-scoped
derivation database path, output path, generation path, profile hash, and
success/failure state. `operations/latest.json` points at the most recent
completed run record for later inspection and comparison.

## Comparison Commands

`conary bootstrap verify-convergence` compares two completed bootstrap run
workdirs, not raw seed paths. Each workdir is resolved through
`<work_dir>/operations/latest.json`, and the command opens the recorded
`derivations.db` for that completed run. Optional `--seed-a` and `--seed-b`
arguments verify that the provided seed directories match the run record’s
stored seed ID before comparing outputs.

`conary bootstrap diff-seeds` is intentionally narrow in this milestone. It
compares:

- `seed.toml` metadata fields
- `seed.erofs` content hashes
- top-level artifact presence (`seed.toml`, `seed.erofs`, `cas/`)

It does not mount or recursively diff EROFS contents.

See also: [docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
