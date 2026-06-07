// src/commands/ccs/install/test_support.rs

pub(super) fn stage_test_boot_assets(root: &std::path::Path) {
    let kernel_version = conary_core::generation::builder::detect_kernel_version_from_troves(&[])
        .unwrap_or_else(|| "test-kernel".to_string());
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
        );
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

        let mut init = FileEntry::new(
            "/usr/sbin/init".to_string(),
            init_hash,
            init_size,
            0o755,
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
    use conary_core::ccs::{FileEntry, FileType};

    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = conary_core::hash::sha256(&init_content);
    (
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        init_content,
        init_hash,
    )
}
