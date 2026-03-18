---
paths:
  - "conary-erofs/**"
---

# EROFS Crate (conary-erofs)

Separate crate that builds valid EROFS filesystem images for use with Linux
composefs. Regular files reference content externally via CAS digest xattrs --
no file content is stored in the image itself.

## Key Types
- `ErofsBuilder` -- high-level image builder, assembles `FsEntry` into valid image
- `FsEntry` -- `File { digest, size, mode, uid, gid }`, `Symlink { target }`, `Directory`
- `BuildStats` -- image_size, inode/file/dir/symlink counts, tail_packed_files
- `InodeInfo` -- inode builder: mode, uid, gid, size, nlink, mtime, data_layout
- `Superblock` -- EROFS superblock with feature flags
- `DirEntry` -- directory entry with file type constants

## Constants
- `EROFS_COMPACT_INODE_SIZE` -- 32 bytes
- `EROFS_EXTENDED_INODE_SIZE` -- 64 bytes
- `EROFS_INODE_FLAT_PLAIN` (0), `EROFS_INODE_FLAT_INLINE` (2), `EROFS_INODE_CHUNK_BASED` (4)
- `S_IFREG` (0o100000), `S_IFDIR` (0o040000), `S_IFLNK` (0o120000)
- `EROFS_FT_REG_FILE`, `EROFS_FT_DIR`, `EROFS_FT_SYMLINK` -- dirent file type tags
- Feature flags: `EROFS_FEATURE_COMPAT_SB_CHKSUM`, `EROFS_FEATURE_INCOMPAT_CHUNKED_FILE`, `EROFS_FEATURE_INCOMPAT_DEVICE_TABLE`

## Invariants
- All regular files use chunk-based external references (composefs mode)
- Symlink targets are tail-packed inline after the inode
- Directories are packed into block-aligned dirent blocks
- `i_format = (data_layout << 1) | version` where version 0 = compact, 1 = extended
- `nid_from_offset()` computes NID: `(byte_offset - meta_blkaddr * block_size) / 32`

## Gotchas
- This is a standalone crate -- does not depend on conary-core
- Generation building now primarily uses composefs-rs (v0.3.0) via
  `conary-core/src/generation/builder.rs` instead of conary-erofs directly.
  The conary-erofs crate still exists for low-level EROFS construction but is
  not the primary builder for generations.
- No file content is stored in the image -- only metadata and CAS digest xattrs
- `build_composefs_xattrs()` creates the xattr entries for CAS references
- `tail_pack::pack_tail()` handles inline data packing for small files/symlinks
- Extended inodes (64 bytes) required when compact format cannot represent the data
- `verify` module validates produced images

## Files
- `builder.rs` -- `ErofsBuilder`, `FsEntry`, `BuildStats`, tree layout
- `inode.rs` -- `InodeInfo`, format constants, `nid_from_offset()`
- `superblock.rs` -- `Superblock`, feature flags
- `dirent.rs` -- `DirEntry`, `pack_directory()`, file type constants
- `xattr.rs` -- `build_composefs_xattrs()` for CAS digest references
- `chunk.rs` -- chunk index handling
- `compress.rs` -- LZ4/LZMA compression support
- `tail_pack.rs` -- `pack_tail()` for inline data
- `verify.rs` -- image validation
