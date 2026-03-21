---
paths:
  - "conary-core/src/generation/**"
---

# EROFS / Composefs

EROFS image building uses `composefs-rs` (v0.3.0) directly in `conary-core`.
The standalone `conary-erofs` crate was removed (unused dead code). All EROFS
construction happens via `conary-core/src/generation/builder.rs`.

## Key Integration Points
- `conary-core/src/generation/builder.rs` -- builds EROFS images for composefs
- `conary-core/src/derivation/compose.rs` -- composes stage EROFS images during bootstrap
- `composefs-rs` crate provides the EROFS builder API

## Invariants
- All regular files use chunk-based external references (composefs mode)
- No file content stored in images -- only metadata and CAS digest xattrs
- Generation images are immutable once built
