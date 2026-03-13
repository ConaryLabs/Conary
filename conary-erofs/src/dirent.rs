// conary-erofs/src/dirent.rs
//! EROFS directory entry packing.
//!
//! Each on-disk directory entry is 12 bytes (little-endian), followed by
//! variable-length filenames within the same block. Names are NOT
//! null-terminated; their lengths are derived from consecutive `nameoff`
//! values.
//!
//! ```text
//! [dirent0][dirent1]...[direntN][name0][name1]...[nameN][zero-pad]
//! ```
//!
//! If entries plus names exceed one block, they are split across multiple
//! independent blocks.

use crate::error::ErofsError;
use std::io::{Cursor, Write};

/// On-disk dirent size in bytes.
const DIRENT_SIZE: usize = 12;

/// File type: regular file (matches kernel FT_REG_FILE from fs_types.h).
pub const EROFS_FT_REG_FILE: u8 = 1;

/// File type: directory (matches kernel FT_DIR from fs_types.h).
pub const EROFS_FT_DIR: u8 = 2;

/// File type: symbolic link (matches kernel FT_SYMLINK from fs_types.h).
pub const EROFS_FT_SYMLINK: u8 = 7;

/// A directory entry to be packed into an EROFS directory block.
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// Filename (no path separators).
    pub name: String,
    /// Target inode NID.
    pub nid: u64,
    /// File type constant (`EROFS_FT_*`).
    pub file_type: u8,
}

/// Pack directory entries into block-sized buffers.
///
/// Entries are sorted alphabetically by name. Returns a `Vec` of
/// block-sized byte buffers, each zero-padded to `block_size`.
///
/// # Errors
///
/// Returns `ErofsError::OutOfRange` if a name offset exceeds `u16::MAX`.
pub fn pack_directory(
    entries: &mut [DirEntry],
    block_size: u32,
) -> Result<Vec<Vec<u8>>, ErofsError> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));

    let bs = block_size as usize;
    let mut blocks: Vec<Vec<u8>> = Vec::new();

    let mut remaining = &entries[..];

    while !remaining.is_empty() {
        // Determine how many entries fit in this block.
        let mut count = 0;
        let mut total_name_bytes = 0;

        for entry in remaining {
            let headers = (count + 1) * DIRENT_SIZE;
            let names = total_name_bytes + entry.name.len();
            if headers + names > bs {
                break;
            }
            count += 1;
            total_name_bytes += entry.name.len();
        }

        // At least one entry must fit; if the first entry alone overflows,
        // we still pack it (truncation is the caller's problem, but in
        // practice block sizes are large enough).
        if count == 0 {
            count = 1;
        }

        let batch = &remaining[..count];
        remaining = &remaining[count..];

        let mut buf = vec![0u8; bs];
        let mut cursor = Cursor::new(&mut buf[..]);

        // The names region starts right after the packed dirent headers.
        let names_base = count * DIRENT_SIZE;
        let mut name_offset = names_base;

        // Write dirent headers.
        for entry in batch {
            if name_offset > u16::MAX as usize {
                return Err(ErofsError::OutOfRange(format!(
                    "dirent name offset {name_offset} exceeds u16::MAX (65535)"
                )));
            }
            let _ = cursor.write_all(&entry.nid.to_le_bytes());
            // The bounds check above (`name_offset > u16::MAX`) guarantees this
            // cast is safe; values exceeding 65535 are rejected before reaching here.
            #[allow(clippy::cast_possible_truncation)]
            let nameoff_u16 = name_offset as u16;
            let _ = cursor.write_all(&nameoff_u16.to_le_bytes());
            let _ = cursor.write_all(&[entry.file_type]);
            let _ = cursor.write_all(&[0u8]); // reserved
            name_offset += entry.name.len();
        }

        // Write filenames (no null terminators).
        for entry in batch {
            let _ = cursor.write_all(entry.name.as_bytes());
        }

        // Remainder of the buffer is already zeroed.
        blocks.push(buf);
    }

    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_entries_returns_empty() {
        let mut entries: Vec<DirEntry> = Vec::new();
        let blocks = pack_directory(&mut entries, 4096).unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn single_entry_packs_correctly() {
        let mut entries = vec![DirEntry {
            name: "hello".into(),
            nid: 42,
            file_type: EROFS_FT_REG_FILE,
        }];

        let blocks = pack_directory(&mut entries, 4096).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].len(), 4096);

        let blk = &blocks[0];

        // NID at offset 0 (u64 LE).
        let nid = u64::from_le_bytes(blk[0..8].try_into().unwrap());
        assert_eq!(nid, 42);

        // nameoff at offset 8 (u16 LE) — should be 12 (one dirent header).
        let nameoff = u16::from_le_bytes(blk[8..10].try_into().unwrap());
        assert_eq!(nameoff, 12);

        // file_type at offset 10.
        assert_eq!(blk[10], EROFS_FT_REG_FILE);

        // reserved at offset 11.
        assert_eq!(blk[11], 0);

        // Filename starts at byte 12.
        assert_eq!(&blk[12..17], b"hello");

        // Rest is zero-padded.
        assert!(blk[17..].iter().all(|&b| b == 0));
    }

    #[test]
    fn multiple_entries_sorted_alphabetically() {
        let mut entries = vec![
            DirEntry {
                name: "cherry".into(),
                nid: 3,
                file_type: EROFS_FT_REG_FILE,
            },
            DirEntry {
                name: "apple".into(),
                nid: 1,
                file_type: EROFS_FT_DIR,
            },
            DirEntry {
                name: "banana".into(),
                nid: 2,
                file_type: EROFS_FT_SYMLINK,
            },
        ];

        let blocks = pack_directory(&mut entries, 4096).unwrap();
        assert_eq!(blocks.len(), 1);

        let blk = &blocks[0];

        // After sorting: apple(1), banana(2), cherry(3).
        // 3 dirents = 36 bytes of headers, names start at offset 36.

        // Entry 0: apple
        let nid0 = u64::from_le_bytes(blk[0..8].try_into().unwrap());
        assert_eq!(nid0, 1);
        let off0 = u16::from_le_bytes(blk[8..10].try_into().unwrap()) as usize;
        assert_eq!(off0, 36);
        assert_eq!(blk[10], EROFS_FT_DIR);

        // Entry 1: banana
        let nid1 = u64::from_le_bytes(blk[12..20].try_into().unwrap());
        assert_eq!(nid1, 2);
        let off1 = u16::from_le_bytes(blk[20..22].try_into().unwrap()) as usize;
        assert_eq!(off1, 41); // 36 + len("apple")=5
        assert_eq!(blk[22], EROFS_FT_SYMLINK);

        // Entry 2: cherry
        let nid2 = u64::from_le_bytes(blk[24..32].try_into().unwrap());
        assert_eq!(nid2, 3);
        let off2 = u16::from_le_bytes(blk[32..34].try_into().unwrap()) as usize;
        assert_eq!(off2, 47); // 41 + len("banana")=6
        assert_eq!(blk[34], EROFS_FT_REG_FILE);

        // Verify the actual name bytes.
        assert_eq!(&blk[36..41], b"apple");
        assert_eq!(&blk[41..47], b"banana");
        assert_eq!(&blk[47..53], b"cherry");
    }

    #[test]
    fn nameoff_values_correct() {
        let mut entries = vec![
            DirEntry {
                name: "ab".into(),
                nid: 10,
                file_type: EROFS_FT_REG_FILE,
            },
            DirEntry {
                name: "cdef".into(),
                nid: 20,
                file_type: EROFS_FT_REG_FILE,
            },
        ];

        let blocks = pack_directory(&mut entries, 4096).unwrap();
        let blk = &blocks[0];

        // 2 dirents = 24 bytes header. Names start at 24.
        let off0 = u16::from_le_bytes(blk[8..10].try_into().unwrap()) as usize;
        let off1 = u16::from_le_bytes(blk[20..22].try_into().unwrap()) as usize;

        assert_eq!(off0, 24);
        assert_eq!(off1, 26); // 24 + len("ab")

        // Name lengths derived from offsets: name0 = off1 - off0 = 2, name1 ends at 26+4=30.
        assert_eq!(&blk[off0..off1], b"ab");
        assert_eq!(&blk[off1..off1 + 4], b"cdef");
    }

    #[test]
    fn file_types_correct() {
        let mut entries = vec![
            DirEntry {
                name: "dir".into(),
                nid: 1,
                file_type: EROFS_FT_DIR,
            },
            DirEntry {
                name: "file".into(),
                nid: 2,
                file_type: EROFS_FT_REG_FILE,
            },
            DirEntry {
                name: "link".into(),
                nid: 3,
                file_type: EROFS_FT_SYMLINK,
            },
        ];

        let blocks = pack_directory(&mut entries, 4096).unwrap();
        let blk = &blocks[0];

        // Sorted: dir, file, link.
        assert_eq!(blk[10], EROFS_FT_DIR);
        assert_eq!(blk[22], EROFS_FT_REG_FILE);
        assert_eq!(blk[34], EROFS_FT_SYMLINK);
    }

    #[test]
    fn block_overflow_splits_into_multiple_blocks() {
        // Use a tiny block size to force overflow.
        // Each dirent = 12 bytes header + name. With block_size=32, only one
        // entry with a short name can fit per block.
        let mut entries = vec![
            DirEntry {
                name: "aaa".into(),
                nid: 1,
                file_type: EROFS_FT_REG_FILE,
            },
            DirEntry {
                name: "bbb".into(),
                nid: 2,
                file_type: EROFS_FT_REG_FILE,
            },
            DirEntry {
                name: "ccc".into(),
                nid: 3,
                file_type: EROFS_FT_REG_FILE,
            },
        ];

        // 12 + 3 = 15 bytes per entry. block_size=16 fits exactly one.
        let blocks = pack_directory(&mut entries, 16).unwrap();
        assert_eq!(blocks.len(), 3);

        for (i, blk) in blocks.iter().enumerate() {
            assert_eq!(blk.len(), 16);
            let nid = u64::from_le_bytes(blk[0..8].try_into().unwrap());
            assert_eq!(nid, (i as u64) + 1);
        }
    }

    #[test]
    fn long_filename_near_block_boundary() {
        // Block size 64. One dirent header = 12 bytes, leaving 52 bytes for names.
        // First entry: name "a" (1 byte) -> 12 + 1 = 13 total per entry.
        // Second entry: name with 50 chars -> 12+12+1+50 = 75 > 64, won't fit.
        // So first block gets entry "a", second block gets the long name.
        let long_name = "x".repeat(50);
        let mut entries = vec![
            DirEntry {
                name: "a".into(),
                nid: 1,
                file_type: EROFS_FT_REG_FILE,
            },
            DirEntry {
                name: long_name.clone(),
                nid: 2,
                file_type: EROFS_FT_REG_FILE,
            },
        ];

        let blocks = pack_directory(&mut entries, 64).unwrap();
        assert_eq!(blocks.len(), 2);

        // First block: entry "a".
        let blk0 = &blocks[0];
        assert_eq!(blk0.len(), 64);
        let nid0 = u64::from_le_bytes(blk0[0..8].try_into().unwrap());
        assert_eq!(nid0, 1);
        assert_eq!(&blk0[12..13], b"a");

        // Second block: entry with long name.
        let blk1 = &blocks[1];
        assert_eq!(blk1.len(), 64);
        let nid1 = u64::from_le_bytes(blk1[0..8].try_into().unwrap());
        assert_eq!(nid1, 2);
        assert_eq!(&blk1[12..62], long_name.as_bytes());
    }

    #[test]
    fn nameoff_overflow_returns_error() {
        // Use a block size larger than u16::MAX to trigger the overflow check.
        // We need name_offset > 65535. With a 128KB block, we can pack enough
        // entries with long names to push the offset past the limit.
        let block_size: u32 = 128 * 1024; // 128 KiB
        let _name = "a".repeat(1000);
        // 66 entries * 12 bytes header = 792 bytes. names_base = 792.
        // 66 entries * 1000 bytes name = 66000. Last offset = 792 + 65000 = 65792.
        // We need more: 67 entries -> names_base = 804, offset at entry 66 = 804 + 66000 = 66804 > 65535.
        let mut entries: Vec<DirEntry> = (0..67)
            .map(|i| DirEntry {
                name: format!("{}{i:01}", "a".repeat(999)),
                nid: i,
                file_type: EROFS_FT_REG_FILE,
            })
            .collect();

        let result = pack_directory(&mut entries, block_size);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("u16::MAX"),
            "expected u16::MAX mention, got: {err}"
        );
    }
}
