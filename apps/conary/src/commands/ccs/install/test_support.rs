// apps/conary/src/commands/ccs/install/test_support.rs

pub(super) fn write_signed_test_package(
    result: &conary_core::ccs::BuildResult,
    package_path: &std::path::Path,
) -> std::path::PathBuf {
    let signing_key =
        conary_core::ccs::signing::SigningKeyPair::generate().with_key_id("test-authority");
    conary_core::ccs::builder::write_signed_current_ccs_package(
        result,
        package_path,
        &signing_key,
        false,
    )
    .unwrap();
    let policy_path = package_path.with_extension("trust-policy.toml");
    std::fs::write(
        &policy_path,
        format!(
            "trusted_keys = [\"{}\"]\nrequire_timestamp = false\n",
            signing_key.public_key_base64()
        ),
    )
    .unwrap();
    policy_path
}

pub(super) fn ccs_regular_file(
    path: impl Into<String>,
    sha256: impl Into<String>,
    size: u64,
    mode: u32,
    component: impl Into<String>,
) -> conary_core::ccs::FileEntry {
    let mut node = conary_core::payload::PayloadNode::regular(mode & 0o7777);
    node.user = conary_core::payload::PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = conary_core::payload::PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    conary_core::ccs::FileEntry {
        path: path.into(),
        node,
        content: Some(conary_core::payload::PayloadContentAuthority {
            sha256: sha256.into(),
            size,
        }),
        component: component.into(),
        chunks: None,
    }
}

pub(super) fn ccs_symlink(
    path: impl Into<String>,
    target: impl Into<String>,
    mode: u32,
    component: impl Into<String>,
) -> conary_core::ccs::FileEntry {
    use conary_core::payload::{PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp};

    conary_core::ccs::FileEntry {
        path: path.into(),
        node: PayloadNode {
            kind: PayloadNodeKind::Symlink {
                target: target.into(),
            },
            mode: libc::S_IFLNK | (mode & 0o7777),
            user: PayloadIdentity::Numeric {
                id: u64::from(unsafe { libc::geteuid() }),
            },
            group: PayloadIdentity::Numeric {
                id: u64::from(unsafe { libc::getegid() }),
            },
            mtime: PayloadTimestamp::UNIX_EPOCH,
            xattrs: Default::default(),
        },
        content: None,
        component: component.into(),
        chunks: None,
    }
}

pub(super) fn stage_test_boot_assets(root: &std::path::Path) {
    let kernel_version = "test-kernel";
    let boot_root = root.join("boot");
    std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
    std::fs::write(
        boot_root.join(format!("vmlinuz-{kernel_version}")),
        b"test-kernel",
    )
    .unwrap();
    std::fs::write(
        boot_root.join(format!("initramfs-{kernel_version}.img")),
        b"test-initramfs",
    )
    .unwrap();
    std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"test-efi").unwrap();
}

pub(super) fn seed_test_init_trove(db_path: &str, db_dir: &std::path::Path) {
    use conary_core::db::models::{
        Changeset, ChangesetStatus, Component, FileEntry, ProvideEntry, Trove, TroveType,
    };
    use conary_core::payload::{
        PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
        ResolvedPayloadNode,
    };

    let cas = conary_core::filesystem::CasStore::new(db_dir.join("objects")).unwrap();
    let init_content = b"#!/bin/sh\nexec true\n";
    let init_hash = cas.store(init_content).unwrap();
    let init_size = i64::try_from(init_content.len()).unwrap();
    let mut conn = conary_core::db::open(db_path).unwrap();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install test-init-1.0.0".to_string());
        let changeset_id = changeset.insert(tx)?;

        let mut trove = Trove::new(
            "test-init".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary);
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        let mut component = Component::new(trove_id, "runtime".to_string());
        let component_id = component.insert(tx)?;

        tx.execute(
            "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                &init_hash,
                format!("objects/{}/{}", &init_hash[0..2], &init_hash[2..]),
                init_size
            ],
        )?;

        let current_identity = || {
            PayloadNode {
                kind: PayloadNodeKind::Directory,
                mode: libc::S_IFDIR | 0o755,
                user: PayloadIdentity::Numeric {
                    id: u64::from(unsafe { libc::geteuid() }),
                },
                group: PayloadIdentity::Numeric {
                    id: u64::from(unsafe { libc::getegid() }),
                },
                mtime: PayloadTimestamp::UNIX_EPOCH,
                xattrs: Default::default(),
            }
        };
        for path in ["/usr", "/usr/sbin"] {
            let mut directory = FileEntry::new(
                path.to_string(),
                ResolvedPayloadNode::from_numeric_source(current_identity()).unwrap(),
                None,
                trove_id,
            );
            directory.component_id = Some(component_id);
            directory.insert(tx)?;
        }

        let mut init_node = PayloadNode::regular(0o755);
        init_node.user = PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::geteuid() }),
        };
        init_node.group = PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::getegid() }),
        };
        let mut init = FileEntry::new(
            "/usr/sbin/init".to_string(),
            ResolvedPayloadNode::from_numeric_source(init_node).unwrap(),
            Some(PayloadContentAuthority {
                sha256: init_hash,
                size: init_content.len() as u64,
            }),
            trove_id,
        );
        init.component_id = Some(component_id);
        init.insert(tx)?;

        let mut provide = ProvideEntry::new(trove_id, "test-init".to_string(), Some("1.0.0".to_string()));
        provide.insert(tx)?;
        changeset.update_status(tx, ChangesetStatus::Applied)?;

        Ok(())
    })
    .unwrap();
}

pub(super) fn ccs_init_file() -> (conary_core::ccs::FileEntry, Vec<u8>, String) {
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = conary_core::hash::sha256(&init_content);
    (
        ccs_regular_file(
            "/usr/sbin/init",
            init_hash.clone(),
            init_content.len() as u64,
            0o100755,
            "runtime",
        ),
        init_content,
        init_hash,
    )
}
