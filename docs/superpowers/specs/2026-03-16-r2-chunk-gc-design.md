---
last_updated: 2026-03-16
revision: 1
summary: Garbage collect orphaned chunks from R2 and local disk
---

# R2 + Local Chunk Garbage Collection

## Problem Statement

When packages are superseded by new versions, chunks unique to the old
version become orphans. They persist indefinitely on local disk and in R2
with no automated cleanup. Over time this wastes storage.

## Design

### Algorithm

1. Build the **referenced set** from `converted_packages.chunk_hashes_json`
   — all chunk hashes belonging to active converted packages. Also include
   hashes from `chunk_access WHERE protected = 1`.
2. Scan **local disk** via existing `scan_chunk_hashes()` in
   `conary-server/src/server/handlers/chunks.rs:855-888` to get all stored
   chunk hashes.
3. If R2 is configured, **list R2 objects** with `list_objects_v2` prefix
   `chunks/` to get the remote set. This requires adding a `list_chunks()`
   method to `R2Store` using the S3 `list_objects_v2` API with pagination.
4. Compute **orphans**: local orphans = stored - referenced, R2 orphans =
   remote - referenced.
5. **Delete** orphans from local disk, R2, and `chunk_access` table.

### Exposure

- **CLI:** `conary system gc --chunks` — extends the existing `system gc`
  command which already handles CAS object GC. Add a `--chunks` flag
  (or make chunks part of the default GC sweep).
- **MCP tool:** `chunk_gc` on remi-admin — for agent-triggered cleanup.
- **Schedulable:** via Forgejo cron or systemd timer on Remi.

### Dry-run mode

`conary system gc --chunks --dry-run` lists what would be deleted without
touching anything. Shows count + total size.

### Safety

- Never delete chunks referenced by any row in `converted_packages` (even
  old versions — only orphaned when the package row itself is removed)
- Respect `chunk_access.protected = 1` — skip protected chunks
- Log every deletion at info level with hash + size
- Single-pass operation, not a background daemon

### Output format

```
Chunk GC: scanned 12,847 local chunks, 12,902 R2 chunks
Referenced: 11,203 chunks (active packages)
Protected: 14 chunks (manually protected)
Orphaned: 1,630 local + 1,685 R2
Freed: 847 MB local, 892 MB R2
```

### Implementation details

**R2Store.list_chunks():** New method using `bucket.list_objects_v2()` with
pagination. Returns `Vec<String>` of chunk hashes. Strips the configured
prefix (`chunks/`) from each key.

**Local deletion:** `std::fs::remove_file()` on each orphan in the
two-level directory structure (`{hash[0:2]}/{hash[2:]}`). Clean up empty
parent directories after deletion.

**Concurrency:** GC runs single-threaded. If a conversion is in progress
concurrently, newly created chunks won't be in the referenced set yet.
Mitigate by only deleting chunks whose `chunk_access.last_accessed` is
older than 1 hour (grace period for in-flight conversions).

**No schema changes needed** — all required data exists in the current
v51 schema.

## Implementation Order

1. Add `list_chunks()` to R2Store
2. Implement `chunk_gc()` function in a new module
   `conary-server/src/server/chunk_gc.rs`
3. Wire into `conary system gc --chunks` CLI
4. Add `chunk_gc` MCP tool to remi-admin
5. Test with `--dry-run` on production

## Success Criteria

- `conary system gc --chunks --dry-run` shows orphan count and size
- `conary system gc --chunks` deletes orphans from local + R2
- Protected chunks are never deleted
- In-flight conversions are not affected (grace period)
- MCP tool `chunk_gc` works for agent-triggered cleanup
