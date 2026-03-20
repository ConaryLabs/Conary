---
paths:
  - "conary-core/src/filesystem/**"
---

# Filesystem Module

Content-addressable storage (CAS) and virtual filesystem tree. Files are stored
by content hash (SHA-256 or XXH128) enabling deduplication and integrity
verification. File deployment is handled by generation building
(see `crate::generation`), not by this module.

## Key Types
- `CasStore` -- content-addressable store with configurable `HashAlgorithm`
- `VfsTree` -- in-memory filesystem tree with arena allocation and O(1) path lookup
- `NodeId` -- lightweight index into VFS arena (just a `usize`)
- `NodeKind` -- `Directory`, `File { hash, size }`, `Symlink { target }`

## Constants
- Default hash: `HashAlgorithm::Sha256` (use `CasStore::with_algorithm()` for XXH128)

## Invariants
- `CasStore::atomic_store()` writes to temp file, fsyncs, then renames -- crash-safe
- Temp names use PID + monotonic counter to avoid races across threads/processes
- Empty paths after normalization are rejected
- VFS uses arena allocation (contiguous Vec) for cache-friendly traversal

## Gotchas
- No FileDeployer -- file deployment was removed; composefs generation building
  mounts the EROFS image over the filesystem instead
- `CasStore::new()` defaults to SHA-256; use `with_algorithm()` for XXH128
- `VfsTree` path lookup is O(1) via HashMap, not tree traversal
- The `path` submodule contains `safe_join()` used by transaction recovery

## Files
- `cas.rs` -- `CasStore`, atomic storage, content retrieval, hash algorithm selection
- `vfs/mod.rs` -- `VfsTree`, `NodeId`, `NodeKind`, arena-based tree
- `fsverity.rs` -- fs-verity support for content verification
- `path.rs` -- `safe_join()`, `sanitize_filename()`, `sanitize_path()` path utilities
