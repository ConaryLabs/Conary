// conary-core/src/generation/builder/erofs.rs

use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::generation::metadata::EROFS_IMAGE_NAME;
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
    /// Owner username (e.g., `"root"`). Resolved to numeric UID at EROFS
    /// build time via `getpwnam_r`. Defaults to root (UID 0) when `None`.
    pub owner: Option<String>,
    /// Group name (e.g., `"wheel"`). Resolved to numeric GID at EROFS
    /// build time via `getgrnam_r`. Defaults to root (GID 0) when `None`.
    pub group_name: Option<String>,
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
    /// Hex-encoded fs-verity digest of the generated EROFS image
    pub erofs_verity_digest: Option<String>,
}

/// Resolve a username to numeric UID via libc `getpwnam_r` (reentrant).
/// Returns 0 (root) if the user does not exist on this system.
fn resolve_uid(owner: Option<&str>) -> u32 {
    let Some(name) = owner else { return 0 };
    if name == "root" || name.is_empty() {
        return 0;
    }
    let c_name = match std::ffi::CString::new(name) {
        Ok(s) => s,
        Err(_) => {
            warn!("Username '{}' contains null byte, defaulting to root", name);
            return 0;
        }
    };
    let mut buf = vec![0u8; 4096];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: getpwnam_r is a POSIX reentrant function. We pass valid pointers
    // to owned buffers that outlive the call. `result` is set to null or a
    // pointer into `pwd` on success.
    let rc = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            pwd.as_mut_ptr(),
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
            &mut result,
        )
    };
    if rc == 0 && !result.is_null() {
        // SAFETY: `result` points into our `pwd` buffer and the call succeeded.
        unsafe { (*result).pw_uid }
    } else {
        warn!("Unknown user '{}', defaulting to root", name);
        0
    }
}

/// Resolve a group name to numeric GID via libc `getgrnam_r` (reentrant).
/// Returns 0 (root) if the group does not exist on this system.
fn resolve_gid(group: Option<&str>) -> u32 {
    let Some(name) = group else { return 0 };
    if name == "root" || name.is_empty() {
        return 0;
    }
    let c_name = match std::ffi::CString::new(name) {
        Ok(s) => s,
        Err(_) => {
            warn!(
                "Group name '{}' contains null byte, defaulting to root",
                name
            );
            return 0;
        }
    };
    let mut buf = vec![0u8; 4096];
    let mut grp = std::mem::MaybeUninit::<libc::group>::uninit();
    let mut result: *mut libc::group = std::ptr::null_mut();
    // SAFETY: getgrnam_r is a POSIX reentrant function. We pass valid pointers
    // to owned buffers that outlive the call. `result` is set to null or a
    // pointer into `grp` on success.
    let rc = unsafe {
        libc::getgrnam_r(
            c_name.as_ptr(),
            grp.as_mut_ptr(),
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
            &mut result,
        )
    };
    if rc == 0 && !result.is_null() {
        // SAFETY: `result` points into our `grp` buffer and the call succeeded.
        unsafe { (*result).gr_gid }
    } else {
        warn!("Unknown group '{}', defaulting to root", name);
        0
    }
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
/// Owner and group names on [`FileEntryRef`] entries are resolved to numeric
/// uid/gid via `getpwnam_r`/`getgrnam_r` at build time. Unknown users or
/// groups default to root (0) with a warning. Synthetic directories (root,
/// `/usr`, symlinks) always use root:root ownership.
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

    fn dir_stat(mode: u32) -> Stat {
        Stat {
            st_mode: mode,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 0,
            xattrs: RefCell::new(BTreeMap::new()),
        }
    }

    fn file_stat(mode: u32, uid: u32, gid: u32) -> Stat {
        Stat {
            st_mode: mode,
            st_uid: uid,
            st_gid: gid,
            st_mtim_sec: 0,
            xattrs: RefCell::new(BTreeMap::new()),
        }
    }

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
        let uid = resolve_uid(entry.owner.as_deref());
        let gid = resolve_gid(entry.group_name.as_deref());

        parent_dir.insert(
            OsStr::new(&file_name),
            Inode::Leaf(Rc::new(Leaf {
                content: LeafContent::Regular(RegularFile::External(hash, entry.size)),
                stat: file_stat(entry.permissions, uid, gid),
            })),
        );

        cas_objects += 1;
    }

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

    for (link, target) in ROOT_SYMLINKS {
        fs.root.insert(
            OsStr::new(link),
            Inode::Leaf(Rc::new(Leaf {
                content: LeafContent::Symlink(OsStr::new(target).into()),
                stat: dir_stat(0o777),
            })),
        );
    }

    let image_bytes = mkfs_erofs(&fs);
    let image_size = image_bytes.len() as u64;
    let erofs_verity_digest =
        composefs::fsverity::compute_verity::<Sha256HashValue>(&image_bytes).to_hex();

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
        erofs_verity_digest: Some(erofs_verity_digest),
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn resolve_uid_none_returns_root() {
        assert_eq!(resolve_uid(None), 0);
    }

    #[test]
    fn resolve_uid_root_returns_zero() {
        assert_eq!(resolve_uid(Some("root")), 0);
    }

    #[test]
    fn resolve_uid_empty_returns_zero() {
        assert_eq!(resolve_uid(Some("")), 0);
    }

    #[test]
    fn resolve_gid_none_returns_root() {
        assert_eq!(resolve_gid(None), 0);
    }

    #[test]
    fn resolve_gid_root_returns_zero() {
        assert_eq!(resolve_gid(Some("root")), 0);
    }

    #[test]
    fn resolve_gid_empty_returns_zero() {
        assert_eq!(resolve_gid(Some("")), 0);
    }

    #[test]
    fn resolve_uid_unknown_returns_zero() {
        assert_eq!(resolve_uid(Some("conary_nonexistent_test_user_xyz")), 0);
    }

    #[test]
    fn resolve_gid_unknown_returns_zero() {
        assert_eq!(resolve_gid(Some("conary_nonexistent_test_group_xyz")), 0);
    }

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
                    owner: None,
                    group_name: None,
                },
                FileEntryRef {
                    path: "/usr/lib/libfoo.so".to_string(),
                    sha256_hash: "1122334411223344112233441122334411223344112233441122334411223344"
                        .to_string(),
                    size: 4096,
                    permissions: 0o644,
                    owner: None,
                    group_name: None,
                },
            ];

            let result = build_erofs_image(&entries, &[], tmp.path()).unwrap();

            assert!(result.image_path.exists(), "EROFS image file must exist");
            assert!(result.image_size > 0, "EROFS image must be non-empty");
            assert!(
                result.erofs_verity_digest.is_some(),
                "EROFS build should compute an fs-verity digest"
            );
            assert_eq!(result.cas_objects_referenced, 2);

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
                    owner: None,
                    group_name: None,
                },
                FileEntryRef {
                    path: "/var/log/messages".to_string(),
                    sha256_hash: "1122334411223344112233441122334411223344112233441122334411223344"
                        .to_string(),
                    size: 2048,
                    permissions: 0o644,
                    owner: None,
                    group_name: None,
                },
            ];

            let result = build_erofs_image(&entries, &[], tmp.path()).unwrap();
            assert_eq!(result.cas_objects_referenced, 1);
        }

        #[test]
        fn root_symlinks_are_added() {
            let tmp = TempDir::new().unwrap();
            let entries: Vec<FileEntryRef> = vec![];

            let result = build_erofs_image(&entries, &[], tmp.path()).unwrap();

            assert!(result.image_size > 0);
            assert_eq!(result.cas_objects_referenced, 0);
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
                    owner: None,
                    group_name: None,
                },
                FileEntryRef {
                    path: "/usr/lib/libfoo.so".to_string(),
                    sha256_hash: "1122334411223344112233441122334411223344112233441122334411223344"
                        .to_string(),
                    size: 4096,
                    permissions: 0o644,
                    owner: None,
                    group_name: None,
                },
            ];

            let _r1 = build_erofs_image(&entries, &[], tmp1.path()).unwrap();
            let _r2 = build_erofs_image(&entries, &[], tmp2.path()).unwrap();

            let bytes1 = std::fs::read(tmp1.path().join(EROFS_IMAGE_NAME)).unwrap();
            let bytes2 = std::fs::read(tmp2.path().join(EROFS_IMAGE_NAME)).unwrap();

            assert_eq!(bytes1, bytes2);
        }

        #[test]
        fn build_erofs_with_owner_group() {
            let tmp = TempDir::new().unwrap();
            let entries = vec![FileEntryRef {
                path: "/usr/bin/owned".to_string(),
                sha256_hash: "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd"
                    .to_string(),
                size: 512,
                permissions: 0o755,
                owner: Some("root".to_string()),
                group_name: Some("root".to_string()),
            }];

            let result = build_erofs_image(&entries, &[], tmp.path()).unwrap();
            assert_eq!(result.cas_objects_referenced, 1);
        }

        #[test]
        fn deterministic_builds_produce_same_verity_digest() {
            let tmp1 = TempDir::new().unwrap();
            let tmp2 = TempDir::new().unwrap();
            let entries = vec![FileEntryRef {
                path: "/usr/bin/hello".to_string(),
                sha256_hash: "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd"
                    .to_string(),
                size: 1024,
                permissions: 0o755,
                owner: None,
                group_name: None,
            }];

            let r1 = build_erofs_image(&entries, &[], tmp1.path()).unwrap();
            let r2 = build_erofs_image(&entries, &[], tmp2.path()).unwrap();

            assert_eq!(r1.erofs_verity_digest, r2.erofs_verity_digest);
        }
    }
}
