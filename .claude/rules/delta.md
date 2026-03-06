---
paths:
  - "conary-core/src/delta/**"
---

# Delta Module

Binary delta updates using zstd dictionary compression. The old file version
serves as a compression dictionary for the new version, providing excellent
compression ratios for similar files (e.g., updated binaries).

## Key Types
- `DeltaGenerator` -- creates deltas: `generate_delta(old_hash, new_hash, output_path)`
- `DeltaApplier` -- reconstructs new version: `apply_delta(old_hash, delta_path, expected_new_hash)`
- `DeltaMetrics` -- tracks `old_size`, `new_size`, `delta_size`, `compression_ratio`, `bandwidth_saved`

## Constants
- `COMPRESSION_LEVEL` -- zstd level 3 (fast, good compression) in `generator.rs`
- `MAX_DELTA_OUTPUT_SIZE` -- 2 GiB decompression limit in `applier.rs`
- `MAX_DELTA_RATIO` -- 0.9 (90%) threshold; deltas above this are not worthwhile

## Invariants
- Both generator and applier use `CasStore` to retrieve file content by hash
- Delta format: `zstd_compress(new_content, dictionary=old_content)`
- Generator fsyncs output file after writing (crash safety)
- Applier verifies output hash matches `expected_new_hash` (integrity check)
- `DeltaMetrics::is_worthwhile()` returns true only if `compression_ratio < MAX_DELTA_RATIO`

## Gotchas
- Generator and applier each create their own `CasStore` from a root path
- `bandwidth_saved` uses saturating subtraction with `i64::try_from` overflow protection
- `savings_percentage()` returns 0.0 for zero-size new files (division guard)
- Delta path is caller-provided -- not stored in CAS
- Both old and new content are loaded fully into memory for compression

## Files
- `mod.rs` -- module re-exports, integration tests
- `generator.rs` -- `DeltaGenerator`, `COMPRESSION_LEVEL`, zstd dictionary compression
- `applier.rs` -- `DeltaApplier`, `MAX_DELTA_OUTPUT_SIZE`, zstd dictionary decompression
- `metrics.rs` -- `DeltaMetrics`, `MAX_DELTA_RATIO`, `is_worthwhile()`, `savings_percentage()`
