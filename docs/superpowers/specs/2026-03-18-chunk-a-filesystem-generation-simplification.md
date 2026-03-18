---
last_updated: 2026-03-18
revision: 1
summary: Simplify filesystem and generation modules -- consolidate CAS walk, fsverity, kernel detection, export scoping
---

# Chunk A: Filesystem & Generation Simplification

## Overview

Targeted refactoring of conary-core's filesystem/ and generation/ modules to
eliminate duplication identified by three parallel code reviews (reuse, quality,
efficiency).

## Changes

### 1. CAS Walk Iterator

Add `CasStore::iter_objects()` returning an iterator of `(String, PathBuf)` (hash,
path) for all objects in the two-level directory. Consistent temp file filtering
(skip `.` prefix and `.tmp` suffix). Replace 4 duplicate walk sites: gc.rs,
fsverity.rs, export.rs, system.rs.

### 2. FsVerityEnableArg Consolidation

Make `FsverityEnableArg` and `FS_IOC_ENABLE_VERITY` `pub(crate)` in fsverity.rs.
Rewrite `composefs.rs::supports_fsverity()` to call `fsverity::enable_fsverity()`
on a probe file instead of reimplementing the ioctl.

### 3. Kernel Version Detection

Delete `bootstrap/image.rs::detect_kernel_in_sysroot()` (exact duplicate). Use
`metadata::detect_kernel_version()` instead.

### 4. composefs_rs_eval.rs Cleanup

Delete the 446-line evaluation file. The builder tests cover the same ground.
Keep only the bloom filter verification test if not covered elsewhere.

### 5. Export Scoped to Generation

Replace `export.rs::collect_generation_cas_hashes()` with `gc::live_cas_hashes()`
scoped to the target generation's state_id.
