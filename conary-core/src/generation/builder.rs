// conary-core/src/generation/builder.rs

//! Generation builder — creates EROFS images from system state.
//!
//! This module provides two levels of API:
//!
//! - [`build_erofs_image`]: Low-level function that takes slices of
//!   [`FileEntryRef`] and [`SymlinkEntryRef`] and produces an EROFS image
//!   at the given path. Uses composefs-rs for image building.
//!
//! - [`build_generation_from_db`]: Higher-level function that queries the
//!   database for installed troves and their files, creates a state
//!   snapshot, and builds a complete generation directory with EROFS
//!   image and metadata JSON.

use std::path::{Path, PathBuf};

use tracing::{debug, info};

use crate::db::models::{FileEntry, StateEngine, SystemState, Trove};
use crate::generation::metadata::{EROFS_IMAGE_NAME, GENERATION_FORMAT, GenerationMetadata};
#[cfg(feature = "composefs-rs")]
use crate::generation::metadata::{ROOT_SYMLINKS, is_excluded};

/// A lightweight view of a file entry for EROFS building.
///
/// Decoupled from the database model so callers can construct entries
/// from any source (DB, changeset diff, test fixtures).
#[derive(Debug, Clone)]
pub struct FileEntryRef {
    /// Absolute path (e.g., `/usr/bin/conary`)
    pub path: String,
    /// 64-character hex-encoded SHA-256 digest
    pub sha256_hash: String,
    /// File size in bytes
    pub size: u64,
    /// Unix permission bits (e.g., 0o755)
    pub permissions: u32,
}

/// A symbolic link entry for EROFS building.
///
/// Decoupled from the database model so callers can construct entries
/// from any source (derivation output, test fixtures, etc.).
#[derive(Debug, Clone)]
pub struct SymlinkEntryRef {
    /// Absolute path of the symlink (e.g., `/usr/lib/libfoo.so.1`)
    pub path: String,
    /// The symlink target (e.g., `libfoo.so`)
    pub target: String,
}

/// Result of building an EROFS image.
#[derive(Debug, Clone)]
pub struct BuildResult {
    /// Path to the generated EROFS image file
    pub image_path: PathBuf,
    /// Size of the EROFS image in bytes
    pub image_size: u64,
    /// Number of CAS objects (external file references) in the image
    pub cas_objects_referenced: u64,
}

/// Convert a 64-character hex string to a 32-byte digest array.
///
/// Returns an error if the string is not exactly 64 hex characters.
pub fn hex_to_digest(hex: &str) -> crate::Result<[u8; 32]> {
    if hex.len() != 64 {
        return Err(crate::error::Error::ParseError(format!(
            "Expected 64-char hex digest, got {} chars",
            hex.len()
        )));
    }
    let mut digest = [0u8; 32];
    for i in 0..32 {
        digest[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| {
            crate::error::Error::ParseError(format!("Invalid hex at position {}", i * 2))
        })?;
    }
    Ok(digest)
}

/// Build an EROFS image from file entries and symlinks.
///
/// The image is written to `generation_dir/root.erofs`. Entries whose paths
/// match [`is_excluded`] are silently skipped. Standard root symlinks
/// (bin -> usr/bin, etc.) are always added. Package symlinks from the
/// `symlinks` slice are inserted using the same `LeafContent::Symlink` path.
///
/// Requires the `composefs-rs` feature. Without it, this function is a
/// compile-time stub that returns an error.
#[cfg(feature = "composefs-rs")]
pub fn build_erofs_image(
    entries: &[FileEntryRef],
    symlinks: &[SymlinkEntryRef],
    generation_dir: &Path,
) -> crate::Result<BuildResult> {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::rc::Rc;

    use composefs::erofs::writer::mkfs_erofs;
    use composefs::fsverity::{FsVerityHashValue, Sha256HashValue};
    use composefs::tree::{Directory, FileSystem, Inode, Leaf, LeafContent, RegularFile, Stat};

    /// Helper: create a Stat with default root ownership and the given mode.
    fn dir_stat(mode: u32) -> Stat {
        Stat {
            st_mode: mode,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 0,
            xattrs: RefCell::new(BTreeMap::new()),
        }
    }

    /// Ensure all parent directories for `abs_path` exist in the filesystem tree.
    ///
    /// Returns `(dir_path_string, leaf_name)` where `dir_path_string` is the
    /// cumulative parent path (empty for root-level entries) and `leaf_name`
    /// is the final path component.
    fn ensure_parent_dirs(
        root: &mut Directory<Sha256HashValue>,
        abs_path: &str,
    ) -> crate::Result<(String, String)> {
        let path = abs_path.strip_prefix('/').unwrap_or(abs_path);
        let components: Vec<&str> = path.split('/').collect();

        if components.is_empty() {
            return Err(crate::error::Error::ParseError(
                "empty path components".to_string(),
            ));
        }

        let dir_components = &components[..components.len() - 1];
        let leaf_name = components[components.len() - 1].to_string();

        let mut dir_path = String::new();
        for comp in dir_components {
            if dir_path.is_empty() {
                root.merge(
                    OsStr::new(comp),
                    Inode::Directory(Box::new(Directory::new(dir_stat(0o755)))),
                );
            } else {
                let parent = root.get_directory_mut(OsStr::new(&dir_path)).map_err(|e| {
                    crate::error::Error::InternalError(format!(
                        "Failed to navigate to '{}' for path {}: {e}",
                        dir_path, abs_path
                    ))
                })?;
                parent.merge(
                    OsStr::new(comp),
                    Inode::Directory(Box::new(Directory::new(dir_stat(0o755)))),
                );
            }

            if !dir_path.is_empty() {
                dir_path.push('/');
            }
            dir_path.push_str(comp);
        }

        Ok((dir_path, leaf_name))
    }

    /// Navigate to the parent directory described by `dir_path` and return a
    /// mutable reference to it. Returns the root if `dir_path` is empty.
    fn get_parent_dir<'a>(
        root: &'a mut Directory<Sha256HashValue>,
        dir_path: &str,
        abs_path: &str,
    ) -> crate::Result<&'a mut Directory<Sha256HashValue>> {
        if dir_path.is_empty() {
            Ok(root)
        } else {
            root.get_directory_mut(OsStr::new(dir_path)).map_err(|e| {
                crate::error::Error::InternalError(format!(
                    "Failed to navigate to parent '{}' for path {}: {e}",
                    dir_path, abs_path
                ))
            })
        }
    }

    let mut fs = FileSystem::<Sha256HashValue>::default();
    fs.set_root_stat(dir_stat(0o755));

    let mut cas_objects: u64 = 0;

    for entry in entries {
        if is_excluded(&entry.path) {
            continue;
        }

        // Parse the hex digest; skip files with invalid hashes (e.g.,
        // directories or adopted files with placeholder hashes).
        let hash = match Sha256HashValue::from_hex(&entry.sha256_hash) {
            Ok(h) => h,
            Err(_) => {
                debug!(
                    "Skipping file with invalid digest ({} chars): {}",
                    entry.sha256_hash.len(),
                    entry.path
                );
                continue;
            }
        };

        let (dir_path, file_name) = ensure_parent_dirs(&mut fs.root, &entry.path)?;
        let parent_dir = get_parent_dir(&mut fs.root, &dir_path, &entry.path)?;

        // Add the file as an External CAS reference
        parent_dir.insert(
            OsStr::new(&file_name),
            Inode::Leaf(Rc::new(Leaf {
                content: LeafContent::Regular(RegularFile::External(hash, entry.size)),
                stat: dir_stat(entry.permissions),
            })),
        );

        cas_objects += 1;
    }

    // Insert package symlinks
    for symlink in symlinks {
        if is_excluded(&symlink.path) {
            continue;
        }

        let (dir_path, link_name) = ensure_parent_dirs(&mut fs.root, &symlink.path)?;
        let parent_dir = get_parent_dir(&mut fs.root, &dir_path, &symlink.path)?;

        parent_dir.insert(
            OsStr::new(&link_name),
            Inode::Leaf(Rc::new(Leaf {
                content: LeafContent::Symlink(OsStr::new(&symlink.target).into()),
                stat: dir_stat(0o777),
            })),
        );
    }

    // Add root-level symlinks (bin -> usr/bin, lib -> usr/lib, etc.)
    for (link, target) in ROOT_SYMLINKS {
        fs.root.insert(
            OsStr::new(link),
            Inode::Leaf(Rc::new(Leaf {
                content: LeafContent::Symlink(OsStr::new(target).into()),
                stat: dir_stat(0o777),
            })),
        );
    }

    // Build the EROFS image
    let image_bytes = mkfs_erofs(&fs);
    let image_size = image_bytes.len() as u64;

    // Write EROFS image atomically: temp file -> fsync -> rename -> fsync parent.
    // This prevents partial writes from surviving a crash during recovery.
    let image_path = generation_dir.join(EROFS_IMAGE_NAME);
    let tmp_path = generation_dir.join(format!(".{EROFS_IMAGE_NAME}.tmp"));
    {
        use std::io::Write;
        let mut tmp_file = std::fs::File::create(&tmp_path).map_err(|e| {
            crate::error::Error::IoError(format!(
                "Failed to create temp EROFS image at {}: {e}",
                tmp_path.display()
            ))
        })?;
        tmp_file.write_all(&image_bytes).map_err(|e| {
            crate::error::Error::IoError(format!(
                "Failed to write EROFS image to {}: {e}",
                tmp_path.display()
            ))
        })?;
        tmp_file.sync_all().map_err(|e| {
            crate::error::Error::IoError(format!(
                "Failed to fsync EROFS image {}: {e}",
                tmp_path.display()
            ))
        })?;
    }
    std::fs::rename(&tmp_path, &image_path).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to rename temp EROFS image to {}: {e}",
            image_path.display()
        ))
    })?;
    // fsync the parent directory so the rename is durable
    if let Ok(parent) = std::fs::File::open(generation_dir) {
        let _ = parent.sync_all();
    }

    info!(
        "EROFS image built: {} bytes, {} CAS objects",
        image_size, cas_objects
    );

    Ok(BuildResult {
        image_path,
        image_size,
        cas_objects_referenced: cas_objects,
    })
}

/// Stub for when composefs-rs feature is not enabled.
#[cfg(not(feature = "composefs-rs"))]
pub fn build_erofs_image(
    _entries: &[FileEntryRef],
    _symlinks: &[SymlinkEntryRef],
    _generation_dir: &Path,
) -> crate::Result<BuildResult> {
    Err(crate::error::Error::NotImplemented(
        "EROFS image building requires the 'composefs-rs' feature".to_string(),
    ))
}

/// Build a complete generation from the current database state.
///
/// This is the high-level entry point that:
/// 1. Queries all installed troves and their file entries
/// 2. Builds the EROFS image via [`build_erofs_image`]
/// 3. Creates a system state snapshot (only after successful image build)
/// 4. Writes generation metadata JSON
///
/// The state snapshot is deliberately created *after* the EROFS image build
/// succeeds. Creating it before would leave an orphaned DB state record if
/// the image build fails.
///
/// Returns `(generation_number, BuildResult)`.
pub fn build_generation_from_db(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    summary: &str,
) -> crate::Result<(i64, BuildResult)> {
    // Step 1: Ensure generations base directory exists
    std::fs::create_dir_all(generations_root).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to create generations directory {}: {e}",
            generations_root.display()
        ))
    })?;

    // Step 2: Reserve the generation number and create the directory
    let gen_number = SystemState::next_state_number(conn).map_err(|e| {
        crate::error::Error::InternalError(format!("Failed to determine next state number: {e}"))
    })?;
    let gen_dir = generations_root.join(gen_number.to_string());
    if gen_dir.exists() {
        return Err(crate::error::Error::AlreadyExists(format!(
            "Generation directory already exists: {}",
            gen_dir.display()
        )));
    }
    std::fs::create_dir_all(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to create generation directory {}: {e}",
            gen_dir.display()
        ))
    })?;

    // Step 3: Collect file entries from all installed troves (single bulk query)
    let troves = Trove::list_all(conn)?;
    let all_files = FileEntry::find_all_ordered(conn)?;

    let file_refs: Vec<FileEntryRef> = all_files
        .iter()
        .map(|file| {
            #[allow(clippy::cast_sign_loss)]
            let permissions = file.permissions as u32;
            #[allow(clippy::cast_sign_loss)]
            let size = file.size as u64;

            FileEntryRef {
                path: file.path.clone(),
                sha256_hash: file.sha256_hash.clone(),
                size,
                permissions,
            }
        })
        .collect();

    // Step 4: Build EROFS image with symlinks from DB.
    // This must succeed before we commit state to the database.
    let symlink_refs = collect_symlink_refs(conn)?;
    let result = build_erofs_image(&file_refs, &symlink_refs, &gen_dir)?;

    // Step 5: Create system state snapshot at the reserved number -- only
    // after successful image build so we never leave orphaned state records
    // on build failure. Using create_snapshot_at() ensures the DB state
    // number matches the directory number we already created.
    let engine = StateEngine::new(conn);
    let _state = engine
        .create_snapshot_at(gen_number, summary, None, None)
        .map_err(|e| {
            crate::error::Error::InternalError(format!(
                "Failed to create system state snapshot: {e}"
            ))
        })?;

    // Step 6: Write generation metadata
    #[allow(clippy::cast_possible_wrap)]
    let metadata = GenerationMetadata {
        generation: gen_number,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(result.image_size as i64),
        cas_objects_referenced: Some(result.cas_objects_referenced as i64),
        fsverity_enabled: false, // Caller can enable separately
        erofs_verity_digest: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        package_count: troves.len() as i64,
        kernel_version: detect_kernel_version_from_troves(&troves),
        summary: summary.to_string(),
    };
    metadata.write_to(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!("Failed to write generation metadata: {e}"))
    })?;

    info!(
        "Generation {} built: {} CAS objects, {} packages, composefs format",
        gen_number,
        result.cas_objects_referenced,
        troves.len()
    );

    Ok((gen_number, result))
}

/// Rebuild the EROFS image for an existing generation without allocating a
/// new state number. Used by recovery to restore a generation that was already
/// recorded in the database.
///
/// Unlike [`build_generation_from_db`], this does NOT create a new system state
/// snapshot. It only rebuilds the EROFS image and metadata for the specified
/// generation number, using the current DB package state.
pub fn rebuild_generation_image(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    gen_number: i64,
    summary: &str,
) -> crate::Result<BuildResult> {
    let gen_dir = generations_root.join(gen_number.to_string());
    std::fs::create_dir_all(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to create generation directory {}: {e}",
            gen_dir.display()
        ))
    })?;

    let troves = Trove::list_all(conn)?;
    let all_files = FileEntry::find_all_ordered(conn)?;

    let file_refs: Vec<FileEntryRef> = all_files
        .iter()
        .map(|file| {
            #[allow(clippy::cast_sign_loss)]
            let permissions = file.permissions as u32;
            #[allow(clippy::cast_sign_loss)]
            let size = file.size as u64;
            FileEntryRef {
                path: file.path.clone(),
                sha256_hash: file.sha256_hash.clone(),
                size,
                permissions,
            }
        })
        .collect();

    let symlink_refs = collect_symlink_refs(conn)?;
    let result = build_erofs_image(&file_refs, &symlink_refs, &gen_dir)?;

    #[allow(clippy::cast_possible_wrap)]
    let metadata = GenerationMetadata {
        generation: gen_number,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(result.image_size as i64),
        cas_objects_referenced: Some(result.cas_objects_referenced as i64),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        package_count: troves.len() as i64,
        kernel_version: detect_kernel_version_from_troves(&troves),
        summary: summary.to_string(),
    };
    metadata.write_to(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!("Failed to write generation metadata: {e}"))
    })?;

    info!(
        "Generation {} rebuilt in place: {} CAS objects, {} packages",
        gen_number,
        result.cas_objects_referenced,
        troves.len()
    );

    Ok(result)
}

/// Collect symlink entries from all installed troves.
///
/// Queries file entries that have a non-NULL symlink_target and returns them
/// as `SymlinkEntryRef` values suitable for EROFS image building.
///
/// Returns an empty vec if the `file_entries` table does not have a
/// `symlink_target` column (older schema or test databases).
fn collect_symlink_refs(conn: &rusqlite::Connection) -> crate::Result<Vec<SymlinkEntryRef>> {
    let mut stmt = match conn.prepare(
        "SELECT path, symlink_target FROM files \
         WHERE symlink_target IS NOT NULL AND symlink_target != ''",
    ) {
        Ok(s) => s,
        Err(e) => {
            // Column may not exist in pre-v60 schemas.
            debug!("Skipping symlink collection: {e}");
            return Ok(Vec::new());
        }
    };

    let refs = stmt
        .query_map([], |row| {
            Ok(SymlinkEntryRef {
                path: row.get(0)?,
                target: row.get(1)?,
            })
        })
        .map_err(|e| crate::error::Error::InternalError(format!("Failed to query symlinks: {e}")))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(refs)
}

/// Get kernel version from an already-loaded trove list.
///
/// Looks for kernel-related packages in the trove list, falling back to
/// the running kernel version from `/proc/version`.
pub fn detect_kernel_version_from_troves(troves: &[Trove]) -> Option<String> {
    for trove in troves {
        if trove.name.starts_with("kernel") || trove.name.starts_with("linux-image") {
            return Some(trove.version.clone());
        }
    }
    // Fall back to running kernel
    std::fs::read_to_string("/proc/version")
        .ok()
        .and_then(|v| v.split_whitespace().nth(2).map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // hex_to_digest tests
    // ---------------------------------------------------------------

    #[test]
    fn hex_to_digest_valid() {
        let hex = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let digest = hex_to_digest(hex).unwrap();
        assert_eq!(digest[0], 0xab);
        assert_eq!(digest[1], 0xcd);
        assert_eq!(digest[31], 0x89);
    }

    #[test]
    fn hex_to_digest_wrong_length() {
        let result = hex_to_digest("abcd");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 64-char"));
    }

    #[test]
    fn hex_to_digest_invalid_chars() {
        let hex = "zzcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let result = hex_to_digest(hex);
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------
    // EROFS builder tests (composefs-rs feature required)
    // ---------------------------------------------------------------

    #[cfg(feature = "composefs-rs")]
    mod composefs_tests {
        use super::*;
        use tempfile::TempDir;

        #[test]
        fn build_erofs_from_file_entries() {
            let tmp = TempDir::new().unwrap();
            let entries = vec![
                FileEntryRef {
                    path: "/usr/bin/hello".to_string(),
                    sha256_hash: "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd"
                        .to_string(),
                    size: 1024,
                    permissions: 0o755,
                },
                FileEntryRef {
                    path: "/usr/lib/libfoo.so".to_string(),
                    sha256_hash: "1122334411223344112233441122334411223344112233441122334411223344"
                        .to_string(),
                    size: 4096,
                    permissions: 0o644,
                },
            ];

            let result = build_erofs_image(&entries, &[], tmp.path()).unwrap();

            assert!(result.image_path.exists(), "EROFS image file must exist");
            assert!(result.image_size > 0, "EROFS image must be non-empty");
            assert_eq!(
                result.cas_objects_referenced, 2,
                "Should reference 2 CAS objects"
            );

            // Verify EROFS magic at offset 1024
            let image_bytes = std::fs::read(&result.image_path).unwrap();
            let magic = u32::from_le_bytes([
                image_bytes[1024],
                image_bytes[1025],
                image_bytes[1026],
                image_bytes[1027],
            ]);
            assert_eq!(magic, 0xE0F5_E1E2, "EROFS magic mismatch");
        }

        #[test]
        fn excluded_paths_are_skipped() {
            let tmp = TempDir::new().unwrap();
            let entries = vec![
                FileEntryRef {
                    path: "/usr/bin/hello".to_string(),
                    sha256_hash: "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd"
                        .to_string(),
                    size: 1024,
                    permissions: 0o755,
                },
                FileEntryRef {
                    path: "/var/log/messages".to_string(),
                    sha256_hash: "1122334411223344112233441122334411223344112233441122334411223344"
                        .to_string(),
                    size: 2048,
                    permissions: 0o644,
                },
            ];

            let result = build_erofs_image(&entries, &[], tmp.path()).unwrap();

            assert_eq!(
                result.cas_objects_referenced, 1,
                "var/log entry should be excluded, leaving 1 CAS object"
            );
        }

        #[test]
        fn root_symlinks_are_added() {
            let tmp = TempDir::new().unwrap();
            let entries: Vec<FileEntryRef> = vec![];

            let result = build_erofs_image(&entries, &[], tmp.path()).unwrap();

            assert!(
                result.image_size > 0,
                "Image with only root dir + symlinks should be non-empty"
            );
            assert_eq!(
                result.cas_objects_referenced, 0,
                "No CAS objects with empty entries"
            );
        }

        #[test]
        fn deterministic_builds() {
            let tmp1 = TempDir::new().unwrap();
            let tmp2 = TempDir::new().unwrap();
            let entries = vec![
                FileEntryRef {
                    path: "/usr/bin/hello".to_string(),
                    sha256_hash: "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd"
                        .to_string(),
                    size: 1024,
                    permissions: 0o755,
                },
                FileEntryRef {
                    path: "/usr/lib/libfoo.so".to_string(),
                    sha256_hash: "1122334411223344112233441122334411223344112233441122334411223344"
                        .to_string(),
                    size: 4096,
                    permissions: 0o644,
                },
            ];

            let _r1 = build_erofs_image(&entries, &[], tmp1.path()).unwrap();
            let _r2 = build_erofs_image(&entries, &[], tmp2.path()).unwrap();

            let bytes1 = std::fs::read(tmp1.path().join(EROFS_IMAGE_NAME)).unwrap();
            let bytes2 = std::fs::read(tmp2.path().join(EROFS_IMAGE_NAME)).unwrap();

            assert_eq!(
                bytes1, bytes2,
                "Two builds with identical entries must produce identical images"
            );
        }
    }

    // ---------------------------------------------------------------
    // detect_kernel_version_from_troves does not panic
    // ---------------------------------------------------------------

    #[test]
    fn detect_kernel_version_does_not_panic() {
        // Test with an empty trove list
        // Just ensure it does not panic (returns None gracefully)
        let result = detect_kernel_version_from_troves(&[]);
        assert!(result.is_some() || result.is_none());
    }
}
