// conary-core/src/generation/composefs_rs_eval.rs

//! Evaluation of composefs-rs for EROFS image building.
//!
//! Retains only the bloom filter (xattr name filter) superblock test.
//! The other evaluation tests (CAS-reference, determinism, empty, mixed,
//! large tree) duplicated coverage in the generation builder.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::rc::Rc;

    use composefs::erofs::writer::mkfs_erofs;
    use composefs::fsverity::Sha256HashValue;
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
    fn build_cas_only_tree() -> FileSystem<Sha256HashValue> {
        let mut fs = FileSystem::<Sha256HashValue>::default();
        fs.set_root_stat(default_stat());

        let hash_a = Sha256HashValue::from_hex(
            "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd",
        )
        .unwrap();
        let hash_b = Sha256HashValue::from_hex(
            "1122334411223344112233441122334411223344112233441122334411223344",
        )
        .unwrap();

        let usr_stat = Stat {
            st_mode: 0o755,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 1000,
            xattrs: RefCell::new(BTreeMap::new()),
        };
        let mut usr_dir = Directory::new(usr_stat);

        let bin_stat = Stat {
            st_mode: 0o755,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 1000,
            xattrs: RefCell::new(BTreeMap::new()),
        };
        let mut bin_dir = Directory::new(bin_stat);

        add_leaf(
            &mut bin_dir,
            "conary",
            LeafContent::Regular(RegularFile::External(hash_a, 1_200_000)),
        );
        add_leaf(
            &mut bin_dir,
            "ls",
            LeafContent::Regular(RegularFile::External(hash_b, 150_000)),
        );

        usr_dir.insert(OsStr::new("bin"), Inode::Directory(Box::new(bin_dir)));
        fs.root
            .insert(OsStr::new("usr"), Inode::Directory(Box::new(usr_dir)));

        fs
    }

    // ---------------------------------------------------------------
    // Bloom filter (xattr name filter) in EROFS superblock
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

        assert!(
            has_xattr_filter,
            "[FAIL] FEATURE_COMPAT_XATTR_FILTER (bloom filter) not set in superblock"
        );
        assert!(
            has_mtime,
            "[FAIL] FEATURE_COMPAT_MTIME not set in superblock"
        );
    }
}
