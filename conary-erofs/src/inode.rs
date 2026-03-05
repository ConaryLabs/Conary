// conary-erofs/src/inode.rs
//! EROFS inode structures and serialization.
//!
//! Supports both compact (32-byte) and extended (64-byte) on-disk inode
//! layouts. The format is determined by the version bit in `i_format`:
//! - Bit 0: version (0 = compact, 1 = extended)
//! - Bits 1-3: data layout
//!
//! So `i_format = (data_layout << 1) | version`.

use std::io::{self, Write};

// --- Data layout constants ---

/// Flat plain data layout: file data stored in consecutive blocks.
pub const EROFS_INODE_FLAT_PLAIN: u16 = 0;

/// Flat inline data layout: tail data packed after the inode (tail packing).
pub const EROFS_INODE_FLAT_INLINE: u16 = 2;

/// Chunk-based data layout: external blob references (composefs).
pub const EROFS_INODE_CHUNK_BASED: u16 = 4;

// --- File type constants (upper bits of i_mode) ---

/// Regular file.
pub const S_IFREG: u16 = 0o100000;

/// Directory.
pub const S_IFDIR: u16 = 0o040000;

/// Symbolic link.
pub const S_IFLNK: u16 = 0o120000;

// --- Inode sizes ---

/// On-disk size of a compact inode.
pub const EROFS_COMPACT_INODE_SIZE: usize = 32;

/// On-disk size of an extended inode.
pub const EROFS_EXTENDED_INODE_SIZE: usize = 64;

/// Compute the inode NID from its byte offset within the metadata area.
///
/// NID = (byte_offset - meta_blkaddr * block_size) / 32
#[must_use]
pub fn nid_from_offset(byte_offset: u64, meta_blkaddr: u32, block_size: u32) -> u64 {
    let meta_start = u64::from(meta_blkaddr) * u64::from(block_size);
    (byte_offset - meta_start) / 32
}

/// High-level inode builder that can produce either compact or extended format.
#[derive(Debug, Clone)]
pub struct InodeInfo {
    /// File mode (type + permission bits).
    pub mode: u16,
    /// Owner user ID.
    pub uid: u32,
    /// Owner group ID.
    pub gid: u32,
    /// File size in bytes.
    pub size: u64,
    /// Number of hard links.
    pub nlink: u32,
    /// Modification time (seconds since epoch).
    pub mtime: u64,
    /// Nanosecond component of modification time.
    pub mtime_nsec: u32,
    /// Original inode number.
    pub ino: u32,
    /// Extended attribute inline count.
    pub xattr_icount: u16,
    /// Data layout type (one of `EROFS_INODE_FLAT_*` or `EROFS_INODE_CHUNK_BASED`).
    pub data_layout: u16,
    /// Union value: `raw_blkaddr`, `rdev`, or `chunk_format` depending on context.
    pub union_value: u32,
}

impl InodeInfo {
    /// Returns `true` if this inode requires the extended (64-byte) format.
    ///
    /// Extended format is needed when uid or gid exceed `u16::MAX`, or when
    /// the file size exceeds `u32::MAX`.
    #[must_use]
    pub fn needs_extended(&self) -> bool {
        self.uid > u32::from(u16::MAX)
            || self.gid > u32::from(u16::MAX)
            || self.size > u64::from(u32::MAX)
    }

    /// Encode `i_format` for a compact inode (version = 0).
    #[must_use]
    fn compact_format(&self) -> u16 {
        self.data_layout << 1
    }

    /// Encode `i_format` for an extended inode (version = 1).
    #[must_use]
    fn extended_format(&self) -> u16 {
        (self.data_layout << 1) | 1
    }

    /// Serialize as a compact (32-byte) inode.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the underlying writer fails.
    pub fn write_compact<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let mut buf = [0u8; EROFS_COMPACT_INODE_SIZE];
        let mut off = 0;

        // i_format (u16)
        buf[off..off + 2].copy_from_slice(&self.compact_format().to_le_bytes());
        off += 2;
        // i_xattr_icount (u16)
        buf[off..off + 2].copy_from_slice(&self.xattr_icount.to_le_bytes());
        off += 2;
        // i_mode (u16)
        buf[off..off + 2].copy_from_slice(&self.mode.to_le_bytes());
        off += 2;
        // i_nlink (u16) — truncated from u32
        #[allow(clippy::cast_possible_truncation)]
        let nlink_u16 = self.nlink as u16;
        buf[off..off + 2].copy_from_slice(&nlink_u16.to_le_bytes());
        off += 2;
        // i_size (u32) — truncated from u64
        #[allow(clippy::cast_possible_truncation)]
        let size_u32 = self.size as u32;
        buf[off..off + 4].copy_from_slice(&size_u32.to_le_bytes());
        off += 4;
        // i_mtime (u32) — lower 32 bits of mtime
        #[allow(clippy::cast_possible_truncation)]
        let mtime_u32 = self.mtime as u32;
        buf[off..off + 4].copy_from_slice(&mtime_u32.to_le_bytes());
        off += 4;
        // i_u (u32)
        buf[off..off + 4].copy_from_slice(&self.union_value.to_le_bytes());
        off += 4;
        // i_ino (u32)
        buf[off..off + 4].copy_from_slice(&self.ino.to_le_bytes());
        off += 4;
        // i_uid (u16) — truncated from u32
        #[allow(clippy::cast_possible_truncation)]
        let uid_u16 = self.uid as u16;
        buf[off..off + 2].copy_from_slice(&uid_u16.to_le_bytes());
        off += 2;
        // i_gid (u16) — truncated from u32
        #[allow(clippy::cast_possible_truncation)]
        let gid_u16 = self.gid as u16;
        buf[off..off + 2].copy_from_slice(&gid_u16.to_le_bytes());
        off += 2;
        // i_reserved2 (u32) — zero
        off += 4;

        debug_assert_eq!(off, EROFS_COMPACT_INODE_SIZE);
        w.write_all(&buf)
    }

    /// Serialize as an extended (64-byte) inode.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the underlying writer fails.
    pub fn write_extended<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let mut buf = [0u8; EROFS_EXTENDED_INODE_SIZE];
        let mut off = 0;

        // i_format (u16)
        buf[off..off + 2].copy_from_slice(&self.extended_format().to_le_bytes());
        off += 2;
        // i_xattr_icount (u16)
        buf[off..off + 2].copy_from_slice(&self.xattr_icount.to_le_bytes());
        off += 2;
        // i_mode (u16)
        buf[off..off + 2].copy_from_slice(&self.mode.to_le_bytes());
        off += 2;
        // i_nb (u16) — nlink lower 16 bits (union erofs_inode_i_nb)
        #[allow(clippy::cast_possible_truncation)]
        let nb_u16 = self.nlink as u16;
        buf[off..off + 2].copy_from_slice(&nb_u16.to_le_bytes());
        off += 2;
        // i_size (u64)
        buf[off..off + 8].copy_from_slice(&self.size.to_le_bytes());
        off += 8;
        // i_u (u32)
        buf[off..off + 4].copy_from_slice(&self.union_value.to_le_bytes());
        off += 4;
        // i_ino (u32)
        buf[off..off + 4].copy_from_slice(&self.ino.to_le_bytes());
        off += 4;
        // i_uid (u32)
        buf[off..off + 4].copy_from_slice(&self.uid.to_le_bytes());
        off += 4;
        // i_gid (u32)
        buf[off..off + 4].copy_from_slice(&self.gid.to_le_bytes());
        off += 4;
        // i_mtime (u64)
        buf[off..off + 8].copy_from_slice(&self.mtime.to_le_bytes());
        off += 8;
        // i_mtime_nsec (u32)
        buf[off..off + 4].copy_from_slice(&self.mtime_nsec.to_le_bytes());
        off += 4;
        // i_nlink (u32)
        buf[off..off + 4].copy_from_slice(&self.nlink.to_le_bytes());
        off += 4;
        // i_reserved2 ([u8; 16]) — zero
        off += 16;

        debug_assert_eq!(off, EROFS_EXTENDED_INODE_SIZE);
        w.write_all(&buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a basic regular file inode for testing.
    fn test_inode() -> InodeInfo {
        InodeInfo {
            mode: S_IFREG | 0o644,
            uid: 1000,
            gid: 1000,
            size: 4096,
            nlink: 1,
            mtime: 1_700_000_000,
            mtime_nsec: 0,
            ino: 42,
            xattr_icount: 0,
            data_layout: EROFS_INODE_FLAT_PLAIN,
            union_value: 5,
        }
    }

    #[test]
    fn compact_inode_is_32_bytes() {
        let inode = test_inode();
        let mut buf = Vec::new();
        inode.write_compact(&mut buf).unwrap();
        assert_eq!(buf.len(), 32, "compact inode must be exactly 32 bytes");
    }

    #[test]
    fn extended_inode_is_64_bytes() {
        let inode = test_inode();
        let mut buf = Vec::new();
        inode.write_extended(&mut buf).unwrap();
        assert_eq!(buf.len(), 64, "extended inode must be exactly 64 bytes");
    }

    #[test]
    fn compact_format_encoding() {
        // compact FLAT_PLAIN: (0 << 1) | 0 = 0
        let mut inode = test_inode();
        inode.data_layout = EROFS_INODE_FLAT_PLAIN;
        let mut buf = Vec::new();
        inode.write_compact(&mut buf).unwrap();
        let i_format = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(i_format, 0, "compact FLAT_PLAIN i_format should be 0");

        // compact CHUNK_BASED: (4 << 1) | 0 = 8
        inode.data_layout = EROFS_INODE_CHUNK_BASED;
        buf.clear();
        inode.write_compact(&mut buf).unwrap();
        let i_format = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(i_format, 8, "compact CHUNK_BASED i_format should be 8");
    }

    #[test]
    fn extended_format_encoding() {
        // extended FLAT_PLAIN: (0 << 1) | 1 = 1
        let mut inode = test_inode();
        inode.data_layout = EROFS_INODE_FLAT_PLAIN;
        let mut buf = Vec::new();
        inode.write_extended(&mut buf).unwrap();
        let i_format = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(i_format, 1, "extended FLAT_PLAIN i_format should be 1");

        // extended CHUNK_BASED: (4 << 1) | 1 = 9
        inode.data_layout = EROFS_INODE_CHUNK_BASED;
        buf.clear();
        inode.write_extended(&mut buf).unwrap();
        let i_format = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(i_format, 9, "extended CHUNK_BASED i_format should be 9");
    }

    #[test]
    fn compact_flat_inline_format() {
        let mut inode = test_inode();
        inode.data_layout = EROFS_INODE_FLAT_INLINE;
        let mut buf = Vec::new();
        inode.write_compact(&mut buf).unwrap();
        let i_format = u16::from_le_bytes([buf[0], buf[1]]);
        // (2 << 1) | 0 = 4
        assert_eq!(i_format, 4, "compact FLAT_INLINE i_format should be 4");
    }

    #[test]
    fn file_mode_preserves_permissions() {
        // Directory with 0o755
        let mut inode = test_inode();
        inode.mode = S_IFDIR | 0o755;
        let mut buf = Vec::new();
        inode.write_compact(&mut buf).unwrap();
        let mode = u16::from_le_bytes([buf[4], buf[5]]);
        assert_eq!(mode, S_IFDIR | 0o755, "directory mode 0o755 must be preserved");

        // Regular file with 0o644
        inode.mode = S_IFREG | 0o644;
        buf.clear();
        inode.write_compact(&mut buf).unwrap();
        let mode = u16::from_le_bytes([buf[4], buf[5]]);
        assert_eq!(mode, S_IFREG | 0o644, "file mode 0o644 must be preserved");
    }

    #[test]
    fn nid_calculation() {
        // meta_blkaddr=1, block_size=4096 => meta starts at byte 4096
        // byte_offset=4096 => NID = (4096 - 4096) / 32 = 0
        assert_eq!(nid_from_offset(4096, 1, 4096), 0);

        // byte_offset=4128 => NID = (4128 - 4096) / 32 = 1
        assert_eq!(nid_from_offset(4128, 1, 4096), 1);

        // byte_offset=4096+320 => NID = 320 / 32 = 10
        assert_eq!(nid_from_offset(4416, 1, 4096), 10);

        // meta_blkaddr=2, block_size=4096 => meta starts at 8192
        // byte_offset=8224 => NID = (8224 - 8192) / 32 = 1
        assert_eq!(nid_from_offset(8224, 2, 4096), 1);
    }

    #[test]
    fn needs_extended_large_uid() {
        let mut inode = test_inode();
        assert!(!inode.needs_extended(), "small uid/gid/size should not need extended");

        inode.uid = 70000;
        assert!(inode.needs_extended(), "uid > 65535 must require extended format");
    }

    #[test]
    fn needs_extended_large_gid() {
        let mut inode = test_inode();
        inode.gid = 70000;
        assert!(inode.needs_extended(), "gid > 65535 must require extended format");
    }

    #[test]
    fn needs_extended_large_size() {
        let mut inode = test_inode();
        inode.size = u64::from(u32::MAX) + 1;
        assert!(inode.needs_extended(), "size > u32::MAX must require extended format");
    }

    #[test]
    fn extended_inode_preserves_full_uid_gid() {
        let mut inode = test_inode();
        inode.uid = 100_000;
        inode.gid = 200_000;

        let mut buf = Vec::new();
        inode.write_extended(&mut buf).unwrap();

        // uid at offset 24 (u32)
        let uid = u32::from_le_bytes(buf[24..28].try_into().unwrap());
        assert_eq!(uid, 100_000);

        // gid at offset 28 (u32)
        let gid = u32::from_le_bytes(buf[28..32].try_into().unwrap());
        assert_eq!(gid, 200_000);
    }

    #[test]
    fn extended_inode_preserves_large_size() {
        let mut inode = test_inode();
        inode.size = 5_000_000_000;

        let mut buf = Vec::new();
        inode.write_extended(&mut buf).unwrap();

        // i_size at offset 8 (u64)
        let size = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        assert_eq!(size, 5_000_000_000);
    }

    #[test]
    fn extended_inode_mtime_fields() {
        let mut inode = test_inode();
        inode.mtime = 1_700_000_000;
        inode.mtime_nsec = 123_456_789;

        let mut buf = Vec::new();
        inode.write_extended(&mut buf).unwrap();

        // i_mtime at offset 32 (u64)
        let mtime = u64::from_le_bytes(buf[32..40].try_into().unwrap());
        assert_eq!(mtime, 1_700_000_000);

        // i_mtime_nsec at offset 40 (u32)
        let mtime_nsec = u32::from_le_bytes(buf[40..44].try_into().unwrap());
        assert_eq!(mtime_nsec, 123_456_789);
    }

    #[test]
    fn compact_inode_mtime_written() {
        let mut inode = test_inode();
        inode.mtime = 1_700_000_000;

        let mut buf = Vec::new();
        inode.write_compact(&mut buf).unwrap();

        // i_mtime at offset 0x0C (u32) — lower 32 bits
        let mtime = u32::from_le_bytes(buf[0x0C..0x10].try_into().unwrap());
        assert_eq!(mtime, 1_700_000_000_u32, "compact inode must write i_mtime at offset 0x0C");
    }

    #[test]
    fn compact_inode_nlink_at_offset_06() {
        let mut inode = test_inode();
        inode.nlink = 42;

        let mut buf = Vec::new();
        inode.write_compact(&mut buf).unwrap();

        // i_nb at offset 0x06 (u16)
        let nlink = u16::from_le_bytes(buf[0x06..0x08].try_into().unwrap());
        assert_eq!(nlink, 42, "compact inode must write i_nb (nlink) at offset 0x06");
    }

    #[test]
    fn extended_inode_nb_at_offset_06() {
        let mut inode = test_inode();
        inode.nlink = 5;

        let mut buf = Vec::new();
        inode.write_extended(&mut buf).unwrap();

        // i_nb at offset 0x06 (u16)
        let nb = u16::from_le_bytes(buf[0x06..0x08].try_into().unwrap());
        assert_eq!(nb, 5, "extended inode must write i_nb at offset 0x06");

        // i_nlink at offset 0x2C (u32)
        let nlink = u32::from_le_bytes(buf[0x2C..0x30].try_into().unwrap());
        assert_eq!(nlink, 5, "extended inode must write i_nlink at offset 0x2C");
    }
}
