# Bootstrap Module (conary-core/src/bootstrap/)

Staged bootstrap pipeline for building a complete Conary system from scratch
without an existing package manager. Follows an LFS 12.4-aligned approach
(binutils 2.45, gcc 15.2.0, glibc 2.42, kernel 6.16.1) through cross-compilation,
self-hosting, base system assembly, and bootable image generation.

## Data Flow: Bootstrap Pipeline

```
Host System (any Linux)
     |
  Stage0Builder -- crosstool-ng cross-compiler
     |              Produces: /tools/x86_64-conary-linux-gnu/
     |
  Stage1Builder -- Self-hosted toolchain (5 packages)
     |              linux-headers -> binutils -> gcc-pass1 -> glibc -> gcc-pass2
     |              Produces: /conary/stage1/
     |
  Stage2Builder -- [optional] Reproducibility rebuild with Stage 1
     |              Same 5 packages, verifies self-hosting
     |
  BaseBuilder -- Full base system (~40+ packages)
     |            RecipeGraph topological sort, phased build
     |            ContainerConfig sandboxing per package
     |            Produces: /conary/sysroot/
     |
  ConaryStageBuilder -- [optional] Rust 1.93 + Conary self-build
     |                   Downloads Rust bootstrap, builds from source
     |
  ImageBuilder -- Bootable disk image (raw / qcow2 / iso)
     |             systemd-repart (fallback: sfdisk/mkfs)
     |             GPT: 512MB ESP (FAT32) + root (ext4)
     |
  StageManager -- JSON checkpoint file (bootstrap-state.json)
                   Per-stage completion + per-package checkpointing
```

## Key Types

| Type | File | Purpose |
|------|------|---------|
| `Bootstrap` | mod.rs | Top-level orchestrator coordinating all stages |
| `BootstrapConfig` | config.rs | Toolchain versions, paths, target arch, parallelism |
| `TargetArch` | config.rs | Enum: X86_64, Aarch64, Riscv64 (with triples) |
| `BootstrapStage` | stages.rs | Enum: Stage0 through Image (8 stages) |
| `StageManager` | stages.rs | Progress tracker with JSON persistence and resume |
| `StageState` | stages.rs | Per-stage completion, timestamps, package checkpoints |
| `Stage0Builder` | stage0.rs | crosstool-ng cross-compiler build |
| `Stage1Builder` | stage1.rs | Self-hosted toolchain (5-package strict order) |
| `Stage2Builder` | stage2.rs | Optional reproducibility rebuild and hash comparison |
| `BaseBuilder` | base.rs | Full base system build with RecipeGraph ordering |
| `ConaryStageBuilder` | conary_stage.rs | Rust bootstrap + Conary self-build |
| `ImageBuilder` | image.rs | Disk image generation (raw, qcow2, iso) |
| `ImageFormat` | image.rs | Enum: Raw, Qcow2, Iso |
| `ImageSize` | image.rs | Parsed size specification for disk images |
| `RepartDefinition` | repart.rs | systemd-repart partition config (ESP, root) |
| `DryRunReport` | mod.rs | Validation results: recipe counts, graph check, errors |
| `Toolchain` | toolchain.rs | Resolved toolchain path and kind |
| `Prerequisites` | mod.rs | Host tool availability check (ct-ng, make, gcc, git) |

## Checkpointing and Resume

`StageManager` persists to `bootstrap-state.json` after every state change.
Stage-level checkpointing tracks which of the 8 stages are complete.
Per-package checkpointing within a stage (via `mark_package_complete()`)
allows resumed builds to skip already-built packages. Calling `reset_from()`
on a stage clears it and all subsequent stages.

## Image Generation

`ImageBuilder` prefers systemd-repart for rootless GPT image creation.
`RepartDefinition` generates `repart.d/*.conf` files for ESP and root
partitions with architecture-aware type GUIDs. Falls back to sfdisk/mkfs
when systemd-repart is unavailable. Supports raw, qcow2 (via qemu-img),
and hybrid ISO output.

## Dry-Run Validation

`Bootstrap::dry_run()` validates the full pipeline without building:
parses all Stage 1, base, and Conary recipes; checks for placeholder
checksums; runs `RecipeGraph::topological_sort()` to detect dependency
cycles. Returns a `DryRunReport` with counts, warnings, and errors.

## Architecture Context

The bootstrap module uses TOML recipes from the recipe module, building
them with `RecipeGraph` for dependency ordering. Base system builds run
inside namespace-isolated sandboxes via `ContainerConfig` from the
container module. Completed bootstrap images can be booted directly or
used as the foundation for a Conary-managed system.

See also: [docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
