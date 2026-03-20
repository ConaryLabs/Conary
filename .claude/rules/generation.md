---
paths:
  - "conary-core/src/generation/**"
---

# Generation Module

EROFS generation building, composefs mounting, /etc merge, garbage collection,
and image deltas. This module is the core of the composefs architecture:
every package transaction produces a new generation (an immutable EROFS image
mounted via composefs).

## Key Types
- `GenerationBuilder` -- builds EROFS images from DB state using composefs-rs
- `BuildResult` -- image_path, image_size, cas_objects_referenced, duration
- `GenerationMetadata` -- JSON metadata: generation number, timestamp, summary, package count, format
- `MountOptions` -- image_path, basedir (CAS), mount_point, verity, digest, upperdir, workdir
- `EtcMergeResult` -- three-way merge outcome for /etc across generation transitions
- `GcResult` -- generations removed, space reclaimed

## Constants
- `EROFS_IMAGE_NAME` -- `"root.erofs"` (in `metadata.rs`)
- `GENERATION_METADATA_FILE` -- `"metadata.json"` (in `metadata.rs`)
- `GENERATION_FORMAT` -- `"composefs-erofs-v1"` (in `metadata.rs`)

## Generation Lifecycle
```
DB state (troves + files)
    |
build_generation_from_db()   -- composefs-rs builds EROFS image
    |
generations/{N}/root.erofs   -- immutable image
generations/{N}/metadata.json
    |
mount_generation()           -- composefs mount with CAS basedir
    |
/conary/current -> generations/{N}  -- symlink to active generation
```

## composefs-rs vs conary-erofs

Generation building uses **composefs-rs** (v0.3.0, feature-gated behind
`composefs-rs`) as the primary EROFS builder. The **conary-erofs** crate still
exists as a standalone low-level EROFS implementation but is not used for
generation building. The `composefs_rs_eval.rs` submodule provides evaluation
and benchmarking of the composefs-rs path.

## /etc Merge

`etc_merge.rs` implements three-way merge for `/etc` across generation
transitions. The upper overlay directory (`etc-state/{N}/`) holds local
modifications that persist across generations.

## Garbage Collection

`gc.rs` removes old generations, keeping the N most recent. Scans
`generations/` directory, skips the currently active generation (via
`/conary/current` symlink), and removes generation directories + EROFS images.

## Gotchas
- `build_generation_from_db()` queries installed troves and file entries from
  SQLite, then constructs the EROFS tree -- no staging directory is used
- `mount_generation()` requires Linux composefs support (kernel 6.2+,
  `CONFIG_EROFS_FS`)
- `composefs.rs` probes for composefs availability at runtime -- not a hard
  requirement for non-mount operations
- The `delta.rs` here computes diffs between EROFS generation images, distinct
  from `src/delta/` which handles CAS-level file deltas
- Feature gate: `composefs-rs` feature enables the composefs-rs builder path
  and integration tests

## Files
- `builder.rs` -- `build_generation_from_db()`, EROFS image construction from DB
- `mount.rs` -- `mount_generation()`, `unmount_generation()`, `current_generation()`,
  `update_current_symlink()`
- `metadata.rs` -- `GenerationMetadata`, constants (image name, format, metadata file)
- `composefs.rs` -- composefs detection and feature probing
- `gc.rs` -- generation garbage collection
- `etc_merge.rs` -- three-way /etc merge across generations
- `delta.rs` -- EROFS image delta computation
- `composefs_rs_eval.rs` -- composefs-rs evaluation (feature-gated)
