# Composefs Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace reflink-based generation trees with composefs-mounted EROFS images, including a full-featured native Rust EROFS builder.

**Architecture:** New workspace crate `conary-erofs` implements the EROFS on-disk format (superblock, inodes, directories, chunk indexes, xattrs, LZ4/LZMA compression, tail packing). Conary's generation builder is rewritten to produce EROFS images referencing CAS objects. Dracut and live-switch use composefs mounts instead of bind-mounts/renameat2.

**Tech Stack:** Rust 1.92+, EROFS on-disk format (linux/erofs_fs.h), composefs kernel driver (Linux 6.2+), LZ4/LZMA compression, fs-verity, CRC32C

**Design doc:** `docs/plans/2026-03-04-composefs-design.md`

---

## Task 1: Convert to Cargo Workspace

**Files:**
- Modify: `Cargo.toml` (root — convert to workspace)
- Create: `conary-erofs/Cargo.toml`
- Create: `conary-erofs/src/lib.rs`

Convert the monolithic crate into a Cargo workspace with two members: the existing `conary` crate (renamed to `.` member) and the new `conary-erofs` crate.

**Root Cargo.toml changes:**

Add workspace section at top:
```toml
[workspace]
members = [".", "conary-erofs"]
resolver = "3"
```

**conary-erofs/Cargo.toml:**
```toml
[package]
name = "conary-erofs"
version = "0.1.0"
edition = "2024"
rust-version = "1.92"
description = "EROFS filesystem image builder for composefs integration"

[dependencies]
crc32c = "0.6"
lz4_flex = "0.11"
lzma-rs = "0.3"
thiserror = "2"
tracing = "0.1"

[dev-dependencies]
tempfile = "3"
```

**conary-erofs/src/lib.rs:**
```rust
// conary-erofs/src/lib.rs
//! EROFS filesystem image builder
//!
//! Produces valid EROFS images for use with Linux composefs.
//! Supports compression (LZ4, LZMA), inline data, tail packing,
//! and chunk-based external file references.

pub mod superblock;
```

**Verify:** `cargo build` compiles both crates. `cargo test` passes all existing tests.

**Commit:** `feat(erofs): Initialize conary-erofs workspace crate`

---

## Task 2: EROFS Superblock

**Files:**
- Create: `conary-erofs/src/superblock.rs`
- Modify: `conary-erofs/src/lib.rs`

Implement the 128-byte EROFS superblock structure per `erofs_fs.h`.

**Key constants:**
```rust
pub const EROFS_SUPER_MAGIC: u32 = 0xE0F5_E1E2;
pub const EROFS_SUPER_OFFSET: u64 = 1024;
pub const EROFS_DEFAULT_BLKBITS: u8 = 12; // 4096 bytes
```

**Superblock struct** with all fields from the spec (magic, checksum, feature_compat, blkszbits, root_nid, inos, epoch, blocks, meta_blkaddr, xattr_blkaddr, uuid, volume_name, feature_incompat, etc.).

**Methods:**
- `new(block_size: u32) -> Self` — initialize with defaults
- `write_to<W: Write>(&self, w: &mut W) -> Result<()>` — serialize to bytes (little-endian)
- `compute_checksum(&self) -> u32` — CRC32C with initial value `0x5045B54A`

**Feature flags needed for composefs:**
```rust
pub const EROFS_FEATURE_INCOMPAT_CHUNKED_FILE: u32 = 0x0004;
pub const EROFS_FEATURE_INCOMPAT_DEVICE_TABLE: u32 = 0x0008;
```

**Tests:**
- Superblock serializes to exactly 128 bytes
- Magic at offset 0 is `0xE0F5E1E2` (little-endian)
- Checksum is valid CRC32C
- Default block size is 4096 (blkszbits = 12)
- Round-trip: write then parse back, all fields match

**Commit:** `feat(erofs): Add EROFS superblock structure`

---

## Task 3: EROFS Inodes

**Files:**
- Create: `conary-erofs/src/inode.rs`
- Modify: `conary-erofs/src/lib.rs`

Implement compact (32-byte) and extended (64-byte) inode layouts.

**Inode format field encoding:**
- Bit 0: version (0=compact, 1=extended)
- Bits 1-3: data layout (FLAT_PLAIN=0, FLAT_INLINE=2, CHUNK_BASED=4)

**Data layout types needed:**
```rust
pub const EROFS_INODE_FLAT_PLAIN: u16 = 0;
pub const EROFS_INODE_FLAT_INLINE: u16 = 2;  // tail packing
pub const EROFS_INODE_CHUNK_BASED: u16 = 4;  // composefs external refs
```

**CompactInode (32 bytes):** i_format, i_xattr_icount, i_mode, i_nlink, i_size (u32), i_mtime, i_u (union), i_ino, i_uid (u16), i_gid (u16), i_reserved

**ExtendedInode (64 bytes):** Same fields but i_size is u64, i_uid/i_gid are u32, i_mtime is u64+nsec, i_nlink is u32, plus 16 bytes reserved

**Inode NID calculation:**
```rust
/// NID = (byte_offset - meta_blkaddr * block_size) / 32
pub fn nid_from_offset(byte_offset: u64, meta_blkaddr: u32, block_size: u32) -> u64
```

**File type constants** (for i_mode):
```rust
pub const S_IFREG: u16 = 0o100000;
pub const S_IFDIR: u16 = 0o040000;
pub const S_IFLNK: u16 = 0o120000;
```

**Tests:**
- Compact inode serializes to exactly 32 bytes
- Extended inode serializes to exactly 64 bytes
- i_format encodes version and layout correctly
- File mode preserves permission bits
- NID calculation is correct for various offsets

**Commit:** `feat(erofs): Add compact and extended inode layouts`

---

## Task 4: Directory Entries

**Files:**
- Create: `conary-erofs/src/dirent.rs`
- Modify: `conary-erofs/src/lib.rs`

Implement EROFS directory entry format. Each dirent is 12 bytes, followed by variable-length filenames within the same block.

**Dirent structure (12 bytes):**
```rust
pub struct ErofsDirent {
    pub nid: u64,       // target inode NID
    pub nameoff: u16,   // filename offset within block
    pub file_type: u8,  // DT_REG=8, DT_DIR=4, DT_LNK=10
    pub reserved: u8,
}
```

**Directory block layout:**
```
[dirent0][dirent1]...[direntN][name0\0][name1\0]...[nameN\0][padding]
```

First entry's `nameoff` indicates where names start (= 12 * entry_count).

**Key function:**
```rust
/// Pack directory entries into blocks, sorting alphabetically.
/// Returns Vec<Vec<u8>> of block-sized buffers.
pub fn pack_directory(entries: &[(String, u64, u8)], block_size: u32) -> Vec<Vec<u8>>
```

Entries must be sorted alphabetically. If entries + names exceed one block, split across multiple blocks.

**File type constants:**
```rust
pub const EROFS_FT_REG_FILE: u8 = 8;
pub const EROFS_FT_DIR: u8 = 4;
pub const EROFS_FT_SYMLINK: u8 = 10;
```

**Tests:**
- Single entry packs correctly
- Multiple entries sorted alphabetically
- Block overflow splits into multiple blocks
- nameoff values point to correct filenames
- File types are correct for reg/dir/symlink

**Commit:** `feat(erofs): Add directory entry packing`

---

## Task 5: Chunk-Based External Data (Composefs)

**Files:**
- Create: `conary-erofs/src/chunk.rs`
- Modify: `conary-erofs/src/lib.rs`

Implement chunk-based data layout for composefs external file references. Instead of inline data, files reference external CAS objects by digest.

**Chunk index entry (8 bytes):**
```rust
pub struct ChunkIndex {
    pub advise: u16,     // always 0
    pub device_id: u16,  // 0 for primary device
    pub blkaddr: u32,    // block address in external device
}
```

**Chunk format flags (stored in inode):**
```rust
pub const EROFS_CHUNK_FORMAT_BLKBITS_MASK: u16 = 0x001F;
pub const EROFS_CHUNK_FORMAT_INDEXES: u16 = 0x0020;
```

For composefs, files use `CHUNK_BASED` layout. The actual content is served from the `basedir` at mount time. The chunk index tells composefs which external object to read.

**Composefs-specific:** Files get xattrs pointing to CAS objects (handled in Task 6). The chunk index is secondary — composefs primarily uses xattrs for the digest-to-file mapping.

**Tests:**
- ChunkIndex serializes to 8 bytes
- Chunk format flags encode correctly in inode
- CHUNK_BASED layout inodes reference external data

**Commit:** `feat(erofs): Add chunk-based external data references`

---

## Task 6: Extended Attributes (xattrs)

**Files:**
- Create: `conary-erofs/src/xattr.rs`
- Modify: `conary-erofs/src/lib.rs`

Implement xattr storage for composefs digests and overlay metadata. This is critical — composefs uses xattrs to map files to external CAS objects.

**Xattr entry (4+ bytes):**
```rust
pub struct XattrEntry {
    pub e_name_len: u8,
    pub e_name_index: u8,  // namespace prefix index
    pub e_value_size: u16,
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}
```

**Name index constants (namespace prefixes):**
```rust
pub const EROFS_XATTR_INDEX_USER: u8 = 1;
pub const EROFS_XATTR_INDEX_POSIX_ACL_ACCESS: u8 = 2;
pub const EROFS_XATTR_INDEX_POSIX_ACL_DEFAULT: u8 = 3;
pub const EROFS_XATTR_INDEX_TRUSTED: u8 = 4;
pub const EROFS_XATTR_INDEX_SECURITY: u8 = 6;
```

**Composefs xattrs per file:**
- `trusted.overlay.redirect` → path to CAS object (e.g., `"ab/c123def..."`)
- `trusted.overlay.metacopy` → empty value (flag indicating external data)
- `user.overlay.redirect` → alternative namespace
- fs-verity digest stored as xattr value (32 bytes SHA-256)

**Xattr body layout after inode:**
```
[xattr_ibody_header (12 bytes)]
  h_shared_count: u8
  h_reserved: [u8; 7]
  h_shared_xattrs: [u32; h_shared_count]  // refs to shared xattr blocks
[inline xattr entries, 4-byte aligned]
```

**Key function:**
```rust
/// Build xattr data for a file with composefs digest.
/// Returns (ibody_bytes, xattr_icount) to embed after inode.
pub fn build_composefs_xattrs(cas_path: &str, digest: &[u8; 32]) -> (Vec<u8>, u16)
```

**Tests:**
- Xattr entry serializes correctly
- Composefs redirect xattr contains correct CAS path
- Metacopy xattr is present (empty value)
- ibody_header shared count is correct
- Entries are 4-byte aligned
- i_xattr_icount matches actual count

**Commit:** `feat(erofs): Add xattr support for composefs digests`

---

## Task 7: LZ4 Compression

**Files:**
- Create: `conary-erofs/src/compress.rs`
- Modify: `conary-erofs/src/lib.rs`

Implement LZ4 metadata compression for EROFS. This compresses directory blocks and xattr blocks to reduce image size.

**Compression enum:**
```rust
pub enum Compression {
    None,
    Lz4,
    Lzma,
}
```

**EROFS compression cluster structure:**
- Logical clusters map to physical clusters
- Each cluster has a type (HEAD, NONHEAD, PLAIN)
- Compression index stored per logical cluster

**Cluster index for LZ4:**
```rust
pub struct CompressedCluster {
    pub cluster_type: ClusterType,
    pub blkaddr: u32,      // physical block address
    pub clusterofs: u16,   // offset within decompressed data
}
```

Use `lz4_flex` crate for LZ4 block compression. Compress each metadata block independently.

**Key function:**
```rust
/// Compress a block with LZ4. Returns compressed bytes if smaller, None otherwise.
pub fn compress_lz4(data: &[u8]) -> Option<Vec<u8>>

/// Build compression index for a sequence of blocks.
pub fn build_compress_index(blocks: &[CompressedBlock]) -> Vec<u8>
```

**Tests:**
- Compressed output is smaller than input for typical metadata
- Incompressible data returns None (store uncompressed)
- Compression index entries are correctly formatted
- Round-trip: compress then decompress matches original

**Commit:** `feat(erofs): Add LZ4 metadata compression`

---

## Task 8: LZMA Compression

**Files:**
- Modify: `conary-erofs/src/compress.rs`

Add LZMA (MicroLZMA) compression as an alternative to LZ4. Higher compression ratio, slower.

Use `lzma-rs` crate. EROFS uses MicroLZMA format (single-stream LZMA without the .lzma header).

**Key function:**
```rust
pub fn compress_lzma(data: &[u8]) -> Option<Vec<u8>>
```

**Feature flag in superblock:**
```rust
pub const EROFS_COMPR_ALG_LZ4: u16 = 1;
pub const EROFS_COMPR_ALG_LZMA: u16 = 2;
```

**Tests:**
- LZMA compressed output is smaller than LZ4 for metadata
- Round-trip compress/decompress matches
- Superblock compr_algs field set correctly

**Commit:** `feat(erofs): Add LZMA metadata compression`

---

## Task 9: Tail Packing

**Files:**
- Create: `conary-erofs/src/tail_pack.rs`
- Modify: `conary-erofs/src/lib.rs`

Implement tail-end packing for small files. Files smaller than the block size can be packed into the inode's tail, after the inode metadata and xattrs. This uses the `FLAT_INLINE` data layout.

**Layout:**
```
[inode (32 or 64 bytes)][xattr ibody][tail data up to block boundary]
```

The inode's `i_u` field stores the size of inline data. The data follows immediately after xattrs.

**Key function:**
```rust
/// Determine if a file should be tail-packed.
/// Returns true if file fits in remaining block space after inode+xattrs.
pub fn should_tail_pack(file_size: u64, inode_size: u32, xattr_size: u32, block_size: u32) -> bool

/// Pack tail data after inode+xattrs, returning the combined buffer.
pub fn pack_tail(inode_bytes: &[u8], xattr_bytes: &[u8], data: &[u8]) -> Vec<u8>
```

For composefs, symlink targets are always tail-packed (they're small strings). Small config files may also qualify.

**Tests:**
- Small file (< block_size - inode_size) gets tail-packed
- Large file does not get tail-packed
- Symlink target is always tail-packed
- Tail data is correctly positioned after inode+xattrs
- FLAT_INLINE layout set in inode i_format

**Commit:** `feat(erofs): Add tail-end packing for small files`

---

## Task 10: High-Level Builder API

**Files:**
- Create: `conary-erofs/src/builder.rs`
- Modify: `conary-erofs/src/lib.rs`

Implement the `ErofsBuilder` that orchestrates all components into a valid EROFS image.

**Builder struct:**
```rust
pub struct ErofsBuilder {
    block_size: u32,
    compression: Compression,
    inline_threshold: u32,
    entries: Vec<FsEntry>,
}

enum FsEntry {
    File { path: String, digest: [u8; 32], size: u64, mode: u32, uid: u32, gid: u32 },
    Symlink { path: String, target: String, mode: u32 },
    Directory { path: String, mode: u32, uid: u32, gid: u32 },
}
```

**Build process:**
1. Sort entries by path (for deterministic output)
2. Build directory tree from flat entries
3. Assign inode NID to each node (bottom-up)
4. For each file: determine layout (CHUNK_BASED for external, FLAT_INLINE for tail-packed)
5. Build xattr data per inode (composefs digests)
6. Pack directory entries into blocks
7. Optionally compress metadata blocks (LZ4 or LZMA)
8. Write: 1024 bytes padding → superblock → inode table → directory data → xattr data → chunk indexes
9. Compute and write CRC32C checksum in superblock

**Public API:**
```rust
impl ErofsBuilder {
    pub fn new() -> Self;
    pub fn block_size(mut self, size: u32) -> Self;
    pub fn compression(mut self, comp: Compression) -> Self;
    pub fn add_file(&mut self, path: &str, digest: &[u8; 32], size: u64, mode: u32, uid: u32, gid: u32);
    pub fn add_symlink(&mut self, path: &str, target: &str, mode: u32);
    pub fn add_directory(&mut self, path: &str, mode: u32, uid: u32, gid: u32);
    pub fn build<W: Write + Seek>(&self, writer: W) -> Result<BuildStats>;
}

pub struct BuildStats {
    pub image_size: u64,
    pub inode_count: u64,
    pub file_count: u64,
    pub compressed_blocks: u64,
    pub tail_packed_files: u64,
}
```

**Tests:**
- Empty image (root dir only) produces valid superblock + root inode
- Single file image has correct inode and xattrs
- Directory hierarchy produces correct dirent blocks
- Symlinks are tail-packed
- Image with 1000 files builds without error
- BuildStats reports correct counts
- Deterministic: same input always produces same output (byte-identical)

**Commit:** `feat(erofs): Add high-level ErofsBuilder API`

---

## Task 11: Image Verification

**Files:**
- Create: `conary-erofs/src/verify.rs`
- Modify: `conary-erofs/src/lib.rs`

Implement a reader/verifier that can parse EROFS images we produce and validate them. This is essential for testing — we can't rely on kernel mounting for unit tests.

**Key functions:**
```rust
/// Parse and validate an EROFS image.
pub fn verify_image<R: Read + Seek>(reader: R) -> Result<ImageInfo>

pub struct ImageInfo {
    pub block_size: u32,
    pub inode_count: u64,
    pub root_nid: u64,
    pub features: u32,
    pub files: Vec<FileInfo>,
}

pub struct FileInfo {
    pub path: String,
    pub file_type: FileType,
    pub mode: u32,
    pub size: u64,
    pub digest: Option<[u8; 32]>,  // from xattr
}
```

**Verification checks:**
- Superblock magic is correct
- CRC32C checksum matches
- Root inode exists and is a directory
- All directory entries point to valid inodes
- All xattr entries are well-formed
- File paths reconstruct correctly from directory tree walk

**Tests:**
- Build image with ErofsBuilder, verify with verify_image
- Corrupt magic → verify fails
- Corrupt checksum → verify fails
- All files/symlinks/dirs round-trip through build→verify

**Commit:** `feat(erofs): Add EROFS image verification`

---

## Task 12: Integration Test — Mount Real Image

**Files:**
- Create: `conary-erofs/tests/mount_test.rs`

Integration test that builds an EROFS image, writes test CAS objects to a temp dir, and mounts via composefs (requires root or CI with namespace support).

```rust
#[test]
#[ignore] // Requires root + composefs kernel support
fn test_composefs_mount() {
    // 1. Create temp dir with CAS objects
    // 2. Build EROFS image referencing them
    // 3. mount -t composefs image.erofs /mnt -o basedir=cas_dir
    // 4. Verify mounted files match expected content
    // 5. Unmount and cleanup
}
```

Also test: corrupt a CAS object, mount with `verity_check=1`, confirm I/O error.

**Commit:** `test(erofs): Add composefs mount integration test`

---

## Task 13: Add conary-erofs Dependency to Conary

**Files:**
- Modify: `Cargo.toml` (root conary crate)
- Modify: `src/commands/generation/mod.rs`

Add `conary-erofs` as a dependency of the main conary crate:

```toml
[dependencies]
conary-erofs = { path = "conary-erofs" }
```

**Verify:** `cargo build` succeeds with the new dependency.

**Commit:** `build: Add conary-erofs dependency to main crate`

---

## Task 14: Composefs Detection and Preflight

**Files:**
- Create: `src/commands/generation/composefs.rs`
- Modify: `src/commands/generation/mod.rs`

Add composefs kernel support detection and preflight checks.

**Detection:**
```rust
/// Check if the running kernel supports composefs.
pub fn supports_composefs() -> bool {
    // Check /sys/fs/composefs exists, or try test mount
    Path::new("/sys/module/composefs").exists()
        || Path::new("/proc/filesystems").read_to_string()
            .map(|s| s.contains("composefs"))
            .unwrap_or(false)
}

/// Check if fs-verity is supported on the given directory.
pub fn supports_fsverity(path: &Path) -> bool {
    // Create temp file, try FS_IOC_ENABLE_VERITY, cleanup
}
```

**Preflight for generation build:**
```rust
pub fn preflight_composefs(cas_dir: &Path) -> Result<ComposefsCaps> {
    if !supports_composefs() {
        return Err(anyhow!("Composefs not supported. Requires Linux 6.2+"));
    }
    let fsverity = supports_fsverity(cas_dir);
    Ok(ComposefsCaps { fsverity })
}
```

**Tests:**
- Detection functions don't panic on non-composefs systems
- Preflight returns error with clear message when unsupported

**Commit:** `feat(generation): Add composefs detection and preflight`

---

## Task 15: Rewrite Generation Builder for EROFS

**Files:**
- Modify: `src/commands/generation/builder.rs`
- Modify: `src/commands/generation/metadata.rs`

Replace the file-by-file reflink deployment with EROFS image building.

**New build_generation flow:**
```rust
pub fn build_generation(conn: &Connection, db_path: &str, summary: &str) -> Result<i64> {
    composefs::preflight_composefs(&objects_dir(db_path))?;

    let state = StateEngine::new(conn);
    let gen_number = state.create_snapshot(summary)?;
    let gen_dir = generation_path(gen_number);
    std::fs::create_dir_all(&gen_dir)?;

    let troves = Trove::list_all(conn)?;
    let mut builder = conary_erofs::ErofsBuilder::new()
        .compression(conary_erofs::Compression::Lz4);

    // Add root directory
    builder.add_directory("/", 0o755, 0, 0);

    for trove in &troves {
        let trove_id = match trove.id { Some(id) => id, None => continue };
        let files = FileEntry::find_by_trove(conn, trove_id)?;

        for file in &files {
            if is_excluded(&file.path) { continue; }

            let digest = hex_to_bytes(&file.sha256_hash)?;
            builder.add_file(
                &file.path, &digest, file.size as u64,
                file.permissions as u32, 0, 0,
            );
        }
    }

    // Add root symlinks
    for (link, target) in ROOT_SYMLINKS {
        builder.add_symlink(link, target, 0o777);
    }

    // Build EROFS image
    let image_path = gen_dir.join("root.erofs");
    let file = File::create(&image_path)?;
    let stats = builder.build(BufWriter::new(file))?;

    // Enable fs-verity on referenced CAS objects
    enable_fsverity_on_cas(db_path, conn)?;

    // Write metadata
    let metadata = GenerationMetadata {
        generation: gen_number,
        format: "composefs".to_string(),
        erofs_size: stats.image_size,
        cas_objects_referenced: stats.file_count,
        // ... other fields
    };
    metadata.write_to(&gen_dir)?;

    Ok(gen_number)
}
```

**Metadata changes:** Add `format`, `erofs_size`, `cas_objects_referenced`, `fsverity_enabled` fields to `GenerationMetadata`.

**Tests:**
- `build_generation` produces `root.erofs` file in generation dir
- Metadata JSON has `format: "composefs"`
- EROFS image passes `verify_image()` check
- Error on systems without composefs

**Commit:** `feat(generation): Rewrite builder to produce EROFS images`

---

## Task 16: fs-verity Enablement on CAS Objects

**Files:**
- Create: `src/filesystem/fsverity.rs`
- Modify: `src/filesystem/mod.rs`

Enable fs-verity on CAS objects so composefs can verify integrity at read time.

**FS_IOC_ENABLE_VERITY ioctl:**
```rust
const FS_IOC_ENABLE_VERITY: u64 = 0x40806685; // _IOW('f', 0x85, struct fsverity_enable_arg)

#[repr(C)]
struct FsverityEnableArg {
    version: u32,        // 1
    hash_algorithm: u32, // FS_VERITY_HASH_ALG_SHA256 = 1
    block_size: u32,     // 4096
    salt_size: u32,      // 0
    salt_ptr: u64,       // 0
    sig_size: u32,       // 0
    reserved1: u32,      // 0
    sig_ptr: u64,        // 0
    reserved2: [u64; 11],// zeroed
}
```

**Key function:**
```rust
/// Enable fs-verity on a file. No-op if already enabled.
pub fn enable_fsverity(path: &Path) -> Result<()>

/// Enable fs-verity on all CAS objects referenced by tracked files.
pub fn enable_fsverity_on_cas(db_path: &str, conn: &Connection) -> Result<(u64, u64)>
// Returns (enabled_count, already_enabled_count)
```

**Tests:**
- enable_fsverity on unsupported fs returns error (not panic)
- enable_fsverity is idempotent (second call is no-op)
- Struct layout matches kernel expectations

**Commit:** `feat(fs): Add fs-verity enablement for CAS objects`

---

## Task 17: Update Dracut Module for Composefs

**Files:**
- Modify: `packaging/dracut/90conary/conary-generator.sh`
- Modify: `packaging/dracut/90conary/module-setup.sh`

Replace bind-mounts with composefs mounts in the initramfs hook.

**module-setup.sh changes:**
```bash
check() {
    [ -d /conary/generations ] && return 0
    return 255
}

depends() {
    return 0
}

install() {
    inst_hook pre-pivot 90 "$moddir/conary-generator.sh"
    # Include mount.composefs if available
    inst_multiple -o mount.composefs
}
```

**conary-generator.sh changes:**
```bash
#!/bin/bash
# Pre-pivot hook: mount Conary generation via composefs

# Read conary.generation=N from kernel cmdline
CONARY_GEN=""
for opt in $(cat /proc/cmdline); do
    case "$opt" in
        conary.generation=*) CONARY_GEN="${opt#conary.generation=}" ;;
    esac
done

# Resolve generation directory
if [ -z "$CONARY_GEN" ]; then
    if [ -L /sysroot/conary/current ]; then
        RAW_TARGET=$(readlink /sysroot/conary/current)
        GEN_DIR="/sysroot${RAW_TARGET}"
    else
        exit 0
    fi
else
    GEN_DIR="/sysroot/conary/generations/${CONARY_GEN}"
fi

EROFS_IMG="${GEN_DIR}/root.erofs"
CAS_DIR="/sysroot/conary/objects"

if [ ! -f "$EROFS_IMG" ]; then
    echo "conary: EROFS image not found at $EROFS_IMG" >&2
    exit 0
fi

# Mount composefs at staging point
mkdir -p /sysroot/conary/mnt
mount -t composefs "$EROFS_IMG" /sysroot/conary/mnt \
    -o "basedir=${CAS_DIR},verity_check=1" 2>/dev/null || \
mount -t composefs "$EROFS_IMG" /sysroot/conary/mnt \
    -o "basedir=${CAS_DIR}" || {
    echo "conary: composefs mount failed" >&2
    exit 1
}

# Bind-mount /usr from composefs tree (read-only)
if [ -d /sysroot/conary/mnt/usr ]; then
    mount --bind /sysroot/conary/mnt/usr /sysroot/usr
    mount -o remount,ro /sysroot/usr
fi

# Overlayfs for /etc (writable upper on immutable composefs lower)
if [ -d /sysroot/conary/mnt/etc ]; then
    mkdir -p /sysroot/conary/etc-state/upper /sysroot/conary/etc-state/work
    mount -t overlay overlay /sysroot/etc \
        -o "lowerdir=/sysroot/conary/mnt/etc,upperdir=/sysroot/conary/etc-state/upper,workdir=/sysroot/conary/etc-state/work"
fi
```

**Commit:** `feat(boot): Update dracut module for composefs mounts`

---

## Task 18: Update Live Switch for Mount-Based Switching

**Files:**
- Modify: `src/commands/generation/switch.rs`

Replace `renameat2(RENAME_EXCHANGE)` with composefs mount-based switching.

**New switch_live:**
```rust
pub fn switch_live(gen_number: i64) -> Result<()> {
    let gen_dir = generation_path(gen_number);
    let erofs_img = gen_dir.join("root.erofs");
    let cas_dir = /* objects dir from db_path */;

    // Mount new generation's composefs
    let staging = PathBuf::from("/conary/mnt-new");
    std::fs::create_dir_all(&staging)?;

    Command::new("mount")
        .args(["-t", "composefs"])
        .arg(&erofs_img)
        .arg(&staging)
        .arg("-o").arg(format!("basedir={},verity_check=1", cas_dir.display()))
        .status()?;

    // Bind-mount /usr (read-only)
    Command::new("mount").args(["--bind"])
        .arg(staging.join("usr")).arg("/usr").status()?;
    Command::new("mount").args(["-o", "remount,ro", "/usr"]).status()?;

    // Rebuild /etc overlay
    Command::new("umount").arg("/etc").status().ok(); // may fail if busy
    Command::new("mount")
        .args(["-t", "overlay", "overlay", "/etc"])
        .arg("-o").arg(format!(
            "lowerdir={},upperdir=/conary/etc-state/upper,workdir=/conary/etc-state/work",
            staging.join("etc").display()
        ))
        .status()?;

    update_current_symlink(gen_number)?;
    println!("Switched to generation {gen_number}. Reboot recommended for full consistency.");
    Ok(())
}
```

**Remove:** `renameat2_exchange()`, `fallback_rename()`, `RENAME_EXCHANGE` constant, `SWAP_DIRS` constant.

**Tests:**
- switch_live errors clearly when composefs mount fails
- current_generation() still works (reads symlink)

**Commit:** `feat(generation): Replace renameat2 with composefs mount switching`

---

## Task 19: Update GC for CAS-Aware Cleanup

**Files:**
- Modify: `src/commands/generation/commands.rs`

Update GC to handle EROFS images and add CAS reference scanning.

**Changes to cmd_generation_gc:**
- Generation size is now `root.erofs` file size (not tree walk)
- Add optional `--gc-cas` flag to also clean unreferenced CAS objects

**CAS reference scanning:**
```rust
/// Scan all remaining generations' EROFS images to find referenced CAS digests.
fn collect_cas_references() -> Result<HashSet<String>> {
    let mut refs = HashSet::new();
    for gen_dir in list_generations()? {
        let image = gen_dir.join("root.erofs");
        let info = conary_erofs::verify::verify_image(File::open(image)?)?;
        for file in &info.files {
            if let Some(digest) = &file.digest {
                refs.insert(hex::encode(digest));
            }
        }
    }
    Ok(refs)
}
```

**Commit:** `feat(generation): Update GC for EROFS images and CAS references`

---

## Task 20: Remove Reflink Code

**Files:**
- Delete: `src/filesystem/reflink.rs`
- Modify: `src/filesystem/mod.rs` — remove `pub mod reflink`
- Modify: `src/filesystem/deployer.rs` — remove `deploy_file_reflink` method

**Verify:** `cargo build` and `cargo test` pass without reflink module.

**Commit:** `refactor: Remove reflink code (replaced by composefs)`

---

## Task 21: Update Integration Tests

**Files:**
- Modify: `tests/integration/remi/runner/test-runner.sh`

Update T33-T35 generation tests for composefs format:

- T33: `conary generation build` produces `root.erofs` (not file tree)
- T34: `conary generation list` shows composefs generations
- T35: `conary generation gc` removes EROFS images

Add new tests:
- T36: `conary generation info N` shows EROFS image size and CAS refs
- T37: `conary system takeover --dry-run` reports composefs format

**Commit:** `test: Update integration tests for composefs generations`

---

## File Summary

| File | Action |
|------|--------|
| `Cargo.toml` | Add workspace config, conary-erofs dependency |
| `conary-erofs/Cargo.toml` | **NEW** — workspace crate |
| `conary-erofs/src/lib.rs` | **NEW** — crate root |
| `conary-erofs/src/superblock.rs` | **NEW** — EROFS superblock |
| `conary-erofs/src/inode.rs` | **NEW** — compact/extended inodes |
| `conary-erofs/src/dirent.rs` | **NEW** — directory entries |
| `conary-erofs/src/chunk.rs` | **NEW** — chunk-based external refs |
| `conary-erofs/src/xattr.rs` | **NEW** — xattr support |
| `conary-erofs/src/compress.rs` | **NEW** — LZ4/LZMA compression |
| `conary-erofs/src/tail_pack.rs` | **NEW** — tail-end packing |
| `conary-erofs/src/builder.rs` | **NEW** — high-level builder API |
| `conary-erofs/src/verify.rs` | **NEW** — image verification |
| `conary-erofs/tests/mount_test.rs` | **NEW** — composefs mount test |
| `src/commands/generation/composefs.rs` | **NEW** — composefs detection |
| `src/filesystem/fsverity.rs` | **NEW** — fs-verity enablement |
| `src/commands/generation/builder.rs` | Rewrite for EROFS |
| `src/commands/generation/metadata.rs` | Add composefs fields |
| `src/commands/generation/switch.rs` | Mount-based switching |
| `src/commands/generation/commands.rs` | GC for EROFS + CAS refs |
| `packaging/dracut/90conary/conary-generator.sh` | composefs mounts |
| `packaging/dracut/90conary/module-setup.sh` | Include composefs tools |
| `src/filesystem/reflink.rs` | **DELETE** |
| `src/filesystem/deployer.rs` | Remove deploy_file_reflink |
| `src/filesystem/mod.rs` | Remove reflink module |
| `tests/integration/remi/runner/test-runner.sh` | Update T33-T37 |

---

## Verification

1. `cargo build` — both crates compile clean
2. `cargo test -p conary-erofs` — all EROFS unit tests pass
3. `cargo test --bin conary` — all existing tests pass
4. `cargo clippy -- -D warnings` — no warnings
5. **Manual test on Fedora 43 (kernel 6.x):**
   - `conary system takeover --dry-run` — shows composefs plan
   - `conary generation build` — produces root.erofs
   - `conary generation list` — shows composefs generation
   - `conary generation info 1` — shows EROFS image size
   - `mount -t composefs` on the image — files are readable
   - `conary generation gc` — cleans old generations
