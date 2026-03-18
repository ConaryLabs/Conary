// conary-core/src/generation/composefs_rs_eval.rs

//! Evaluation of composefs-rs for EROFS image building.
//!
//! This module contains proof-of-concept tests to verify that
//! the composefs crate can produce CAS-reference-only EROFS images
//! suitable for Conary's generation-based deployment model.
//!
//! Evaluation criteria:
//! 1. CAS-reference-only images (External file references)
//! 2. Deterministic output (byte-identical on repeated builds)
//! 3. Bloom filter in superblock (xattr name filter)
//! 4. Dependency weight (new transitive deps, conflicts)
//! 5. API stability (core builder types)

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::rc::Rc;

    use composefs::erofs::writer::mkfs_erofs;
    use composefs::fsverity::{FsVerityHashValue, Sha256HashValue};
    use composefs::tree::{Directory, FileSystem, Inode, Leaf, LeafContent, RegularFile, Stat};

    /// Helper: create a default Stat with root ownership and 0o755 mode.
    fn default_stat() -> Stat {
        Stat {
            st_mode: 0o755,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 0,
            xattrs: RefCell::new(BTreeMap::new()),
        }
    }

    /// Helper: create a new FileSystem with a given root stat.
    fn new_fs(stat: Stat) -> FileSystem<Sha256HashValue> {
        let mut fs = FileSystem::<Sha256HashValue>::default();
        fs.set_root_stat(stat);
        fs
    }

    /// Helper: insert a leaf inode into a directory.
    fn add_leaf(
        dir: &mut Directory<Sha256HashValue>,
        name: &str,
        content: LeafContent<Sha256HashValue>,
    ) {
        dir.insert(
            OsStr::new(name),
            Inode::Leaf(Rc::new(Leaf {
                content,
                stat: Stat {
                    st_mode: 0o644,
                    st_uid: 0,
                    st_gid: 0,
                    st_mtim_sec: 0,
                    xattrs: RefCell::new(BTreeMap::new()),
                },
            })),
        );
    }

    /// Build a minimal filesystem tree with CAS-reference-only files.
    /// This models the Conary use case: all regular files are External
    /// references to content-addressable objects, not inline data.
    fn build_cas_only_tree() -> FileSystem<Sha256HashValue> {
        let mut fs = new_fs(default_stat());

        // Simulate two CAS-backed files with different hashes
        let hash_a = Sha256HashValue::from_hex(
            "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd",
        )
        .unwrap();
        let hash_b = Sha256HashValue::from_hex(
            "1122334411223344112233441122334411223344112233441122334411223344",
        )
        .unwrap();

        // /usr directory
        let usr_stat = Stat {
            st_mode: 0o755,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 1000,
            xattrs: RefCell::new(BTreeMap::new()),
        };
        let mut usr_dir = Directory::new(usr_stat);

        // /usr/bin directory
        let bin_stat = Stat {
            st_mode: 0o755,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 1000,
            xattrs: RefCell::new(BTreeMap::new()),
        };
        let mut bin_dir = Directory::new(bin_stat);

        // /usr/bin/conary -> External CAS reference (1.2 MB binary)
        add_leaf(
            &mut bin_dir,
            "conary",
            LeafContent::Regular(RegularFile::External(hash_a, 1_200_000)),
        );

        // /usr/bin/ls -> External CAS reference (150 KB binary)
        add_leaf(
            &mut bin_dir,
            "ls",
            LeafContent::Regular(RegularFile::External(hash_b, 150_000)),
        );

        usr_dir.insert(OsStr::new("bin"), Inode::Directory(Box::new(bin_dir)));

        // /usr/lib directory (empty for now)
        let lib_stat = Stat {
            st_mode: 0o755,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 1000,
            xattrs: RefCell::new(BTreeMap::new()),
        };
        usr_dir.insert(
            OsStr::new("lib"),
            Inode::Directory(Box::new(Directory::new(lib_stat))),
        );

        fs.root
            .insert(OsStr::new("usr"), Inode::Directory(Box::new(usr_dir)));

        // /etc -> symlink (not a CAS reference)
        add_leaf(
            &mut fs.root,
            "etc",
            LeafContent::Symlink(OsStr::new("/usr/share/conary/default.conf").into()),
        );

        fs
    }

    // ---------------------------------------------------------------
    // Test 1: CAS-reference-only EROFS image production
    // ---------------------------------------------------------------
    #[test]
    fn test_cas_reference_only_image() {
        let fs = build_cas_only_tree();
        let image = mkfs_erofs(&fs);

        // Image must be non-empty
        assert!(
            !image.is_empty(),
            "[FAIL] mkfs_erofs produced an empty image"
        );

        // EROFS magic (0xE0F5E1E2) at offset 1024 (after composefs header)
        assert!(
            image.len() > 1028,
            "[FAIL] Image too small to contain EROFS superblock"
        );
        let magic = u32::from_le_bytes([image[1024], image[1025], image[1026], image[1027]]);
        assert_eq!(
            magic, 0xE0F5_E1E2,
            "[FAIL] EROFS magic mismatch: got {magic:#010X}, expected 0xE0F5E1E2"
        );

        // Also check composefs header magic at offset 0
        let cfs_magic = u32::from_le_bytes([image[0], image[1], image[2], image[3]]);
        assert_eq!(
            cfs_magic, 0xd078_629a,
            "[FAIL] composefs header magic mismatch: got {cfs_magic:#010X}, expected 0xD078629A"
        );

        println!(
            "[PASS] CAS-reference-only EROFS image: {} bytes",
            image.len()
        );
    }

    // ---------------------------------------------------------------
    // Test 2: Deterministic output (byte-identical on repeated builds)
    // ---------------------------------------------------------------
    #[test]
    fn test_deterministic_output() {
        let fs1 = build_cas_only_tree();
        let image1 = mkfs_erofs(&fs1);

        let fs2 = build_cas_only_tree();
        let image2 = mkfs_erofs(&fs2);

        assert_eq!(
            image1.len(),
            image2.len(),
            "[FAIL] Image sizes differ: {} vs {}",
            image1.len(),
            image2.len()
        );
        assert_eq!(
            image1, image2,
            "[FAIL] Images are not byte-identical across two builds"
        );

        println!(
            "[PASS] Deterministic output: two builds produce identical {} byte images",
            image1.len()
        );
    }

    // ---------------------------------------------------------------
    // Test 3: Bloom filter (xattr name filter) in superblock
    // ---------------------------------------------------------------
    #[test]
    fn test_bloom_filter_feature_flag() {
        let fs = build_cas_only_tree();
        let image = mkfs_erofs(&fs);

        // feature_compat is at superblock offset +8 (after magic U32 and checksum U32)
        // Superblock starts at offset 1024
        let feature_compat_offset = 1024 + 8;
        assert!(
            image.len() > feature_compat_offset + 4,
            "[FAIL] Image too small to read feature_compat"
        );
        let feature_compat = u32::from_le_bytes([
            image[feature_compat_offset],
            image[feature_compat_offset + 1],
            image[feature_compat_offset + 2],
            image[feature_compat_offset + 3],
        ]);

        // FEATURE_COMPAT_XATTR_FILTER = 0x0000_0004
        let has_xattr_filter = (feature_compat & 0x0000_0004) != 0;
        // FEATURE_COMPAT_MTIME = 0x0000_0002
        let has_mtime = (feature_compat & 0x0000_0002) != 0;

        println!(
            "feature_compat = {feature_compat:#010X} (mtime={has_mtime}, xattr_filter={has_xattr_filter})"
        );

        assert!(
            has_xattr_filter,
            "[FAIL] FEATURE_COMPAT_XATTR_FILTER (bloom filter) not set in superblock"
        );
        assert!(
            has_mtime,
            "[FAIL] FEATURE_COMPAT_MTIME not set in superblock"
        );

        println!("[PASS] Bloom filter (xattr name filter) is enabled in superblock");
    }

    // ---------------------------------------------------------------
    // Test 4: Empty filesystem produces valid image
    // ---------------------------------------------------------------
    #[test]
    fn test_empty_filesystem() {
        let fs = new_fs(default_stat());
        let image = mkfs_erofs(&fs);

        assert!(
            !image.is_empty(),
            "[FAIL] Empty filesystem produced empty image"
        );

        let magic = u32::from_le_bytes([image[1024], image[1025], image[1026], image[1027]]);
        assert_eq!(
            magic, 0xE0F5_E1E2,
            "[FAIL] Empty filesystem EROFS magic mismatch"
        );

        println!("[PASS] Empty filesystem: valid {} byte image", image.len());
    }

    // ---------------------------------------------------------------
    // Test 5: Mixed content (inline + external + symlinks + dirs)
    // ---------------------------------------------------------------
    #[test]
    fn test_mixed_content_tree() {
        let mut fs = new_fs(default_stat());

        let hash = Sha256HashValue::from_hex(
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        )
        .unwrap();

        // External CAS reference
        add_leaf(
            &mut fs.root,
            "external_file",
            LeafContent::Regular(RegularFile::External(hash, 4096)),
        );

        // Inline file (small content stored directly in the EROFS image)
        add_leaf(
            &mut fs.root,
            "inline_file",
            LeafContent::Regular(RegularFile::Inline((*b"hello composefs").into())),
        );

        // Symlink
        add_leaf(
            &mut fs.root,
            "link",
            LeafContent::Symlink(OsStr::new("/target").into()),
        );

        // Character device
        add_leaf(&mut fs.root, "null", LeafContent::CharacterDevice(259));

        // FIFO
        add_leaf(&mut fs.root, "pipe", LeafContent::Fifo);

        // Nested directory
        let subdir = Directory::new(default_stat());
        fs.root
            .insert(OsStr::new("subdir"), Inode::Directory(Box::new(subdir)));

        let image = mkfs_erofs(&fs);

        let magic = u32::from_le_bytes([image[1024], image[1025], image[1026], image[1027]]);
        assert_eq!(
            magic, 0xE0F5_E1E2,
            "[FAIL] Mixed content EROFS magic mismatch"
        );

        println!(
            "[PASS] Mixed content tree: valid {} byte image with inline, external, symlink, chardev, fifo, dir",
            image.len()
        );
    }

    // ---------------------------------------------------------------
    // Test 6: External files get overlay metacopy xattrs
    // ---------------------------------------------------------------
    #[test]
    fn test_external_files_produce_overlay_xattrs() {
        // External files in composefs get trusted.overlay.metacopy and
        // trusted.overlay.redirect xattrs written by the EROFS writer.
        // We verify that the overlay redirect path (derived from the
        // fsverity hash) appears in the image bytes.
        let mut fs = new_fs(default_stat());
        let hash = Sha256HashValue::from_hex(
            "abcdef01abcdef01abcdef01abcdef01abcdef01abcdef01abcdef01abcdef01",
        )
        .unwrap();
        add_leaf(
            &mut fs.root,
            "file",
            LeafContent::Regular(RegularFile::External(hash.clone(), 1024)),
        );
        let image = mkfs_erofs(&fs);

        // The EROFS writer embeds the redirect path as
        // "/ab/cdef01abcdef01abcdef01abcdef01abcdef01abcdef01abcdef01abcdef01"
        // Check that the hash hex appears in the image bytes (from the
        // overlay.redirect xattr value).
        let redirect_path = hash.to_object_pathname();
        let redirect_bytes = redirect_path.as_bytes();

        let found = image
            .windows(redirect_bytes.len())
            .any(|w| w == redirect_bytes);

        assert!(
            found,
            "[FAIL] Overlay redirect path '{}' not found in EROFS image bytes",
            redirect_path
        );

        // Also verify the image is non-empty and has valid EROFS magic
        let magic = u32::from_le_bytes([image[1024], image[1025], image[1026], image[1027]]);
        assert_eq!(magic, 0xE0F5_E1E2, "[FAIL] EROFS magic mismatch");

        println!(
            "[PASS] External file overlay xattrs present: redirect='/{redirect_path}' found in {} byte image",
            image.len()
        );
    }

    // ---------------------------------------------------------------
    // Test 7: Large tree determinism (100 files)
    // ---------------------------------------------------------------
    #[test]
    fn test_large_tree_determinism() {
        fn build_large_tree() -> FileSystem<Sha256HashValue> {
            let mut fs = FileSystem::<Sha256HashValue>::default();
            fs.set_root_stat(Stat {
                st_mode: 0o755,
                st_uid: 0,
                st_gid: 0,
                st_mtim_sec: 0,
                xattrs: RefCell::new(BTreeMap::new()),
            });

            let usr_stat = Stat {
                st_mode: 0o755,
                st_uid: 0,
                st_gid: 0,
                st_mtim_sec: 1000,
                xattrs: RefCell::new(BTreeMap::new()),
            };
            let mut usr_dir = Directory::new(usr_stat);

            // Create 100 files with unique hashes
            for i in 0..100u32 {
                let hash_bytes = format!(
                    "{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
                    i,
                    i + 1,
                    i + 2,
                    i + 3,
                    i + 4,
                    i + 5,
                    i + 6,
                    i + 7
                );
                let hash = Sha256HashValue::from_hex(&hash_bytes).unwrap();
                let name = format!("file_{i:04}");
                add_leaf(
                    &mut usr_dir,
                    &name,
                    LeafContent::Regular(RegularFile::External(hash, (i as u64 + 1) * 1000)),
                );
            }

            fs.root
                .insert(OsStr::new("usr"), Inode::Directory(Box::new(usr_dir)));
            fs
        }

        let image1 = mkfs_erofs(&build_large_tree());
        let image2 = mkfs_erofs(&build_large_tree());

        assert_eq!(
            image1, image2,
            "[FAIL] Large tree (100 files) not deterministic"
        );

        println!(
            "[PASS] Large tree determinism: 100 files, {} byte image, byte-identical",
            image1.len()
        );
    }
}
