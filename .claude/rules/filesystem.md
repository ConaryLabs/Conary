---
paths:
  - "conary-core/src/filesystem/**"
---

# Filesystem Module

Content-addressable storage (CAS) and file deployment. Files are stored by
content hash (SHA-256 or XXH128) enabling deduplication and efficient rollback.
Deployment uses hardlinks from CAS to install root, falling back to copy for
cross-device moves.

## Key Types
- `CasStore` -- content-addressable store with configurable `HashAlgorithm`
- `FileDeployer` -- deploys files from CAS to install root via hardlinks
- `VfsTree` -- in-memory filesystem tree with arena allocation and O(1) path lookup
- `NodeId` -- lightweight index into VFS arena (just a `usize`)
- `NodeKind` -- `Directory`, `File { hash, size }`, `Symlink { target }`

## Constants
- Default hash: `HashAlgorithm::Sha256` (use `CasStore::with_algorithm()` for XXH128)

## Invariants
- `CasStore::atomic_store()` writes to temp file, fsyncs, then renames -- crash-safe
- Temp names use PID + monotonic counter to avoid races across threads/processes
- `FileDeployer::safe_target_path()` rejects `..` components (path traversal prevention)
- Empty paths after normalization are rejected
- VFS uses arena allocation (contiguous Vec) for cache-friendly traversal

## Gotchas
- Hardlinks require same filesystem -- `EXDEV` triggers copy fallback in transaction module
- `CasStore::new()` defaults to SHA-256; use `with_algorithm()` for XXH128
- `FileDeployer` can be created with existing CAS via `with_cas()` or new CAS via `new()`
- `VfsTree` path lookup is O(1) via HashMap, not tree traversal
- The `path` submodule contains `safe_join()` used by transaction recovery

## Files
- `cas.rs` -- `CasStore`, atomic storage, content retrieval, hash algorithm selection
- `deployer.rs` -- `FileDeployer`, hardlink deployment, path traversal prevention
- `vfs/mod.rs` -- `VfsTree`, `NodeId`, `NodeKind`, arena-based tree
- `path.rs` -- `safe_join()` path utility
