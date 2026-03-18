# Composefs Integration — Design Document

**Date:** 2026-03-04
**Status:** Approved
**Goal:** [#189] Full System Takeover (Level 3) — composefs milestone
**Depends on:** Generation infrastructure (completed)

## Summary

Replace reflink-based generation trees with composefs-mounted EROFS images. Each generation becomes a small EROFS image (~2-50MB) that indexes file paths to CAS object digests. The Linux composefs driver mounts the image and serves file content directly from the CAS store with fs-verity integrity verification. `/usr` is immutable; `/etc` is writable via overlayfs.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| EROFS builder | Native Rust, from scratch | Self-contained, CAS-aware optimizations, no external deps |
| Code organization | Separate workspace crate (`conary-erofs`) | Clean API boundary, independently testable |
| EROFS features | Full (compression, inlining, tail packing, chunk dedup) | Maximum space efficiency |
| Activation | Auto-detect default | Use composefs when kernel supports it (6.2+), error otherwise |
| Kernel requirement | Linux 6.2+ required | Target distros (Fedora 43+, Arch, Ubuntu Noble+) all qualify |
| /etc handling | Writable via overlayfs | composefs lower + persistent upper for config changes |
| Image builder tool | None (native Rust) | No mkcomposefs dependency, full control |

## Generation Storage Layout

```
/conary/
  objects/                          # CAS (unchanged) — file content by hash
    ab/c123def...                   # fs-verity enabled on first generation build
    ff/9a8b7c6...
  generations/
    1/
      root.erofs                    # EROFS image (filesystem index)
      .conary-gen.json              # Generation metadata (format: "composefs")
    2/
      root.erofs
      .conary-gen.json
  current -> generations/2
  etc-state/                        # Persistent /etc overlay state
    upper/                          # User config changes survive rollback
    work/                           # overlayfs workdir
```

## `conary-erofs` Crate

### EROFS On-Disk Format

```
┌─────────────────┐  offset 0
│   Superblock     │  128 bytes — magic (0xE0F5E1E2), features, root nid, block size
├─────────────────┤  offset 128
│   Inode Table    │  Packed inodes (compact 32B or extended 64B)
│                  │  Each inode: type, permissions, uid, gid, size, xattrs
├─────────────────┤
│   Directory Data │  Sorted dirent blocks (name hash, inode nid, type)
├─────────────────┤
│   File Data      │  Chunk-based references to external CAS objects
│   (chunk index)  │  Maps file regions → fs-verity digests
├─────────────────┤
│   Xattr Data     │  Extended attributes (fs-verity digests, SELinux labels)
├─────────────────┤
│   Compressed     │  LZ4/LZMA compressed metadata blocks
│   Indexes        │  Compression cluster maps
└─────────────────┘
```

### Public API

```rust
pub struct ErofsBuilder {
    block_size: u32,          // 4096 default
    compression: Compression, // None, Lz4, Lzma
    inline_threshold: u32,    // Inline files smaller than this (64B default)
}

impl ErofsBuilder {
    pub fn new() -> Self;
    pub fn add_file(&mut self, path: &str, digest: &[u8; 32], size: u64, mode: u32, uid: u32, gid: u32);
    pub fn add_symlink(&mut self, path: &str, target: &str, mode: u32);
    pub fn add_directory(&mut self, path: &str, mode: u32, uid: u32, gid: u32);
    pub fn set_fsverity_digest(&mut self, path: &str, digest: &[u8; 32]);
    pub fn build<W: Write>(&self, writer: W) -> Result<()>;
}
```

### Modules

- `superblock.rs` — Superblock construction, magic, feature flags
- `inode.rs` — Compact (32B) and extended (64B) inode layouts
- `dirent.rs` — Directory entry blocks with name hashing and sorting
- `chunk.rs` — Chunk-based external data references (composefs digest mapping)
- `xattr.rs` — Extended attribute storage (inline + shared)
- `compress.rs` — LZ4/LZMA metadata compression, cluster index maps
- `tail_pack.rs` — Tail-end packing for small file fragments
- `builder.rs` — High-level API orchestrating the above
- `verify.rs` — Read-back verification (parse own output, compare)

## Generation Builder Changes

### New Build Flow

1. `StateEngine::create_snapshot()`
2. `mkdir /conary/generations/{N}/`
3. For each trove → for each file:
   - If not excluded: `builder.add_file(path, sha256_hash, size, perms, uid, gid)`
   - Set fs-verity digest: `builder.set_fsverity_digest(path, sha256_hash)`
   - For symlinks: `builder.add_symlink(path, target, perms)`
4. Add root symlinks (bin→usr/bin, etc.) as symlink entries
5. `builder.build(File::create("generations/{N}/root.erofs"))`
6. Write `.conary-gen.json` with `"format": "composefs"`
7. Enable fs-verity on referenced CAS objects (idempotent `FS_IOC_ENABLE_VERITY`)

### fs-verity Enablement

- CAS objects get fs-verity enabled lazily during first generation build
- Once enabled, permanent (kernel enforces)
- If filesystem doesn't support fs-verity, skip with warning
- Track verity status to avoid redundant ioctls

## Mount Integration

### Boot-Time (Dracut)

```bash
# Mount full composefs tree at staging point
mount -t composefs /sysroot/conary/generations/N/root.erofs \
    /sysroot/conary/mnt \
    -o basedir=/sysroot/conary/objects,verity_check=1

# Bind-mount /usr from composefs tree (read-only)
mount --bind /sysroot/conary/mnt/usr /sysroot/usr
mount -o remount,ro /sysroot/usr

# Overlayfs for /etc (writable on top of immutable composefs lower)
mkdir -p /sysroot/conary/etc-state/upper /sysroot/conary/etc-state/work
mount -t overlay overlay /sysroot/etc \
    -o lowerdir=/sysroot/conary/mnt/etc,\
       upperdir=/sysroot/conary/etc-state/upper,\
       workdir=/sysroot/conary/etc-state/work
```

### Live Switch

Mount-based switching replaces `renameat2(RENAME_EXCHANGE)`:

1. Mount new generation's composefs at `/conary/mnt-new`
2. Bind-mount `/conary/mnt-new/usr` over `/usr`, remount read-only
3. Rebuild `/etc` overlay with new lower
4. Update `/conary/current` symlink
5. Unmount old generation's composefs
6. Reboot recommended for full consistency

### /etc Persistence

- Overlay upper dir at `/conary/etc-state/upper` persists across reboots
- User config changes survive generation switches and rollbacks
- New generation build can optionally merge upper changes into EROFS image

## GC and CAS Integration

### Generation GC

Each generation is ~2-50MB (EROFS image + metadata). Deletion is trivial.

### CAS GC Enhancement

CAS objects are shared across generations. Before removing a CAS object, scan all remaining generations:

```
For each generation's EROFS image:
    Parse to extract all referenced digests
    Add to "in-use" set
CAS objects not in the in-use set are candidates for removal
```

## Removed Code

- `src/filesystem/reflink.rs` — No longer needed
- `deployer.deploy_file_reflink()` — Generation builder doesn't deploy files
- `renameat2_exchange()` and `fallback_rename()` — Replaced by mount operations

## CLI Surface

No new subcommands. Existing commands change internal implementation:

```
conary generation build          # Builds EROFS image instead of reflink tree
conary generation switch N       # Composefs remount instead of renameat2
conary generation rollback       # Same semantics, mount-based mechanism
conary generation list           # Same — reads metadata JSON
conary generation info N         # Shows EROFS image size
conary generation gc             # Deletes EROFS images
conary system takeover           # Builds composefs generation
```

## Metadata Changes

`.conary-gen.json` updated format:

```json
{
  "generation": 5,
  "format": "composefs",
  "erofs_size": 12845056,
  "cas_objects_referenced": 48231,
  "fsverity_enabled": true,
  "created_at": "2026-03-04T12:00:00Z",
  "package_count": 847,
  "kernel_version": "6.18.13",
  "summary": "System takeover — initial generation"
}
```

## Error Handling

| Scenario | Behavior |
|----------|----------|
| No composefs kernel support | Error at `generation build` preflight: "Requires Linux 6.2+ with composefs" |
| EROFS build fails | Remove partial generation dir, return error |
| fs-verity enablement fails | Warn, continue (composefs works without verity) |
| CAS object missing at mount | Composefs mount fails with I/O error. Suggest `conary system verify` |
| CAS object corrupted | Composefs returns -EIO. Kernel logs verity failure. `conary system verify --repair` |
| /etc overlay mount fails | Fall back to bind-mount for /etc with warning |
| Live switch fails (busy mount) | Error: "reboot recommended for clean switch" |
| Disk full during EROFS build | Builder returns error, cleanup guard removes partial image |

## Testing

### `conary-erofs` Unit Tests
- Superblock: correct magic, feature flags, block size
- Inodes: compact vs extended layout, permission encoding
- Directory entries: sorting, name hashing, block boundaries
- Chunk index: digest mapping, block alignment
- Xattrs: inline vs shared, fs-verity digest format
- Compression: LZ4/LZMA round-trip, cluster index
- Tail packing: small file fragments in inode tail
- Builder: full image from synthetic tree

### `conary-erofs` Integration Tests
- Build image, `mount -t erofs` via loopback, verify file contents
- Compare output against `mkcomposefs` for identical inputs
- Mount via composefs with `verity_check=1`, confirm integrity
- Corrupt CAS object, confirm composefs rejects it

### Conary Generation Tests
- `conary generation build` produces `root.erofs` + metadata
- `conary generation list` reads composefs generations
- `conary generation gc` removes EROFS images, CAS preserved
- `conary system takeover --dry-run` reports composefs format
