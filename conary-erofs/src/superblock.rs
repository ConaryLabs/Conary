// conary-erofs/src/superblock.rs
//! EROFS superblock structure and serialization.
//!
//! The superblock is the first on-disk structure in an EROFS image,
//! located at byte offset 1024. It is exactly 128 bytes and describes
//! the filesystem layout, feature flags, and metadata pointers.

use std::io::{self, Write};

/// EROFS magic number identifying the filesystem.
pub const EROFS_SUPER_MAGIC: u32 = 0xE0F5_E1E2;

/// Byte offset where the superblock begins (preceded by 1024 zero bytes).
pub const EROFS_SUPER_OFFSET: u64 = 1024;

/// Default log2 block size (2^12 = 4096 bytes).
pub const EROFS_DEFAULT_BLKBITS: u8 = 12;

// --- Feature flags ---

/// Superblock checksum is present and valid.
pub const EROFS_FEATURE_COMPAT_SB_CHKSUM: u32 = 0x0001;

/// Inodes may use chunk-based data layout.
pub const EROFS_FEATURE_INCOMPAT_CHUNKED_FILE: u32 = 0x0004;

/// A device table is present (for external blob references).
pub const EROFS_FEATURE_INCOMPAT_DEVICE_TABLE: u32 = 0x0008;

/// On-disk EROFS superblock (128 bytes, little-endian).
///
/// Field layout matches the Linux kernel's `struct erofs_super_block`
/// from `fs/erofs/erofs_fs.h`.
#[derive(Debug, Clone)]
pub struct Superblock {
    /// Magic number (`EROFS_SUPER_MAGIC`).
    pub magic: u32,
    /// CRC32C checksum of the superblock (with this field zeroed during computation).
    pub checksum: u32,
    /// Compatible feature flags.
    pub feature_compat: u32,
    /// Log2 of block size (e.g. 12 for 4096).
    pub blkszbits: u8,
    /// Extra superblock extension slots.
    pub sb_extslots: u8,
    /// NID of root directory inode.
    pub root_nid: u16,
    /// Total number of inodes.
    pub inos: u64,
    /// Image build time (seconds since epoch).
    pub build_time: u64,
    /// Nanosecond component of build time.
    pub build_time_nsec: u32,
    /// Total number of blocks.
    pub blocks: u32,
    /// Start block address of the metadata area.
    pub meta_blkaddr: u32,
    /// Start block address of the shared xattr area.
    pub xattr_blkaddr: u32,
    /// Filesystem UUID.
    pub uuid: [u8; 16],
    /// Volume name (null-padded).
    pub volume_name: [u8; 16],
    /// Incompatible feature flags.
    pub feature_incompat: u32,
    /// Union field: `available_compr_algs` or `lz4_max_distance`.
    pub u1: u16,
    /// Number of extra devices in the device table.
    pub extra_devices: u16,
    /// Slot offset for device table entries.
    pub devt_slotoff: u16,
    /// Log2 of directory block size.
    pub dirblkbits: u8,
    /// Number of long xattr name prefixes.
    pub xattr_prefix_count: u8,
    /// Start offset of xattr prefix entries.
    pub xattr_prefix_start: u32,
    /// NID of the packed inode (for packed/tail data).
    pub packed_nid: u64,
    /// Reserved xattr filter byte.
    pub xattr_filter_reserved: u8,
    /// Reserved bytes (padding to 128 total).
    pub reserved: [u8; 23],
}

impl Superblock {
    /// Create a new superblock with sensible defaults.
    ///
    /// `block_size` must be a power of two (typically 4096). The `blkszbits`
    /// field is computed as `log2(block_size)`.
    #[must_use]
    pub fn new(block_size: u32) -> Self {
        assert!(
            block_size.is_power_of_two() && block_size >= 512,
            "block_size must be a power of two >= 512"
        );
        let blkszbits = block_size.trailing_zeros() as u8;

        Self {
            magic: EROFS_SUPER_MAGIC,
            checksum: 0,
            feature_compat: EROFS_FEATURE_COMPAT_SB_CHKSUM,
            blkszbits,
            sb_extslots: 0,
            root_nid: 0,
            inos: 0,
            build_time: 0,
            build_time_nsec: 0,
            blocks: 0,
            meta_blkaddr: 0,
            xattr_blkaddr: 0,
            uuid: [0; 16],
            volume_name: [0; 16],
            feature_incompat: 0,
            u1: 0,
            extra_devices: 0,
            devt_slotoff: 0,
            dirblkbits: 0,
            xattr_prefix_count: 0,
            xattr_prefix_start: 0,
            packed_nid: 0,
            xattr_filter_reserved: 0,
            reserved: [0; 23],
        }
    }

    /// Serialize the superblock to bytes (128 bytes, little-endian).
    ///
    /// This writes only the 128-byte superblock itself, not the leading
    /// 1024-byte padding. Use [`write_to`](Self::write_to) for the
    /// complete on-disk representation.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 128] {
        let mut buf = [0u8; 128];
        let mut off = 0;

        macro_rules! put_le {
            ($val:expr, u8) => {
                buf[off] = $val;
                off += 1;
            };
            ($val:expr, u16) => {
                buf[off..off + 2].copy_from_slice(&$val.to_le_bytes());
                off += 2;
            };
            ($val:expr, u32) => {
                buf[off..off + 4].copy_from_slice(&$val.to_le_bytes());
                off += 4;
            };
            ($val:expr, u64) => {
                buf[off..off + 8].copy_from_slice(&$val.to_le_bytes());
                off += 8;
            };
        }

        put_le!(self.magic, u32);         // 0..4
        put_le!(self.checksum, u32);      // 4..8
        put_le!(self.feature_compat, u32); // 8..12
        put_le!(self.blkszbits, u8);      // 12
        put_le!(self.sb_extslots, u8);    // 13
        put_le!(self.root_nid, u16);      // 14..16
        put_le!(self.inos, u64);          // 16..24
        put_le!(self.build_time, u64);    // 24..32
        put_le!(self.build_time_nsec, u32); // 32..36
        put_le!(self.blocks, u32);        // 36..40
        put_le!(self.meta_blkaddr, u32);  // 40..44
        put_le!(self.xattr_blkaddr, u32); // 44..48
        buf[off..off + 16].copy_from_slice(&self.uuid); // 48..64
        off += 16;
        buf[off..off + 16].copy_from_slice(&self.volume_name); // 64..80
        off += 16;
        put_le!(self.feature_incompat, u32); // 80..84
        put_le!(self.u1, u16);            // 84..86
        put_le!(self.extra_devices, u16); // 86..88
        put_le!(self.devt_slotoff, u16);  // 88..90
        put_le!(self.dirblkbits, u8);     // 90
        put_le!(self.xattr_prefix_count, u8); // 91
        put_le!(self.xattr_prefix_start, u32); // 92..96
        put_le!(self.packed_nid, u64);    // 96..104
        put_le!(self.xattr_filter_reserved, u8); // 104
        buf[off..off + 23].copy_from_slice(&self.reserved); // 105..128
        off += 23;

        debug_assert_eq!(off, 128);
        buf
    }

    /// Parse a superblock from a 128-byte little-endian buffer.
    ///
    /// Returns `None` if the magic number does not match.
    #[must_use]
    pub fn from_bytes(buf: &[u8; 128]) -> Option<Self> {
        let mut off = 0;

        macro_rules! get_le {
            (u8) => {{
                let v = buf[off];
                off += 1;
                v
            }};
            (u16) => {{
                let v = u16::from_le_bytes(buf[off..off + 2].try_into().unwrap());
                off += 2;
                v
            }};
            (u32) => {{
                let v = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
                off += 4;
                v
            }};
            (u64) => {{
                let v = u64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
                off += 8;
                v
            }};
        }

        let magic = get_le!(u32);
        if magic != EROFS_SUPER_MAGIC {
            return None;
        }
        let checksum = get_le!(u32);
        let feature_compat = get_le!(u32);
        let blkszbits = get_le!(u8);
        let sb_extslots = get_le!(u8);
        let root_nid = get_le!(u16);
        let inos = get_le!(u64);
        let build_time = get_le!(u64);
        let build_time_nsec = get_le!(u32);
        let blocks = get_le!(u32);
        let meta_blkaddr = get_le!(u32);
        let xattr_blkaddr = get_le!(u32);

        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&buf[off..off + 16]);
        off += 16;

        let mut volume_name = [0u8; 16];
        volume_name.copy_from_slice(&buf[off..off + 16]);
        off += 16;

        let feature_incompat = get_le!(u32);
        let u1 = get_le!(u16);
        let extra_devices = get_le!(u16);
        let devt_slotoff = get_le!(u16);
        let dirblkbits = get_le!(u8);
        let xattr_prefix_count = get_le!(u8);
        let xattr_prefix_start = get_le!(u32);
        let packed_nid = get_le!(u64);
        let xattr_filter_reserved = get_le!(u8);

        let mut reserved = [0u8; 23];
        reserved.copy_from_slice(&buf[off..off + 23]);
        off += 23;

        debug_assert_eq!(off, 128);

        Some(Self {
            magic,
            checksum,
            feature_compat,
            blkszbits,
            sb_extslots,
            root_nid,
            inos,
            build_time,
            build_time_nsec,
            blocks,
            meta_blkaddr,
            xattr_blkaddr,
            uuid,
            volume_name,
            feature_incompat,
            u1,
            extra_devices,
            devt_slotoff,
            dirblkbits,
            xattr_prefix_count,
            xattr_prefix_start,
            packed_nid,
            xattr_filter_reserved,
            reserved,
        })
    }

    /// Compute the CRC32C checksum of this superblock.
    ///
    /// The checksum covers all 128 bytes with the `checksum` field itself
    /// set to zero during computation.
    #[must_use]
    pub fn compute_checksum(&self) -> u32 {
        let mut copy = self.clone();
        copy.checksum = 0;
        let bytes = copy.to_bytes();
        crc32c::crc32c(&bytes)
    }

    /// Write the full on-disk representation: 1024 zero bytes followed by
    /// the 128-byte superblock.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the underlying writer fails.
    pub fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        // 1024 bytes of leading padding (boot sector area).
        w.write_all(&[0u8; 1024])?;
        w.write_all(&self.to_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superblock_total_size() {
        let sb = Superblock::new(4096);
        let mut buf = Vec::new();
        sb.write_to(&mut buf).unwrap();
        assert_eq!(buf.len(), 1024 + 128, "superblock output must be 1152 bytes");
    }

    #[test]
    fn magic_at_offset_1024() {
        let sb = Superblock::new(4096);
        let mut buf = Vec::new();
        sb.write_to(&mut buf).unwrap();

        // Little-endian encoding of 0xE0F5_E1E2 is [0xE2, 0xE1, 0xF5, 0xE0].
        assert_eq!(&buf[1024..1028], &[0xE2, 0xE1, 0xF5, 0xE0]);
    }

    #[test]
    fn default_block_size() {
        let sb = Superblock::new(4096);
        assert_eq!(sb.blkszbits, 12);
    }

    #[test]
    fn checksum_is_valid_crc32c() {
        let mut sb = Superblock::new(4096);
        sb.inos = 42;
        sb.blocks = 100;
        sb.root_nid = 1;

        // Compute and store the checksum.
        sb.checksum = sb.compute_checksum();
        assert_ne!(sb.checksum, 0, "checksum should not be zero for non-trivial data");

        // Verify: re-computing with checksum field zeroed yields the same value.
        let expected = sb.compute_checksum();
        assert_eq!(sb.checksum, expected);
    }

    #[test]
    fn round_trip() {
        let mut sb = Superblock::new(4096);
        sb.root_nid = 37;
        sb.inos = 1500;
        sb.build_time = 1_700_000_000;
        sb.build_time_nsec = 123_456_789;
        sb.blocks = 256;
        sb.meta_blkaddr = 1;
        sb.xattr_blkaddr = 2;
        sb.uuid = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        sb.volume_name = *b"testvol\0\0\0\0\0\0\0\0\0";
        sb.feature_incompat = EROFS_FEATURE_INCOMPAT_CHUNKED_FILE
            | EROFS_FEATURE_INCOMPAT_DEVICE_TABLE;
        sb.u1 = 0x00FF;
        sb.extra_devices = 3;
        sb.devt_slotoff = 10;
        sb.dirblkbits = 12;
        sb.xattr_prefix_count = 1;
        sb.xattr_prefix_start = 100;
        sb.packed_nid = 99;
        sb.xattr_filter_reserved = 0xAB;
        sb.checksum = sb.compute_checksum();

        let bytes = sb.to_bytes();
        let parsed = Superblock::from_bytes(&bytes).expect("round-trip parse failed");

        assert_eq!(parsed.magic, EROFS_SUPER_MAGIC);
        assert_eq!(parsed.checksum, sb.checksum);
        assert_eq!(parsed.feature_compat, sb.feature_compat);
        assert_eq!(parsed.blkszbits, 12);
        assert_eq!(parsed.root_nid, 37);
        assert_eq!(parsed.inos, 1500);
        assert_eq!(parsed.build_time, 1_700_000_000);
        assert_eq!(parsed.build_time_nsec, 123_456_789);
        assert_eq!(parsed.blocks, 256);
        assert_eq!(parsed.meta_blkaddr, 1);
        assert_eq!(parsed.xattr_blkaddr, 2);
        assert_eq!(parsed.uuid, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        assert_eq!(&parsed.volume_name[..7], b"testvol");
        assert_eq!(
            parsed.feature_incompat,
            EROFS_FEATURE_INCOMPAT_CHUNKED_FILE | EROFS_FEATURE_INCOMPAT_DEVICE_TABLE
        );
        assert_eq!(parsed.u1, 0x00FF);
        assert_eq!(parsed.extra_devices, 3);
        assert_eq!(parsed.devt_slotoff, 10);
        assert_eq!(parsed.dirblkbits, 12);
        assert_eq!(parsed.xattr_prefix_count, 1);
        assert_eq!(parsed.xattr_prefix_start, 100);
        assert_eq!(parsed.packed_nid, 99);
        assert_eq!(parsed.xattr_filter_reserved, 0xAB);
    }

    #[test]
    fn bad_magic_returns_none() {
        let mut bytes = [0u8; 128];
        // Write wrong magic.
        bytes[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        assert!(Superblock::from_bytes(&bytes).is_none());
    }

    #[test]
    fn leading_padding_is_zeroed() {
        let sb = Superblock::new(4096);
        let mut buf = Vec::new();
        sb.write_to(&mut buf).unwrap();
        assert!(buf[..1024].iter().all(|&b| b == 0), "first 1024 bytes must be zero");
    }

    #[test]
    fn alternate_block_size() {
        let sb = Superblock::new(512);
        assert_eq!(sb.blkszbits, 9);

        let sb = Superblock::new(8192);
        assert_eq!(sb.blkszbits, 13);
    }

    #[test]
    #[should_panic(expected = "block_size must be a power of two")]
    fn non_power_of_two_panics() {
        let _ = Superblock::new(1000);
    }

    #[test]
    fn struct_size_is_128_bytes() {
        let sb = Superblock::new(4096);
        let bytes = sb.to_bytes();
        assert_eq!(bytes.len(), 128);
    }

    #[test]
    fn checksum_changes_with_data() {
        let sb1 = Superblock::new(4096);
        let mut sb2 = Superblock::new(4096);
        sb2.blocks = 999;

        assert_ne!(
            sb1.compute_checksum(),
            sb2.compute_checksum(),
            "different data must produce different checksums"
        );
    }
}
