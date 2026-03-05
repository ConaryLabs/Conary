// conary-erofs/src/builder.rs
//! High-level EROFS image builder.
//!
//! Orchestrates the superblock, inode, dirent, chunk, xattr, and tail-packing
//! modules into a valid EROFS image suitable for composefs use. Files are
//! referenced externally via CAS digest xattrs; no file content is stored
//! in the image itself.

use std::collections::BTreeMap;
use std::io::{self, Seek, SeekFrom, Write};

use crate::chunk::build_chunk_indexes;
use crate::compress::Compression;
use crate::dirent::{pack_directory, DirEntry, EROFS_FT_DIR, EROFS_FT_REG_FILE, EROFS_FT_SYMLINK};
use crate::inode::{
    nid_from_offset, InodeInfo, EROFS_COMPACT_INODE_SIZE, EROFS_EXTENDED_INODE_SIZE,
    EROFS_INODE_CHUNK_BASED, EROFS_INODE_FLAT_INLINE, EROFS_INODE_FLAT_PLAIN, S_IFDIR, S_IFLNK,
    S_IFREG,
};
use crate::superblock::{
    Superblock, EROFS_FEATURE_COMPAT_SB_CHKSUM, EROFS_FEATURE_INCOMPAT_CHUNKED_FILE,
    EROFS_FEATURE_INCOMPAT_DEVICE_TABLE,
};
use crate::tail_pack::pack_tail;
use crate::xattr::build_composefs_xattrs;

/// EROFS image builder that assembles filesystem entries into a valid image.
///
/// All regular files use chunk-based external references (composefs mode).
/// Symlink targets are tail-packed inline. Directories are packed into
/// block-aligned dirent blocks.
pub struct ErofsBuilder {
    block_size: u32,
    compression: Compression,
    entries: Vec<FsEntry>,
}

/// A filesystem entry to include in the image.
#[derive(Debug, Clone)]
enum FsEntry {
    File {
        path: String,
        digest: [u8; 32],
        size: u64,
        mode: u32,
        uid: u32,
        gid: u32,
    },
    Symlink {
        path: String,
        target: String,
        mode: u32,
    },
    Directory {
        path: String,
        mode: u32,
        uid: u32,
        gid: u32,
    },
}

/// Statistics from a completed build.
pub struct BuildStats {
    pub image_size: u64,
    pub inode_count: u64,
    pub file_count: u64,
    pub dir_count: u64,
    pub symlink_count: u64,
    pub tail_packed_files: u64,
}

// ---------------------------------------------------------------------------
// Internal tree node used during layout
// ---------------------------------------------------------------------------

/// A node in the filesystem tree, built from flat path entries.
#[derive(Debug)]
struct TreeNode {
    name: String,
    kind: NodeKind,
    children: Vec<usize>, // indices into the node arena
}

#[derive(Debug)]
enum NodeKind {
    Directory { mode: u32, uid: u32, gid: u32 },
    File { digest: [u8; 32], size: u64, mode: u32, uid: u32, gid: u32 },
    Symlink { target: String, mode: u32 },
}

// ---------------------------------------------------------------------------
// Hex encoding helper
// ---------------------------------------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn cas_path_from_digest(digest: &[u8; 32]) -> String {
    let hex = hex_encode(digest);
    format!("{}/{}", &hex[..2], &hex[2..])
}

/// Round `offset` up to the next multiple of `align`.
fn align_up(offset: u64, align: u64) -> u64 {
    (offset + align - 1) & !(align - 1)
}

// ---------------------------------------------------------------------------
// Builder implementation
// ---------------------------------------------------------------------------

impl ErofsBuilder {
    /// Create a new builder with default settings (4096-byte blocks, no compression).
    #[must_use]
    pub fn new() -> Self {
        Self {
            block_size: 4096,
            compression: Compression::None,
            entries: Vec::new(),
        }
    }

    /// Set the block size (must be a power of two >= 512).
    #[must_use]
    pub fn block_size(mut self, size: u32) -> Self {
        self.block_size = size;
        self
    }

    /// Set the compression algorithm for metadata.
    #[must_use]
    pub fn compression(mut self, comp: Compression) -> Self {
        self.compression = comp;
        self
    }

    /// Add a regular file entry with a CAS digest for external data.
    pub fn add_file(
        &mut self,
        path: &str,
        digest: &[u8; 32],
        size: u64,
        mode: u32,
        uid: u32,
        gid: u32,
    ) {
        self.entries.push(FsEntry::File {
            path: normalize_path(path),
            digest: *digest,
            size,
            mode,
            uid,
            gid,
        });
    }

    /// Add a symbolic link entry.
    pub fn add_symlink(&mut self, path: &str, target: &str, mode: u32) {
        self.entries.push(FsEntry::Symlink {
            path: normalize_path(path),
            target: target.to_string(),
            mode,
        });
    }

    /// Add a directory entry.
    pub fn add_directory(&mut self, path: &str, mode: u32, uid: u32, gid: u32) {
        self.entries.push(FsEntry::Directory {
            path: normalize_path(path),
            mode,
            uid,
            gid,
        });
    }

    /// Build the EROFS image, writing to the provided writer.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the writer fails or if the image layout is
    /// internally inconsistent.
    pub fn build<W: Write + Seek>(&self, mut writer: W) -> io::Result<BuildStats> {
        let bs = self.block_size;
        let bs64 = u64::from(bs);
        let meta_blkaddr: u32 = 1; // inodes start at block 1

        // 1. Build tree from flat entries
        let (arena, root_idx) = self.build_tree();

        // 2. Collect traversal order (BFS, root first)
        let order = bfs_order(&arena, root_idx);
        let inode_count = order.len() as u64;

        // 3. Phase 1: compute inode sizes and assign offsets
        //    We need two passes: first compute sizes, then after we know
        //    where the directory data area starts, fill in directory union_values.

        // 3a. Compute per-node inode+xattr+tail sizes (without dir block addresses)
        let chunk_bits = self.block_size.trailing_zeros() as u8;
        let mut node_sizes: Vec<(usize, u64)> = Vec::with_capacity(order.len()); // (arena_idx, total_inode_bytes)
        let mut file_chunk_data: BTreeMap<usize, (Vec<u8>, u16)> = BTreeMap::new(); // arena_idx -> (chunk_bytes, chunk_format)
        let mut file_xattr_data: BTreeMap<usize, (Vec<u8>, u16)> = BTreeMap::new(); // arena_idx -> (xattr_bytes, xattr_icount)
        let mut symlink_targets: BTreeMap<usize, String> = BTreeMap::new();

        for &idx in &order {
            let node = &arena[idx];
            match &node.kind {
                NodeKind::Directory { .. } => {
                    // Compact inode, no xattrs, no inline data
                    let inode_sz = EROFS_COMPACT_INODE_SIZE as u64;
                    node_sizes.push((idx, inode_sz));
                }
                NodeKind::File { digest, size, uid, gid, .. } => {
                    let cas_path = cas_path_from_digest(digest);
                    let (xattr_bytes, xattr_icount) = build_composefs_xattrs(&cas_path);
                    let (chunk_bytes, chunk_format) = build_chunk_indexes(*size, chunk_bits);

                    let inode = build_file_inode_info(*size, *uid, *gid, chunk_format);
                    let inode_sz = if inode.needs_extended() {
                        EROFS_EXTENDED_INODE_SIZE as u64
                    } else {
                        EROFS_COMPACT_INODE_SIZE as u64
                    };

                    let total = inode_sz + xattr_bytes.len() as u64;
                    file_xattr_data.insert(idx, (xattr_bytes, xattr_icount));
                    file_chunk_data.insert(idx, (chunk_bytes, chunk_format));
                    node_sizes.push((idx, total));
                }
                NodeKind::Symlink { target, .. } => {
                    // Inline the target string
                    let inode_sz = EROFS_COMPACT_INODE_SIZE as u64;
                    let total = inode_sz + target.len() as u64;
                    symlink_targets.insert(idx, target.clone());
                    node_sizes.push((idx, total));
                }
            }
        }

        // 3b. Assign byte offsets to inodes (starting at meta_blkaddr * block_size)
        let meta_start = u64::from(meta_blkaddr) * bs64;
        let mut inode_offsets: BTreeMap<usize, u64> = BTreeMap::new();
        let mut cursor = meta_start;

        for &(idx, size) in &node_sizes {
            // Each inode must start on a 32-byte boundary so NID addressing
            // works correctly: NID = (byte_offset - meta_start) / 32.
            cursor = align_up(cursor, 32);
            inode_offsets.insert(idx, cursor);
            cursor += size;
        }

        // Align to block boundary after inode table
        let dir_data_start = align_up(cursor, bs64);

        // 3c. Build directory data blocks now that we know all NIDs
        //     We also need to know where each directory's dirent blocks go.
        let mut dir_blocks_map: BTreeMap<usize, Vec<Vec<u8>>> = BTreeMap::new();
        let mut dir_data_cursor = dir_data_start;
        let mut dir_block_addrs: BTreeMap<usize, u32> = BTreeMap::new();

        for &idx in &order {
            let node = &arena[idx];
            if !matches!(node.kind, NodeKind::Directory { .. }) {
                continue;
            }
            if arena[idx].children.is_empty() {
                // Empty directory: no dirent blocks, size=0, union_value=0
                dir_blocks_map.insert(idx, Vec::new());
                dir_block_addrs.insert(idx, 0);
                continue;
            }

            let mut dir_entries: Vec<DirEntry> = Vec::new();
            for &child_idx in &arena[idx].children {
                let child = &arena[child_idx];
                let child_offset = inode_offsets[&child_idx];
                let child_nid = nid_from_offset(child_offset, meta_blkaddr, bs);
                let file_type = match &child.kind {
                    NodeKind::Directory { .. } => EROFS_FT_DIR,
                    NodeKind::File { .. } => EROFS_FT_REG_FILE,
                    NodeKind::Symlink { .. } => EROFS_FT_SYMLINK,
                };
                dir_entries.push(DirEntry {
                    name: child.name.clone(),
                    nid: child_nid,
                    file_type,
                });
            }

            let blocks = pack_directory(&mut dir_entries, bs);
            #[allow(clippy::cast_possible_truncation)]
            let blk_addr = (dir_data_cursor / bs64) as u32;
            dir_block_addrs.insert(idx, blk_addr);
            dir_data_cursor += blocks.len() as u64 * bs64;
            dir_blocks_map.insert(idx, blocks);
        }

        // After directory data: chunk index area
        let chunk_data_start = dir_data_cursor;
        let mut chunk_offsets: BTreeMap<usize, u64> = BTreeMap::new();
        let mut chunk_cursor = chunk_data_start;

        for &idx in &order {
            if let Some((chunk_bytes, _)) = file_chunk_data.get(&idx)
                && !chunk_bytes.is_empty()
            {
                chunk_offsets.insert(idx, chunk_cursor);
                chunk_cursor += chunk_bytes.len() as u64;
            }
        }

        // Total image size (block-aligned)
        let total_size = align_up(chunk_cursor, bs64);
        #[allow(clippy::cast_possible_truncation)]
        let total_blocks = (total_size / bs64) as u32;

        // 4. Phase 2: serialize and write everything

        // 4a. Write superblock placeholder (1024 + 128 bytes of zeros)
        writer.write_all(&[0u8; 1024 + 128])?;

        // Pad to block boundary (fill rest of block 0)
        let sb_end = 1024 + 128;
        let pad_to_meta = meta_start as usize - sb_end;
        if pad_to_meta > 0 {
            writer.write_all(&vec![0u8; pad_to_meta])?;
        }

        // 4b. Write inode table
        let mut stats = BuildStats {
            image_size: 0,
            inode_count,
            file_count: 0,
            dir_count: 0,
            symlink_count: 0,
            tail_packed_files: 0,
        };

        let mut written = meta_start;
        for &(idx, _size) in &node_sizes {
            // Align to 32-byte boundary (matching offset assignment in phase 3b).
            let aligned = align_up(written, 32);
            let pad = aligned - written;
            if pad > 0 {
                writer.write_all(&vec![0u8; pad as usize])?;
                written += pad;
            }

            let node = &arena[idx];
            match &node.kind {
                NodeKind::Directory { mode, uid, gid } => {
                    stats.dir_count += 1;
                    let dir_blocks = &dir_blocks_map[&idx];
                    #[allow(clippy::cast_possible_truncation)]
                    let dir_size = (dir_blocks.len() as u64) * bs64;
                    let blk_addr = dir_block_addrs[&idx];

                    let inode = InodeInfo {
                        mode: S_IFDIR | (*mode as u16 & 0o7777),
                        uid: *uid,
                        gid: *gid,
                        size: dir_size,
                        nlink: 2, // . and parent
                        mtime: 0,
                        mtime_nsec: 0,
                        ino: idx as u32,
                        xattr_icount: 0,
                        data_layout: EROFS_INODE_FLAT_PLAIN,
                        union_value: blk_addr,
                    };
                    inode.write_compact(&mut writer)?;
                    written += EROFS_COMPACT_INODE_SIZE as u64;
                }
                NodeKind::File { digest: _, size, mode, uid, gid, .. } => {
                    stats.file_count += 1;
                    let (ref xattr_bytes, xattr_icount) = file_xattr_data[&idx];
                    let (_, chunk_format) = file_chunk_data[&idx];

                    let inode = InodeInfo {
                        mode: S_IFREG | (*mode as u16 & 0o7777),
                        uid: *uid,
                        gid: *gid,
                        size: *size,
                        nlink: 1,
                        mtime: 0,
                        mtime_nsec: 0,
                        ino: idx as u32,
                        xattr_icount,
                        data_layout: EROFS_INODE_CHUNK_BASED,
                        union_value: u32::from(chunk_format),
                    };

                    if inode.needs_extended() {
                        inode.write_extended(&mut writer)?;
                        written += EROFS_EXTENDED_INODE_SIZE as u64;
                    } else {
                        inode.write_compact(&mut writer)?;
                        written += EROFS_COMPACT_INODE_SIZE as u64;
                    }
                    writer.write_all(xattr_bytes)?;
                    written += xattr_bytes.len() as u64;
                }
                NodeKind::Symlink { target, mode } => {
                    stats.symlink_count += 1;
                    stats.tail_packed_files += 1;

                    let inode = InodeInfo {
                        mode: S_IFLNK | (*mode as u16 & 0o7777),
                        uid: 0,
                        gid: 0,
                        size: target.len() as u64,
                        nlink: 1,
                        mtime: 0,
                        mtime_nsec: 0,
                        ino: idx as u32,
                        xattr_icount: 0,
                        data_layout: EROFS_INODE_FLAT_INLINE,
                        union_value: 0,
                    };

                    // Use pack_tail to combine inode + target
                    let mut inode_buf = Vec::new();
                    inode.write_compact(&mut inode_buf)?;
                    let packed = pack_tail(&inode_buf, &[], target.as_bytes());
                    writer.write_all(&packed)?;
                    written += packed.len() as u64;
                }
            }
        }

        // Pad inode table to block boundary
        let pad = align_up(written, bs64) - written;
        if pad > 0 {
            writer.write_all(&vec![0u8; pad as usize])?;
            written += pad;
        }

        // 4c. Write directory data blocks
        for &idx in &order {
            if let Some(blocks) = dir_blocks_map.get(&idx) {
                for blk in blocks {
                    writer.write_all(blk)?;
                    written += bs64;
                }
            }
        }

        // 4d. Write chunk index data
        for &idx in &order {
            if let Some((chunk_bytes, _)) = file_chunk_data.get(&idx)
                && !chunk_bytes.is_empty()
            {
                writer.write_all(chunk_bytes)?;
                written += chunk_bytes.len() as u64;
            }
        }

        // Pad to final block boundary
        let final_pad = align_up(written, bs64) - written;
        if final_pad > 0 {
            writer.write_all(&vec![0u8; final_pad as usize])?;
            written += final_pad;
        }

        // 4e. Go back and write the real superblock
        let root_offset = inode_offsets[&root_idx];
        let root_nid = nid_from_offset(root_offset, meta_blkaddr, bs);

        let mut sb = Superblock::new(bs);
        sb.feature_compat = EROFS_FEATURE_COMPAT_SB_CHKSUM;
        sb.feature_incompat =
            EROFS_FEATURE_INCOMPAT_CHUNKED_FILE | EROFS_FEATURE_INCOMPAT_DEVICE_TABLE;
        #[allow(clippy::cast_possible_truncation)]
        {
            sb.root_nid = root_nid as u16;
        }
        sb.inos = inode_count;
        sb.blocks = total_blocks;
        sb.meta_blkaddr = meta_blkaddr;

        sb.checksum = sb.compute_checksum();

        writer.seek(SeekFrom::Start(0))?;
        sb.write_to(&mut writer)?;

        // Seek back to end
        writer.seek(SeekFrom::Start(written))?;

        stats.image_size = written;

        Ok(stats)
    }

    // -----------------------------------------------------------------------
    // Tree building
    // -----------------------------------------------------------------------

    /// Build a tree from flat entries. Returns (arena, root_index).
    fn build_tree(&self) -> (Vec<TreeNode>, usize) {
        let mut arena: Vec<TreeNode> = Vec::new();

        // Root node is always index 0
        arena.push(TreeNode {
            name: String::new(),
            kind: NodeKind::Directory {
                mode: 0o755,
                uid: 0,
                gid: 0,
            },
            children: Vec::new(),
        });

        // Collect all entries, sorted for determinism
        let mut sorted_entries = self.entries.clone();
        sorted_entries.sort_by(|a, b| entry_path(a).cmp(entry_path(b)));

        // Track which paths already have directory nodes: path -> arena index
        let mut dir_map: BTreeMap<String, usize> = BTreeMap::new();
        dir_map.insert(String::new(), 0);

        for entry in &sorted_entries {
            let path = entry_path(entry);
            let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            if components.is_empty() {
                // Root directory entry: update root node's attributes
                if let FsEntry::Directory { mode, uid, gid, .. } = entry {
                    arena[0].kind = NodeKind::Directory {
                        mode: *mode,
                        uid: *uid,
                        gid: *gid,
                    };
                }
                continue;
            }

            // Ensure all parent directories exist (implicit directory creation)
            let mut parent_idx = 0;
            for i in 0..components.len() - 1 {
                let partial: String = components[..=i].join("/");
                if let Some(&idx) = dir_map.get(&partial) {
                    parent_idx = idx;
                } else {
                    let new_idx = arena.len();
                    arena.push(TreeNode {
                        name: components[i].to_string(),
                        kind: NodeKind::Directory {
                            mode: 0o755,
                            uid: 0,
                            gid: 0,
                        },
                        children: Vec::new(),
                    });
                    arena[parent_idx].children.push(new_idx);
                    dir_map.insert(partial, new_idx);
                    parent_idx = new_idx;
                }
            }

            let leaf_name = components.last().unwrap().to_string();
            let full_path: String = components.join("/");

            // Check if this is a directory that might already exist as implicit
            if let FsEntry::Directory { mode, uid, gid, .. } = entry
                && let Some(&existing_idx) = dir_map.get(&full_path)
            {
                // Update the existing implicit directory with explicit attrs
                arena[existing_idx].kind = NodeKind::Directory {
                    mode: *mode,
                    uid: *uid,
                    gid: *gid,
                };
                continue;
            }

            let new_idx = arena.len();
            let kind = match entry {
                FsEntry::File {
                    digest,
                    size,
                    mode,
                    uid,
                    gid,
                    ..
                } => NodeKind::File {
                    digest: *digest,
                    size: *size,
                    mode: *mode,
                    uid: *uid,
                    gid: *gid,
                },
                FsEntry::Symlink { target, mode, .. } => NodeKind::Symlink {
                    target: target.clone(),
                    mode: *mode,
                },
                FsEntry::Directory { mode, uid, gid, .. } => NodeKind::Directory {
                    mode: *mode,
                    uid: *uid,
                    gid: *gid,
                },
            };

            arena.push(TreeNode {
                name: leaf_name,
                kind,
                children: Vec::new(),
            });
            arena[parent_idx].children.push(new_idx);

            if let FsEntry::Directory { .. } = entry {
                dir_map.insert(full_path, new_idx);
            }
        }

        // Sort children of each directory by name for deterministic output.
        for i in 0..arena.len() {
            let mut children = std::mem::take(&mut arena[i].children);
            children.sort_by(|&a, &b| arena[a].name.cmp(&arena[b].name));
            arena[i].children = children;
        }

        (arena, 0)
    }
}

impl Default for ErofsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

fn entry_path(entry: &FsEntry) -> &str {
    match entry {
        FsEntry::File { path, .. }
        | FsEntry::Symlink { path, .. }
        | FsEntry::Directory { path, .. } => path,
    }
}

/// Normalize a path: strip leading `/`, ensure no trailing `/`.
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
    trimmed.to_string()
}

/// BFS traversal order starting from root.
fn bfs_order(arena: &[TreeNode], root: usize) -> Vec<usize> {
    let mut order = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(root);
    while let Some(idx) = queue.pop_front() {
        order.push(idx);
        for &child in &arena[idx].children {
            queue.push_back(child);
        }
    }
    order
}

/// Build an `InodeInfo` for a regular file in chunk-based mode.
#[allow(clippy::cast_possible_truncation)]
fn build_file_inode_info(size: u64, uid: u32, gid: u32, chunk_format: u16) -> InodeInfo {
    InodeInfo {
        mode: S_IFREG | 0o644,
        uid,
        gid,
        size,
        nlink: 1,
        mtime: 0,
        mtime_nsec: 0,
        ino: 0,
        xattr_icount: 0,
        data_layout: EROFS_INODE_CHUNK_BASED,
        union_value: u32::from(chunk_format),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::superblock::{Superblock, EROFS_SUPER_MAGIC, EROFS_SUPER_OFFSET};
    use std::io::Cursor;

    /// Parse a superblock from the image bytes.
    fn parse_superblock(image: &[u8]) -> Superblock {
        let sb_start = EROFS_SUPER_OFFSET as usize;
        let sb_bytes: [u8; 128] = image[sb_start..sb_start + 128].try_into().unwrap();
        Superblock::from_bytes(&sb_bytes).expect("valid superblock")
    }

    #[test]
    fn empty_image_has_valid_superblock() {
        let builder = ErofsBuilder::new();
        let mut buf = Cursor::new(Vec::new());
        let stats = builder.build(&mut buf).unwrap();

        let image = buf.into_inner();
        let sb = parse_superblock(&image);

        assert_eq!(sb.magic, EROFS_SUPER_MAGIC);
        assert_eq!(sb.inos, 1, "empty image should have 1 inode (root dir)");
        assert_eq!(stats.dir_count, 1);
        assert_eq!(stats.file_count, 0);
        assert_eq!(stats.inode_count, 1);
    }

    #[test]
    fn single_file_image() {
        let mut builder = ErofsBuilder::new();
        let digest = [0xAB; 32];
        builder.add_file("/hello.txt", &digest, 1024, 0o644, 1000, 1000);

        let mut buf = Cursor::new(Vec::new());
        let stats = builder.build(&mut buf).unwrap();

        assert_eq!(stats.inode_count, 2, "root dir + 1 file");
        assert_eq!(stats.file_count, 1);
        assert_eq!(stats.dir_count, 1);

        let image = buf.into_inner();
        let sb = parse_superblock(&image);
        assert_eq!(sb.inos, 2);
        assert_ne!(sb.checksum, 0, "checksum should be set");
    }

    #[test]
    fn directory_hierarchy() {
        let mut builder = ErofsBuilder::new();
        let d1 = [0x01; 32];
        let d2 = [0x02; 32];
        builder.add_file("/usr/bin/foo", &d1, 4096, 0o755, 0, 0);
        builder.add_file("/usr/lib/bar", &d2, 8192, 0o644, 0, 0);

        let mut buf = Cursor::new(Vec::new());
        let stats = builder.build(&mut buf).unwrap();

        // root + /usr + /usr/bin + /usr/lib + foo + bar = 6
        assert_eq!(stats.inode_count, 6);
        assert_eq!(stats.dir_count, 4); // root, usr, bin, lib
        assert_eq!(stats.file_count, 2);

        let image = buf.into_inner();
        let sb = parse_superblock(&image);
        assert_eq!(sb.inos, 6);
    }

    #[test]
    fn symlinks_are_flat_inline() {
        let mut builder = ErofsBuilder::new();
        builder.add_symlink("/usr/bin/python", "python3", 0o777);

        let mut buf = Cursor::new(Vec::new());
        let stats = builder.build(&mut buf).unwrap();

        // root + /usr + /usr/bin + symlink = 4
        assert_eq!(stats.inode_count, 4);
        assert_eq!(stats.symlink_count, 1);
        assert_eq!(stats.tail_packed_files, 1);
    }

    #[test]
    fn large_file_count() {
        let mut builder = ErofsBuilder::new();
        for i in 0u32..100 {
            let mut digest = [0u8; 32];
            digest[0..4].copy_from_slice(&i.to_le_bytes());
            let path = format!("/files/file_{i:03}");
            builder.add_file(&path, &digest, 4096, 0o644, 0, 0);
        }

        let mut buf = Cursor::new(Vec::new());
        let stats = builder.build(&mut buf).unwrap();

        // root + /files + 100 files = 102
        assert_eq!(stats.inode_count, 102);
        assert_eq!(stats.file_count, 100);

        let image = buf.into_inner();
        let sb = parse_superblock(&image);
        assert_eq!(sb.inos, 102);
    }

    #[test]
    fn build_stats_reports_correct_counts() {
        let mut builder = ErofsBuilder::new();
        builder.add_directory("/etc", 0o755, 0, 0);
        builder.add_file("/etc/hosts", &[0x11; 32], 256, 0o644, 0, 0);
        builder.add_symlink("/etc/localtime", "/usr/share/zoneinfo/UTC", 0o777);

        let mut buf = Cursor::new(Vec::new());
        let stats = builder.build(&mut buf).unwrap();

        assert_eq!(stats.dir_count, 2); // root + /etc
        assert_eq!(stats.file_count, 1);
        assert_eq!(stats.symlink_count, 1);
        assert_eq!(stats.inode_count, 4);
        assert!(stats.image_size > 0);
    }

    #[test]
    fn deterministic_output() {
        let build_image = || {
            let mut builder = ErofsBuilder::new();
            builder.add_file("/a", &[0x01; 32], 100, 0o644, 0, 0);
            builder.add_file("/b", &[0x02; 32], 200, 0o644, 0, 0);
            builder.add_directory("/d", 0o755, 0, 0);
            builder.add_symlink("/d/link", "target", 0o777);

            let mut buf = Cursor::new(Vec::new());
            builder.build(&mut buf).unwrap();
            buf.into_inner()
        };

        let img1 = build_image();
        let img2 = build_image();
        assert_eq!(img1, img2, "same input must produce byte-identical output");
    }

    #[test]
    fn implicit_parent_directories() {
        let mut builder = ErofsBuilder::new();
        // Never explicitly add /usr or /usr/bin
        builder.add_file("/usr/bin/nginx", &[0xAA; 32], 2048, 0o755, 0, 0);

        let mut buf = Cursor::new(Vec::new());
        let stats = builder.build(&mut buf).unwrap();

        // root + /usr (implicit) + /usr/bin (implicit) + nginx = 4
        assert_eq!(stats.inode_count, 4);
        assert_eq!(stats.dir_count, 3);
        assert_eq!(stats.file_count, 1);
    }

    #[test]
    fn image_is_block_aligned() {
        let mut builder = ErofsBuilder::new();
        builder.add_file("/test", &[0xFF; 32], 512, 0o644, 0, 0);

        let mut buf = Cursor::new(Vec::new());
        let stats = builder.build(&mut buf).unwrap();

        assert_eq!(
            stats.image_size % 4096,
            0,
            "image size must be a multiple of block size"
        );
    }

    #[test]
    fn root_nid_is_zero() {
        let mut builder = ErofsBuilder::new();
        builder.add_file("/x", &[0x42; 32], 64, 0o644, 0, 0);

        let mut buf = Cursor::new(Vec::new());
        builder.build(&mut buf).unwrap();

        let image = buf.into_inner();
        let sb = parse_superblock(&image);
        assert_eq!(sb.root_nid, 0, "root directory must be the first inode (NID 0)");
    }

    #[test]
    fn hex_encode_works() {
        assert_eq!(hex_encode(&[0xab, 0xcd, 0x01, 0xff]), "abcd01ff");
    }

    #[test]
    fn cas_path_format() {
        let digest = [0xAB; 32];
        let path = cas_path_from_digest(&digest);
        assert!(path.starts_with("ab/"));
        assert_eq!(path.len(), 2 + 1 + 62); // 2 prefix + / + 62 rest
    }

    #[test]
    fn chunked_file_feature_flag_set() {
        let builder = ErofsBuilder::new();
        let mut buf = Cursor::new(Vec::new());
        builder.build(&mut buf).unwrap();

        let image = buf.into_inner();
        let sb = parse_superblock(&image);
        assert_ne!(
            sb.feature_incompat & EROFS_FEATURE_INCOMPAT_CHUNKED_FILE,
            0,
            "CHUNKED_FILE feature flag must be set"
        );
    }
}
