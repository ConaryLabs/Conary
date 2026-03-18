---
name: composefs-native code reuse findings
description: Code duplication issues found in the composefs-native branch (2026-03-17) -- hashing, CAS walks, fsverity structs, kernel detection
type: project
---

Composefs-native branch has 8 code reuse issues (3 P1, 4 P2, 1 P3):

**P1 issues:**
- `export.rs::hex_digest()` reimplements `conary_core::hash::sha256()` with direct sha2 import
- `composefs.rs` duplicates `FsverityEnableArg` struct + `FS_IOC_ENABLE_VERITY` const from `fsverity.rs`
- `image.rs::detect_kernel_in_sysroot()` is identical to `metadata.rs::detect_kernel_version()`

**P2 issues:**
- CAS two-level walk pattern duplicated 4x (gc.rs, fsverity.rs, export.rs, system.rs) with inconsistent filtering
- `builder.rs::hex_to_digest()` hand-rolls hex parsing; redundant with `Sha256HashValue::from_hex()`
- `export.rs::collect_generation_cas_hashes()` includes ALL CAS objects instead of using DB-scoped query
- `dir_size()` in image.rs is generic utility without a shared home

**Why:** The composefs-native branch was developed as a large feature branch; these are cross-cutting concerns that escaped notice during initial implementation.

**How to apply:** When reviewing composefs-native changes, flag these patterns. When emerge implements fixes, verify the CAS walk consolidation covers all 4 call sites.
