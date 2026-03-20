---
name: derivation_efficiency
description: Efficiency review of conary-core/src/derivation/ module (13 files, ~5600 lines) -- 12 findings for 114-package bootstrap pipeline
type: project
---

Efficiency review of the derivation module (2026-03-20). 12 findings: 2 HIGH, 6 MEDIUM, 4 LOW.

**Why:** Module is the inner loop of the bootstrap pipeline building 114 packages. Memory and I/O inefficiencies multiply across that package count.

**How to apply:** Prioritize HIGH findings (streaming file I/O) before MEDIUM (allocation reduction, batching). LOW findings are polish.

## HIGH

1. **capture.rs:143** -- `std::fs::read(path)` buffers entire DESTDIR files before CAS store. GCC binaries can be 100+ MB. Fix: add `CasStore::store_path()` with streaming hash+write.

2. **compose.rs:388** -- `erofs_image_hash()` reads entire EROFS image into memory via `std::fs::read()`. Stage images can be 200-400 MB. Fix: streaming SHA-256 (same pattern already exists in `src/commands/derivation.rs:sha256_of_path`). Extract shared utility.

## MEDIUM

3. **id.rs:1350, profile.rs:3439** -- `canonical_string()` builds `Vec<String>` then joins. N+1 allocations per call (114 calls). Fix: single `String` with `push_str()`/`write!()`.

4. **pipeline.rs:2857** -- `create_dir_all(sysroot)` inside per-package loop (114 redundant calls). Fix: hoist before loop.

5. **pipeline.rs:2828-2918** -- `completed` map stores full `OutputManifest` but `collect_dep_ids` only needs `DerivationId`. Plus `.clone()` on manifest insertion. Fix: split into `dep_ids` + `stage_manifests`.

6. **executor.rs:929** -- `dep_ids.clone()` on every `execute()` call. Fix: take `dep_ids` by value.

7. **pipeline.rs:2982** -- `ordered_stages()` clones every package name. Fix: return `Vec<(Stage, Vec<&str>)>`.

8. **index.rs** -- No batch insert. 114 individual INSERT transactions with fsync. Fix: add `insert_batch()` wrapped in single transaction.

## LOW

9. **output.rs:2410** -- `format!()` per file in `compute_output_hash()`. Fix: reusable String buffer.
10. **recipe_hash.rs:3793** -- `expand_variables()` does N full-string scans. Fix: short-circuit when no `%(` present.
11. **seed.rs:4309** -- TOCTOU exists() checks before read. Fix: attempt read directly, map NotFound.
12. **profile.rs:3491** -- O(N^2) diff via `.find()` in loop. Fix: HashMap lookup.

## Cross-reference

- Finding 2 duplicates existing known issue from derivation_code_reuse.md (erofs_image_hash OOM)
- `CasStore::store()` takes `&[u8]` only -- no streaming path API exists yet
