// conary-erofs/src/xattr.rs
//! Extended attribute storage for composefs digests and overlay metadata.
//!
//! EROFS inodes can carry inline xattrs in an "ibody" region that follows
//! the fixed inode structure. Each xattr entry uses a compact encoding
//! where well-known namespace prefixes (e.g. `"trusted."`) are replaced
//! by a single-byte index, and only the suffix is stored.
//!
//! For composefs, every regular file needs two xattrs:
//! - `trusted.overlay.redirect` with the CAS object path as value
//! - `trusted.overlay.metacopy` with an empty value (presence flag)

use std::io::{self, Write};

// ---------------------------------------------------------------------------
// Name-index constants (namespace prefix shortcuts)
// ---------------------------------------------------------------------------

/// `"user."` namespace prefix.
pub const EROFS_XATTR_INDEX_USER: u8 = 1;

/// `"system.posix_acl_access"` namespace prefix.
pub const EROFS_XATTR_INDEX_POSIX_ACL_ACCESS: u8 = 2;

/// `"system.posix_acl_default"` namespace prefix.
pub const EROFS_XATTR_INDEX_POSIX_ACL_DEFAULT: u8 = 3;

/// `"trusted."` namespace prefix.
pub const EROFS_XATTR_INDEX_TRUSTED: u8 = 4;

/// `"security."` namespace prefix.
pub const EROFS_XATTR_INDEX_SECURITY: u8 = 6;

/// Size of the xattr inline-body header in bytes.
///
/// Layout: `h_reserved` (4) + `h_shared_count` (1) + `h_reserved2` (7) = 12.
const XATTR_IBODY_HEADER_SIZE: usize = 12;

// ---------------------------------------------------------------------------
// Xattr entry
// ---------------------------------------------------------------------------

/// A single extended attribute entry ready for serialization.
///
/// The on-disk format is:
/// ```text
/// e_name_len   (u8)   — length of the name suffix
/// e_name_index (u8)   — namespace prefix index
/// e_value_size (u16)  — value length, little-endian
/// name suffix  [u8; e_name_len]
/// value        [u8; e_value_size]
/// ```
/// followed by zero-padding to a 4-byte boundary.
pub struct XattrEntry {
    /// Namespace prefix index (see `EROFS_XATTR_INDEX_*` constants).
    pub name_index: u8,
    /// Name suffix (everything after the namespace prefix).
    pub name_suffix: Vec<u8>,
    /// Attribute value (may be empty).
    pub value: Vec<u8>,
}

impl XattrEntry {
    /// Serialize this xattr entry to `w`, including 4-byte alignment padding.
    ///
    /// Returns the total number of bytes written (always a multiple of 4).
    pub fn write_to<W: Write>(&self, w: &mut W) -> io::Result<usize> {
        let name_len: u8 = self
            .name_suffix
            .len()
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name suffix too long"))?;
        let value_size: u16 = self
            .value
            .len()
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "value too long"))?;

        // 4-byte entry header
        w.write_all(&[name_len])?;
        w.write_all(&[self.name_index])?;
        w.write_all(&value_size.to_le_bytes())?;

        // Name suffix + value
        w.write_all(&self.name_suffix)?;
        w.write_all(&self.value)?;

        let unaligned = 4 + self.name_suffix.len() + self.value.len();
        let aligned = align4(unaligned);
        let padding = aligned - unaligned;
        if padding > 0 {
            w.write_all(&[0u8; 3][..padding])?;
        }

        Ok(aligned)
    }

    /// Compute the 4-byte-aligned serialized size of this entry.
    fn aligned_size(&self) -> usize {
        align4(4 + self.name_suffix.len() + self.value.len())
    }
}

// ---------------------------------------------------------------------------
// Ibody builder
// ---------------------------------------------------------------------------

/// Build an xattr inline body from a list of entries.
///
/// Returns `(ibody_bytes, xattr_icount)` where:
/// - `ibody_bytes` includes the 12-byte header followed by serialized entries
/// - `xattr_icount` is the value for the inode's `i_xattr_icount` field
///
/// If `entries` is empty, returns an empty vec and icount of 0.
pub fn build_xattr_ibody(entries: &[XattrEntry]) -> (Vec<u8>, u16) {
    if entries.is_empty() {
        return (Vec::new(), 0);
    }

    let total_entry_bytes: usize = entries.iter().map(XattrEntry::aligned_size).sum();

    // i_xattr_icount = ceil(total_entry_bytes / 4) + 1
    // Since each entry is already 4-byte aligned, total_entry_bytes is a
    // multiple of 4, so ceil is just the division.
    let icount = (total_entry_bytes / 4 + 1) as u16;

    let ibody_size = XATTR_IBODY_HEADER_SIZE + total_entry_bytes;
    let mut buf = Vec::with_capacity(ibody_size);

    // Header: h_reserved (4 bytes) + h_shared_count (1) + h_reserved2 (7)
    buf.extend_from_slice(&[0u8; XATTR_IBODY_HEADER_SIZE]);

    // Serialize each entry
    for entry in entries {
        entry
            .write_to(&mut buf)
            .expect("writing to Vec<u8> should not fail");
    }

    debug_assert_eq!(buf.len(), ibody_size);
    (buf, icount)
}

// ---------------------------------------------------------------------------
// Composefs convenience
// ---------------------------------------------------------------------------

/// Build xattr inline body for a composefs file.
///
/// `cas_path` is the CAS object path, e.g. `"ab/c123def456..."` where the
/// first two hex characters form a subdirectory prefix.
///
/// The resulting ibody contains two `trusted.overlay.*` xattrs:
/// 1. `trusted.overlay.redirect` with the CAS path as its value
/// 2. `trusted.overlay.metacopy` with an empty value (presence flag)
///
/// Returns `(ibody_bytes, xattr_icount)`.
pub fn build_composefs_xattrs(cas_path: &str) -> (Vec<u8>, u16) {
    let entries = [
        XattrEntry {
            name_index: EROFS_XATTR_INDEX_TRUSTED,
            name_suffix: b"overlay.redirect".to_vec(),
            value: cas_path.as_bytes().to_vec(),
        },
        XattrEntry {
            name_index: EROFS_XATTR_INDEX_TRUSTED,
            name_suffix: b"overlay.metacopy".to_vec(),
            value: Vec::new(),
        },
    ];
    build_xattr_ibody(&entries)
}

/// Round `n` up to the next multiple of 4.
const fn align4(n: usize) -> usize {
    (n + 3) & !3
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xattr_entry_serializes_header_correctly() {
        let entry = XattrEntry {
            name_index: EROFS_XATTR_INDEX_TRUSTED,
            name_suffix: b"overlay.redirect".to_vec(),
            value: b"/cas/ab/cdef".to_vec(),
        };

        let mut buf = Vec::new();
        let written = entry.write_to(&mut buf).unwrap();

        // Header fields
        assert_eq!(buf[0], 16); // e_name_len = len("overlay.redirect")
        assert_eq!(buf[1], EROFS_XATTR_INDEX_TRUSTED); // e_name_index
        assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 12); // e_value_size

        // Name suffix
        assert_eq!(&buf[4..20], b"overlay.redirect");

        // Value
        assert_eq!(&buf[20..32], b"/cas/ab/cdef");

        // Total is 4-byte aligned
        assert_eq!(written % 4, 0);
        assert_eq!(buf.len(), written);
    }

    #[test]
    fn xattr_entry_alignment_padding() {
        // 4 (header) + 3 (name) + 0 (value) = 7 -> aligned to 8
        let entry = XattrEntry {
            name_index: EROFS_XATTR_INDEX_USER,
            name_suffix: b"foo".to_vec(),
            value: Vec::new(),
        };

        let mut buf = Vec::new();
        let written = entry.write_to(&mut buf).unwrap();

        assert_eq!(written, 8);
        assert_eq!(buf.len(), 8);
        // Last byte should be zero padding
        assert_eq!(buf[7], 0);
    }

    #[test]
    fn xattr_entry_exact_alignment_no_padding() {
        // 4 (header) + 4 (name) + 0 (value) = 8 -> already aligned
        let entry = XattrEntry {
            name_index: EROFS_XATTR_INDEX_USER,
            name_suffix: b"test".to_vec(),
            value: Vec::new(),
        };

        let mut buf = Vec::new();
        let written = entry.write_to(&mut buf).unwrap();

        assert_eq!(written, 8);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn composefs_xattrs_redirect_contains_cas_path() {
        let cas_path = "ab/c123def456789";
        let (ibody, _) = build_composefs_xattrs(cas_path);

        // Skip the 12-byte header, then read the first entry
        let entry_start = XATTR_IBODY_HEADER_SIZE;
        let name_len = ibody[entry_start] as usize;
        let value_size =
            u16::from_le_bytes([ibody[entry_start + 2], ibody[entry_start + 3]]) as usize;

        assert_eq!(name_len, 16); // "overlay.redirect"
        assert_eq!(
            &ibody[entry_start + 4..entry_start + 4 + name_len],
            b"overlay.redirect"
        );

        let value_offset = entry_start + 4 + name_len;
        assert_eq!(
            &ibody[value_offset..value_offset + value_size],
            cas_path.as_bytes()
        );
    }

    #[test]
    fn composefs_xattrs_metacopy_has_empty_value() {
        let (ibody, _) = build_composefs_xattrs("ab/cdef0123456789");

        // Skip header + first entry (redirect)
        let redirect_entry = XattrEntry {
            name_index: EROFS_XATTR_INDEX_TRUSTED,
            name_suffix: b"overlay.redirect".to_vec(),
            value: b"ab/cdef0123456789".to_vec(),
        };
        let first_entry_size = redirect_entry.aligned_size();
        let metacopy_start = XATTR_IBODY_HEADER_SIZE + first_entry_size;

        let name_len = ibody[metacopy_start] as usize;
        let name_index = ibody[metacopy_start + 1];
        let value_size =
            u16::from_le_bytes([ibody[metacopy_start + 2], ibody[metacopy_start + 3]]) as usize;

        assert_eq!(name_len, 16); // "overlay.metacopy"
        assert_eq!(name_index, EROFS_XATTR_INDEX_TRUSTED);
        assert_eq!(value_size, 0);
        assert_eq!(
            &ibody[metacopy_start + 4..metacopy_start + 4 + name_len],
            b"overlay.metacopy"
        );
    }

    #[test]
    fn ibody_starts_with_zeroed_header() {
        let (ibody, _) = build_composefs_xattrs("ab/cdef");

        // First 12 bytes should all be zero (h_reserved + h_shared_count + h_reserved2)
        assert_eq!(&ibody[..XATTR_IBODY_HEADER_SIZE], &[0u8; 12]);
    }

    #[test]
    fn xattr_icount_computed_correctly() {
        // Two entries: redirect + metacopy
        let cas_path = "ab/c123def456789abcdef0123456789abcdef0123456789abcdef0123456789ab";
        let (ibody, icount) = build_composefs_xattrs(cas_path);

        let total_entry_bytes = ibody.len() - XATTR_IBODY_HEADER_SIZE;
        let expected_icount = (total_entry_bytes / 4 + 1) as u16;

        assert_eq!(icount, expected_icount);
        assert!(icount > 0);

        // Verify the kernel formula: xattr_ibody_size = 12 + (icount - 1) * 4
        let kernel_size = XATTR_IBODY_HEADER_SIZE + (icount as usize - 1) * 4;
        assert_eq!(kernel_size, ibody.len());
    }

    #[test]
    fn empty_xattr_list_returns_zero() {
        let (ibody, icount) = build_xattr_ibody(&[]);

        assert!(ibody.is_empty());
        assert_eq!(icount, 0);
    }

    #[test]
    fn single_entry_icount() {
        let entries = [XattrEntry {
            name_index: EROFS_XATTR_INDEX_SECURITY,
            name_suffix: b"selinux".to_vec(),
            value: b"system_u:object_r:usr_t:s0".to_vec(),
        }];

        let (ibody, icount) = build_xattr_ibody(&entries);

        // 4 + 7 + 26 = 37 -> aligned to 40
        let entry_bytes = ibody.len() - XATTR_IBODY_HEADER_SIZE;
        assert_eq!(entry_bytes, 40);
        assert_eq!(icount, (40 / 4 + 1) as u16); // 11
    }
}
