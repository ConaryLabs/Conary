// conary-erofs/src/tail_pack.rs
//! Tail-end packing for small files in EROFS images.
//!
//! Files smaller than the remaining block space after the inode and xattrs
//! can be packed directly into the inode's tail using `FLAT_INLINE` data
//! layout. This avoids allocating a full block for tiny files.
//!
//! On-disk layout:
//! ```text
//! [inode (32 or 64 bytes)][xattr ibody (variable)][tail data (rest of block)]
//! ```
//!
//! Primary use cases:
//! - Symlink targets (always small strings)
//! - Very small config files
//! - Metacopy flag files (zero-length data)

/// Determine if a file's data should be tail-packed.
///
/// Returns `true` if the data fits in the remaining space within the block
/// after the inode and xattr data.
#[must_use]
pub fn should_tail_pack(data_size: u64, inode_size: u32, xattr_size: u32, block_size: u32) -> bool {
    let used = inode_size + xattr_size;
    if used >= block_size {
        return false;
    }
    data_size <= u64::from(block_size - used)
}

/// Pack tail data after inode+xattr bytes within a single buffer.
///
/// The caller provides already-serialized inode and xattr bytes.
/// Returns the combined buffer (NOT padded to block size -- the builder
/// handles alignment).
#[must_use]
pub fn pack_tail(inode_bytes: &[u8], xattr_bytes: &[u8], data: &[u8]) -> Vec<u8> {
    let total = inode_bytes.len() + xattr_bytes.len() + data.len();
    let mut buf = Vec::with_capacity(total);
    buf.extend_from_slice(inode_bytes);
    buf.extend_from_slice(xattr_bytes);
    buf.extend_from_slice(data);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK_SIZE: u32 = 4096;
    const COMPACT_INODE: u32 = 32;
    const EXTENDED_INODE: u32 = 64;

    #[test]
    fn small_file_should_tail_pack() {
        // 100 bytes of data with a compact inode and 48 bytes of xattrs
        // Used: 32 + 48 = 80, remaining: 4016, data: 100 -> fits
        assert!(should_tail_pack(100, COMPACT_INODE, 48, BLOCK_SIZE));
    }

    #[test]
    fn large_file_should_not_tail_pack() {
        // Data exceeds remaining space after inode + xattrs
        // Used: 64 + 48 = 112, remaining: 3984, data: 5000 -> does not fit
        assert!(!should_tail_pack(5000, EXTENDED_INODE, 48, BLOCK_SIZE));
    }

    #[test]
    fn symlink_target_always_tail_packable() {
        let target = b"usr/bin";
        assert!(should_tail_pack(
            target.len() as u64,
            COMPACT_INODE,
            0,
            BLOCK_SIZE,
        ));
    }

    #[test]
    fn pack_tail_concatenates_correctly() {
        let inode = [0xAA; 32];
        let xattr = [0xBB; 16];
        let data = [0xCC; 8];

        let result = pack_tail(&inode, &xattr, &data);

        assert_eq!(result.len(), 32 + 16 + 8);
        assert_eq!(&result[..32], &[0xAA; 32]);
        assert_eq!(&result[32..48], &[0xBB; 16]);
        assert_eq!(&result[48..56], &[0xCC; 8]);
    }

    #[test]
    fn zero_length_data_works() {
        // Metacopy flag files have zero-length data
        assert!(should_tail_pack(0, COMPACT_INODE, 48, BLOCK_SIZE));

        let inode = [0x01; 32];
        let xattr = [0x02; 48];
        let result = pack_tail(&inode, &xattr, &[]);
        assert_eq!(result.len(), 32 + 48);
    }

    #[test]
    fn data_exactly_fills_remaining_space() {
        // Used: 32 + 48 = 80, remaining: 4016
        let remaining = BLOCK_SIZE - COMPACT_INODE - 48;
        assert!(should_tail_pack(
            u64::from(remaining),
            COMPACT_INODE,
            48,
            BLOCK_SIZE,
        ));
    }

    #[test]
    fn data_one_byte_over_remaining_space() {
        let remaining = BLOCK_SIZE - COMPACT_INODE - 48;
        assert!(!should_tail_pack(
            u64::from(remaining) + 1,
            COMPACT_INODE,
            48,
            BLOCK_SIZE,
        ));
    }

    #[test]
    fn inode_plus_xattr_fills_block() {
        // inode + xattr = block_size, no room for data
        assert!(!should_tail_pack(1, 2048, 2048, BLOCK_SIZE));
    }

    #[test]
    fn inode_plus_xattr_exceeds_block() {
        // Degenerate case: used > block_size
        assert!(!should_tail_pack(0, 3000, 2000, BLOCK_SIZE));
    }
}
