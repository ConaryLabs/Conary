// conary-erofs/src/verify.rs
//! EROFS image reader and verifier.
//!
//! Parses EROFS images produced by [`ErofsBuilder`](crate::builder::ErofsBuilder)
//! and validates their structure. Designed for unit testing -- no kernel mounting
//! required.
//!
//! Only handles the subset of EROFS that our builder produces: compact inodes,
//! composefs xattrs, chunk-based files, inline symlinks, and plain directories.

use std::io::{self, Read, Seek, SeekFrom};

use crate::dirent::{EROFS_FT_DIR, EROFS_FT_REG_FILE, EROFS_FT_SYMLINK};
use crate::inode::{EROFS_COMPACT_INODE_SIZE, EROFS_EXTENDED_INODE_SIZE};
use crate::superblock::{EROFS_SUPER_MAGIC, EROFS_SUPER_OFFSET};
use crate::xattr::EROFS_XATTR_INDEX_TRUSTED;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Information about a verified EROFS image.
#[derive(Debug)]
pub struct ImageInfo {
    /// Block size in bytes.
    pub block_size: u32,
    /// Total number of inodes recorded in the superblock.
    pub inode_count: u64,
    /// NID of the root directory inode.
    pub root_nid: u64,
    /// Compatible feature flags.
    pub features_compat: u32,
    /// Incompatible feature flags.
    pub features_incompat: u32,
    /// All files discovered by walking the directory tree.
    pub files: Vec<FileInfo>,
}

/// Information about a single file in the image.
#[derive(Debug)]
pub struct FileInfo {
    /// Full path from the root (e.g. `"usr/bin/foo"`).
    pub path: String,
    /// Type of this entry.
    pub file_type: FileType,
    /// Permission + type mode bits.
    pub mode: u16,
    /// File size in bytes.
    pub size: u64,
    /// Composefs CAS digest extracted from the `trusted.overlay.redirect` xattr,
    /// if present.
    pub digest: Option<[u8; 32]>,
}

/// File type enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
}

// ---------------------------------------------------------------------------
// Parsed inode (internal)
// ---------------------------------------------------------------------------

/// Parsed inode fields needed for verification.
struct ParsedInode {
    mode: u16,
    size: u64,
    union_value: u32,
    xattr_icount: u16,
    /// Whether the on-disk format was extended (64 bytes) vs compact (32 bytes).
    is_extended: bool,
}

impl ParsedInode {
    fn inode_size(&self) -> u64 {
        if self.is_extended {
            EROFS_EXTENDED_INODE_SIZE as u64
        } else {
            EROFS_COMPACT_INODE_SIZE as u64
        }
    }

    /// Total xattr ibody size (0 if no xattrs).
    fn xattr_ibody_size(&self) -> u64 {
        if self.xattr_icount == 0 {
            0
        } else {
            12 + u64::from(self.xattr_icount - 1) * 4
        }
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Parse and validate an EROFS image.
///
/// Reads the superblock, verifies the magic number and CRC32C checksum, then
/// walks the directory tree starting from the root inode to collect all files.
///
/// # Errors
///
/// Returns an I/O error if the image is malformed or truncated.
pub fn verify_image<R: Read + Seek>(mut reader: R) -> io::Result<ImageInfo> {
    // 1. Read superblock at offset 1024.
    reader.seek(SeekFrom::Start(EROFS_SUPER_OFFSET))?;
    let mut sb_bytes = [0u8; 128];
    reader.read_exact(&mut sb_bytes)?;

    // Verify magic.
    let magic = u32::from_le_bytes(sb_bytes[0..4].try_into().unwrap());
    if magic != EROFS_SUPER_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad EROFS magic: expected 0x{EROFS_SUPER_MAGIC:08X}, got 0x{magic:08X}"),
        ));
    }

    // Verify CRC32C checksum.
    let stored_checksum = u32::from_le_bytes(sb_bytes[4..8].try_into().unwrap());
    let mut check_bytes = sb_bytes;
    check_bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
    let computed = crc32c::crc32c(&check_bytes);
    if stored_checksum != computed {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "superblock checksum mismatch: stored 0x{stored_checksum:08X}, computed 0x{computed:08X}"
            ),
        ));
    }

    // Extract superblock fields.
    let features_compat = u32::from_le_bytes(sb_bytes[8..12].try_into().unwrap());
    let blkszbits = sb_bytes[12];
    let block_size: u32 = 1 << blkszbits;
    let root_nid = u64::from(u16::from_le_bytes(sb_bytes[14..16].try_into().unwrap()));
    let inode_count = u64::from_le_bytes(sb_bytes[16..24].try_into().unwrap());
    let meta_blkaddr = u32::from_le_bytes(sb_bytes[40..44].try_into().unwrap());
    let features_incompat = u32::from_le_bytes(sb_bytes[80..84].try_into().unwrap());

    // 2. Walk directory tree from root.
    let mut files = Vec::new();
    walk_directory(
        &mut reader,
        root_nid,
        String::new(),
        meta_blkaddr,
        block_size,
        &mut files,
    )?;

    Ok(ImageInfo {
        block_size,
        inode_count,
        root_nid,
        features_compat,
        features_incompat,
        files,
    })
}

// ---------------------------------------------------------------------------
// Inode reading
// ---------------------------------------------------------------------------

/// Read and parse an inode at the given NID.
fn read_inode<R: Read + Seek>(
    reader: &mut R,
    nid: u64,
    meta_blkaddr: u32,
    block_size: u32,
) -> io::Result<ParsedInode> {
    let offset = u64::from(meta_blkaddr) * u64::from(block_size) + nid * 32;
    reader.seek(SeekFrom::Start(offset))?;

    // Read first 2 bytes to determine format version.
    let mut fmt_buf = [0u8; 2];
    reader.read_exact(&mut fmt_buf)?;
    let i_format = u16::from_le_bytes(fmt_buf);
    let version = i_format & 1; // 0 = compact, 1 = extended

    // Seek back to start of inode and read full structure.
    reader.seek(SeekFrom::Start(offset))?;

    if version == 0 {
        // Compact inode (32 bytes).
        let mut buf = [0u8; EROFS_COMPACT_INODE_SIZE];
        reader.read_exact(&mut buf)?;

        let xattr_icount = u16::from_le_bytes(buf[2..4].try_into().unwrap());
        let mode = u16::from_le_bytes(buf[4..6].try_into().unwrap());
        let size = u64::from(u32::from_le_bytes(buf[8..12].try_into().unwrap()));
        let union_value = u32::from_le_bytes(buf[16..20].try_into().unwrap());

        Ok(ParsedInode {
            mode,
            size,
            union_value,
            xattr_icount,
            is_extended: false,
        })
    } else {
        // Extended inode (64 bytes).
        let mut buf = [0u8; EROFS_EXTENDED_INODE_SIZE];
        reader.read_exact(&mut buf)?;

        let xattr_icount = u16::from_le_bytes(buf[2..4].try_into().unwrap());
        let mode = u16::from_le_bytes(buf[4..6].try_into().unwrap());
        let size = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let union_value = u32::from_le_bytes(buf[16..20].try_into().unwrap());

        Ok(ParsedInode {
            mode,
            size,
            union_value,
            xattr_icount,
            is_extended: true,
        })
    }
}

// ---------------------------------------------------------------------------
// Xattr parsing
// ---------------------------------------------------------------------------

/// Extract the composefs digest from inline xattrs, if present.
///
/// Looks for `trusted.overlay.redirect` (name_index=4, suffix="overlay.redirect")
/// and converts its CAS-path value (e.g. `"ab/cdef1234..."`) back to a 32-byte
/// digest.
fn extract_digest<R: Read + Seek>(
    reader: &mut R,
    inode_offset: u64,
    inode: &ParsedInode,
) -> io::Result<Option<[u8; 32]>> {
    if inode.xattr_icount == 0 {
        return Ok(None);
    }

    let ibody_offset = inode_offset + inode.inode_size();
    let ibody_size = inode.xattr_ibody_size() as usize;

    reader.seek(SeekFrom::Start(ibody_offset))?;
    let mut ibody = vec![0u8; ibody_size];
    reader.read_exact(&mut ibody)?;

    // Skip the 12-byte header.
    let mut pos = 12;
    while pos + 4 <= ibody.len() {
        let e_name_len = ibody[pos] as usize;
        let e_name_index = ibody[pos + 1];
        let e_value_size = u16::from_le_bytes(ibody[pos + 2..pos + 4].try_into().unwrap()) as usize;

        let name_start = pos + 4;
        let name_end = name_start + e_name_len;
        let value_start = name_end;
        let value_end = value_start + e_value_size;

        if value_end > ibody.len() {
            break;
        }

        // Check for trusted.overlay.redirect.
        if e_name_index == EROFS_XATTR_INDEX_TRUSTED
            && &ibody[name_start..name_end] == b"overlay.redirect"
        {
            let cas_path = std::str::from_utf8(&ibody[value_start..value_end]).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 CAS path in xattr")
            })?;
            if let Some(digest) = cas_path_to_digest(cas_path) {
                return Ok(Some(digest));
            }
        }

        // Advance to next entry (4-byte aligned).
        let unaligned = 4 + e_name_len + e_value_size;
        let aligned = (unaligned + 3) & !3;
        pos += aligned;
    }

    Ok(None)
}

/// Convert a CAS path like `"ab/cdef0123..."` back to a 32-byte digest.
fn cas_path_to_digest(path: &str) -> Option<[u8; 32]> {
    // Remove the "/" to get the full 64-char hex string.
    let hex: String = path.chars().filter(|&c| c != '/').collect();
    if hex.len() != 64 {
        return None;
    }
    let mut digest = [0u8; 32];
    for i in 0..32 {
        digest[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(digest)
}

// ---------------------------------------------------------------------------
// Directory walking
// ---------------------------------------------------------------------------

/// Recursively walk a directory, collecting file info.
fn walk_directory<R: Read + Seek>(
    reader: &mut R,
    dir_nid: u64,
    prefix: String,
    meta_blkaddr: u32,
    block_size: u32,
    files: &mut Vec<FileInfo>,
) -> io::Result<()> {
    let inode = read_inode(reader, dir_nid, meta_blkaddr, block_size)?;

    // The root directory itself is added as a directory entry.
    let dir_path = prefix.clone();
    files.push(FileInfo {
        path: dir_path.clone(),
        file_type: FileType::Directory,
        mode: inode.mode,
        size: inode.size,
        digest: None,
    });

    if inode.size == 0 {
        // Empty directory -- no dirent blocks.
        return Ok(());
    }

    // Read dirent blocks. The directory data starts at block address `union_value`.
    let dir_data_start = u64::from(inode.union_value) * u64::from(block_size);
    let num_blocks = inode.size.div_ceil(u64::from(block_size));

    for blk_idx in 0..num_blocks {
        let blk_offset = dir_data_start + blk_idx * u64::from(block_size);
        reader.seek(SeekFrom::Start(blk_offset))?;
        let mut block = vec![0u8; block_size as usize];
        reader.read_exact(&mut block)?;

        // Parse dirents from this block.
        parse_dirent_block(reader, &block, &dir_path, meta_blkaddr, block_size, files)?;
    }

    Ok(())
}

/// Parse all directory entries from a single block and process each child.
fn parse_dirent_block<R: Read + Seek>(
    reader: &mut R,
    block: &[u8],
    parent_path: &str,
    meta_blkaddr: u32,
    block_size: u32,
    files: &mut Vec<FileInfo>,
) -> io::Result<()> {
    // First, determine how many dirents there are. The first dirent's nameoff
    // tells us the total size of the dirent header region.
    if block.len() < 12 {
        return Ok(());
    }
    let first_nameoff = u16::from_le_bytes(block[8..10].try_into().unwrap()) as usize;
    if first_nameoff == 0 || !first_nameoff.is_multiple_of(12) {
        return Ok(());
    }
    let dirent_count = first_nameoff / 12;

    // Collect all dirents.
    struct RawDirent {
        nid: u64,
        nameoff: usize,
        file_type: u8,
    }

    let mut dirents = Vec::with_capacity(dirent_count);
    for i in 0..dirent_count {
        let base = i * 12;
        if base + 12 > block.len() {
            break;
        }
        let nid = u64::from_le_bytes(block[base..base + 8].try_into().unwrap());
        let nameoff = u16::from_le_bytes(block[base + 8..base + 10].try_into().unwrap()) as usize;
        let ft = block[base + 10];
        dirents.push(RawDirent {
            nid,
            nameoff,
            file_type: ft,
        });
    }

    // Extract names. The name for entry i runs from nameoff[i] to nameoff[i+1]
    // (or to the end of meaningful data for the last entry).
    for (i, de) in dirents.iter().enumerate() {
        let name_start = de.nameoff;
        let name_end = if i + 1 < dirents.len() {
            dirents[i + 1].nameoff
        } else {
            // Last entry: scan forward until we hit a zero byte or end of block.
            let mut end = name_start;
            while end < block.len() && block[end] != 0 {
                end += 1;
            }
            end
        };

        if name_start >= block.len() || name_end > block.len() || name_end <= name_start {
            continue;
        }

        let name = std::str::from_utf8(&block[name_start..name_end]).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 filename in dirent")
        })?;

        let child_path = if parent_path.is_empty() {
            name.to_string()
        } else {
            format!("{parent_path}/{name}")
        };

        match de.file_type {
            EROFS_FT_DIR => {
                walk_directory(reader, de.nid, child_path, meta_blkaddr, block_size, files)?;
            }
            EROFS_FT_REG_FILE => {
                let child_inode = read_inode(reader, de.nid, meta_blkaddr, block_size)?;
                let inode_offset = u64::from(meta_blkaddr) * u64::from(block_size) + de.nid * 32;
                let digest = extract_digest(reader, inode_offset, &child_inode)?;

                files.push(FileInfo {
                    path: child_path,
                    file_type: FileType::Regular,
                    mode: child_inode.mode,
                    size: child_inode.size,
                    digest,
                });
            }
            EROFS_FT_SYMLINK => {
                let child_inode = read_inode(reader, de.nid, meta_blkaddr, block_size)?;
                files.push(FileInfo {
                    path: child_path,
                    file_type: FileType::Symlink,
                    mode: child_inode.mode,
                    size: child_inode.size,
                    digest: None,
                });
            }
            _ => {
                // Unknown file type -- skip.
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::ErofsBuilder;
    use crate::inode::{S_IFDIR, S_IFLNK, S_IFREG};
    use std::io::Cursor;

    #[test]
    fn verify_empty_image() {
        let builder = ErofsBuilder::new();
        let mut buf = Cursor::new(Vec::new());
        let _ = builder.build(&mut buf).unwrap();

        let mut cursor = Cursor::new(buf.into_inner());
        let info = verify_image(&mut cursor).unwrap();

        assert_eq!(info.block_size, 4096);
        assert_eq!(info.inode_count, 1, "empty image has 1 inode (root dir)");
        assert_eq!(info.root_nid, 0);

        // Only the root directory.
        assert_eq!(info.files.len(), 1);
        assert_eq!(info.files[0].file_type, FileType::Directory);
        assert_eq!(info.files[0].path, "");
        assert_eq!(info.files[0].mode & 0o170000, S_IFDIR);
    }

    #[test]
    fn verify_single_file() {
        let mut builder = ErofsBuilder::new();
        let digest = [0xAB; 32];
        builder
            .add_file("/hello.txt", &digest, 1024, 0o644, 1000, 1000)
            .unwrap();

        let mut buf = Cursor::new(Vec::new());
        let _ = builder.build(&mut buf).unwrap();

        let mut cursor = Cursor::new(buf.into_inner());
        let info = verify_image(&mut cursor).unwrap();

        assert_eq!(info.inode_count, 2);

        // Should have: root dir + hello.txt.
        let dirs: Vec<_> = info
            .files
            .iter()
            .filter(|f| f.file_type == FileType::Directory)
            .collect();
        let regulars: Vec<_> = info
            .files
            .iter()
            .filter(|f| f.file_type == FileType::Regular)
            .collect();

        assert_eq!(dirs.len(), 1);
        assert_eq!(regulars.len(), 1);
        assert_eq!(regulars[0].path, "hello.txt");
        assert_eq!(regulars[0].size, 1024);
        assert_eq!(regulars[0].mode & 0o170000, S_IFREG);

        // Verify digest was extracted from xattr.
        assert_eq!(regulars[0].digest, Some(digest));
    }

    #[test]
    fn verify_directory_hierarchy() {
        let mut builder = ErofsBuilder::new();
        let d1 = [0x01; 32];
        let d2 = [0x02; 32];
        builder
            .add_file("/usr/bin/foo", &d1, 4096, 0o755, 0, 0)
            .unwrap();
        builder
            .add_file("/usr/lib/bar", &d2, 8192, 0o644, 0, 0)
            .unwrap();

        let mut buf = Cursor::new(Vec::new());
        let _ = builder.build(&mut buf).unwrap();

        let mut cursor = Cursor::new(buf.into_inner());
        let info = verify_image(&mut cursor).unwrap();

        assert_eq!(info.inode_count, 6);

        // Collect paths.
        let mut paths: Vec<String> = info.files.iter().map(|f| f.path.clone()).collect();
        paths.sort();

        // Expected: "" (root), "usr", "usr/bin", "usr/bin/foo", "usr/lib", "usr/lib/bar"
        assert!(paths.contains(&String::new()), "root directory missing");
        assert!(paths.contains(&"usr".to_string()));
        assert!(paths.contains(&"usr/bin".to_string()));
        assert!(paths.contains(&"usr/bin/foo".to_string()));
        assert!(paths.contains(&"usr/lib".to_string()));
        assert!(paths.contains(&"usr/lib/bar".to_string()));

        // Verify digests on files.
        let foo = info.files.iter().find(|f| f.path == "usr/bin/foo").unwrap();
        assert_eq!(foo.digest, Some(d1));
        assert_eq!(foo.file_type, FileType::Regular);

        let bar = info.files.iter().find(|f| f.path == "usr/lib/bar").unwrap();
        assert_eq!(bar.digest, Some(d2));
    }

    #[test]
    fn verify_symlink() {
        let mut builder = ErofsBuilder::new();
        builder
            .add_symlink("/usr/bin/python", "python3", 0o777)
            .unwrap();

        let mut buf = Cursor::new(Vec::new());
        let _ = builder.build(&mut buf).unwrap();

        let mut cursor = Cursor::new(buf.into_inner());
        let info = verify_image(&mut cursor).unwrap();

        let symlinks: Vec<_> = info
            .files
            .iter()
            .filter(|f| f.file_type == FileType::Symlink)
            .collect();

        assert_eq!(symlinks.len(), 1);
        assert_eq!(symlinks[0].path, "usr/bin/python");
        assert_eq!(symlinks[0].mode & 0o170000, S_IFLNK);
        assert_eq!(symlinks[0].size, 7, "symlink size should be target length");
        assert_eq!(symlinks[0].digest, None);
    }

    #[test]
    fn corrupt_magic_fails() {
        let builder = ErofsBuilder::new();
        let mut buf = Cursor::new(Vec::new());
        let _ = builder.build(&mut buf).unwrap();

        let mut image = buf.into_inner();
        // Corrupt magic at offset 1024.
        image[1024] = 0xFF;
        image[1025] = 0xFF;
        image[1026] = 0xFF;
        image[1027] = 0xFF;

        let mut cursor = Cursor::new(image);
        let result = verify_image(&mut cursor);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("magic"),
            "error should mention magic: {err}"
        );
    }

    #[test]
    fn corrupt_checksum_fails() {
        let builder = ErofsBuilder::new();
        let mut buf = Cursor::new(Vec::new());
        let _ = builder.build(&mut buf).unwrap();

        let mut image = buf.into_inner();
        // Corrupt checksum at offset 1024 + 4.
        image[1028] ^= 0xFF;

        let mut cursor = Cursor::new(image);
        let result = verify_image(&mut cursor);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("checksum"),
            "error should mention checksum: {err}"
        );
    }

    #[test]
    fn cas_path_to_digest_round_trip() {
        let original = [0xAB; 32];
        // Simulate the CAS path format: "ab/abab...ab" (first two hex chars / rest).
        let hex: String = original.iter().map(|b| format!("{b:02x}")).collect();
        let cas_path = format!("{}/{}", &hex[..2], &hex[2..]);

        let recovered = cas_path_to_digest(&cas_path).unwrap();
        assert_eq!(recovered, original);
    }

    #[test]
    fn cas_path_to_digest_invalid_length() {
        assert!(cas_path_to_digest("ab/cd").is_none());
        assert!(cas_path_to_digest("").is_none());
    }

    #[test]
    fn verify_mixed_content() {
        let mut builder = ErofsBuilder::new();
        builder.add_directory("/etc", 0o755, 0, 0).unwrap();
        builder
            .add_file("/etc/hosts", &[0x11; 32], 256, 0o644, 0, 0)
            .unwrap();
        builder
            .add_symlink("/etc/localtime", "/usr/share/zoneinfo/UTC", 0o777)
            .unwrap();

        let mut buf = Cursor::new(Vec::new());
        let _ = builder.build(&mut buf).unwrap();

        let mut cursor = Cursor::new(buf.into_inner());
        let info = verify_image(&mut cursor).unwrap();

        assert_eq!(info.inode_count, 4); // root + etc + hosts + localtime

        let dirs: Vec<_> = info
            .files
            .iter()
            .filter(|f| f.file_type == FileType::Directory)
            .collect();
        let regulars: Vec<_> = info
            .files
            .iter()
            .filter(|f| f.file_type == FileType::Regular)
            .collect();
        let symlinks: Vec<_> = info
            .files
            .iter()
            .filter(|f| f.file_type == FileType::Symlink)
            .collect();

        assert_eq!(dirs.len(), 2); // root + etc
        assert_eq!(regulars.len(), 1);
        assert_eq!(symlinks.len(), 1);
        assert_eq!(regulars[0].path, "etc/hosts");
        assert_eq!(symlinks[0].path, "etc/localtime");
    }
}
