// src/commands/test_helpers.rs

//! Shared test fixtures for command handler tests.
//!
//! Both `model.rs` and `update.rs` use identical database setup and
//! replatform seeding logic. This module provides those fixtures once
//! so each test module can import them with `use super::test_helpers::*`.

use conary_core::db::models::{
    Changeset, ChangesetStatus, Component, DependencyEntry, FileEntry, InstallSource, LabelEntry,
    PackageResolution, PrimaryStrategy, ProvideEntry, Repository, RepositoryPackage,
    ResolutionStrategy, Trove, TroveType,
};
use conary_core::db::schema;
use tempfile::{NamedTempFile, TempDir};

pub(crate) fn create_test_db() -> (NamedTempFile, String) {
    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().display().to_string();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::migrate(&conn).unwrap();
    drop(conn);
    (temp_file, db_path)
}

pub(crate) fn setup_command_test_db() -> (TempDir, String) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir
        .path()
        .join("test.db")
        .to_str()
        .unwrap()
        .to_string();

    conary_core::db::init(&db_path).unwrap();
    stage_test_boot_assets(temp_dir.path());
    let cas = conary_core::filesystem::CasStore::new(temp_dir.path().join("objects")).unwrap();
    let nginx_binary = b"test nginx binary";
    let nginx_binary_hash = cas.store(nginx_binary).unwrap();
    let nginx_binary_size = i64::try_from(nginx_binary.len()).unwrap();
    let init_binary = b"test init binary";
    let init_binary_hash = cas.store(init_binary).unwrap();
    let init_binary_size = i64::try_from(init_binary.len()).unwrap();
    let nginx_config_contents = b"worker_processes 1;\n";
    let nginx_config_hash = cas.store(nginx_config_contents).unwrap();
    let nginx_config_size = i64::try_from(nginx_config_contents.len()).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset1 = Changeset::new("Install nginx-1.24.0".to_string());
        let changeset1_id = changeset1.insert(tx)?;

        let mut nginx = Trove::new(
            "nginx".to_string(),
            "1.24.0".to_string(),
            TroveType::Package,
        );
        nginx.architecture = Some("x86_64".to_string());
        nginx.description = Some("High performance web server".to_string());
        nginx.installed_by_changeset_id = Some(changeset1_id);
        let nginx_id = nginx.insert(tx)?;

        let mut nginx_runtime = Component::new(nginx_id, "runtime".to_string());
        let runtime_id = nginx_runtime.insert(tx)?;

        let mut nginx_config = Component::new(nginx_id, "config".to_string());
        let config_id = nginx_config.insert(tx)?;

        tx.execute(
            "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                &nginx_binary_hash,
                format!("objects/{}/{}", &nginx_binary_hash[0..2], &nginx_binary_hash[2..]),
                nginx_binary_size
            ],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                &nginx_config_hash,
                format!("objects/{}/{}", &nginx_config_hash[0..2], &nginx_config_hash[2..]),
                nginx_config_size
            ],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                &init_binary_hash,
                format!("objects/{}/{}", &init_binary_hash[0..2], &init_binary_hash[2..]),
                init_binary_size
            ],
        )?;

        let mut f1 = FileEntry::new(
            "/usr/sbin/nginx".to_string(),
            nginx_binary_hash.clone(),
            nginx_binary_size,
            0o755,
            nginx_id,
        );
        f1.component_id = Some(runtime_id);
        f1.insert(tx)?;

        let mut init = FileEntry::new(
            "/usr/sbin/init".to_string(),
            init_binary_hash.clone(),
            init_binary_size,
            0o755,
            nginx_id,
        );
        init.component_id = Some(runtime_id);
        init.insert(tx)?;

        let mut f2 = FileEntry::new(
            "/etc/nginx/nginx.conf".to_string(),
            nginx_config_hash.clone(),
            nginx_config_size,
            0o644,
            nginx_id,
        );
        f2.component_id = Some(config_id);
        f2.insert(tx)?;

        let mut p1 = ProvideEntry::new(nginx_id, "nginx".to_string(), Some("1.24.0".to_string()));
        p1.insert(tx)?;
        let mut p2 = ProvideEntry::new(nginx_id, "webserver".to_string(), None);
        p2.insert(tx)?;

        let mut dep = DependencyEntry::new(
            nginx_id,
            "openssl".to_string(),
            Some(">= 3.0".to_string()),
            "runtime".to_string(),
            None,
        );
        dep.insert(tx)?;

        changeset1.update_status(tx, ChangesetStatus::Applied)?;

        let mut changeset2 = Changeset::new("Install openssl-3.0.0".to_string());
        let changeset2_id = changeset2.insert(tx)?;

        let mut openssl = Trove::new(
            "openssl".to_string(),
            "3.0.0".to_string(),
            TroveType::Package,
        );
        openssl.architecture = Some("x86_64".to_string());
        openssl.description = Some("Cryptography and SSL/TLS toolkit".to_string());
        openssl.installed_by_changeset_id = Some(changeset2_id);
        let openssl_id = openssl.insert(tx)?;

        let mut openssl_runtime = Component::new(openssl_id, "runtime".to_string());
        openssl_runtime.insert(tx)?;

        let mut p3 =
            ProvideEntry::new(openssl_id, "openssl".to_string(), Some("3.0.0".to_string()));
        p3.insert(tx)?;
        let mut p4 = ProvideEntry::new(openssl_id, "soname(libssl.so.3)".to_string(), None);
        p4.insert(tx)?;

        changeset2.update_status(tx, ChangesetStatus::Applied)?;

        Ok(())
    })
    .unwrap();

    (temp_dir, db_path)
}

fn stage_test_boot_assets(root: &std::path::Path) {
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

pub(crate) fn seed_mixed_replatform_fixture(conn: &rusqlite::Connection) {
    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.default_strategy_distro = Some("fedora-43".to_string());
    let fedora_repo_id = fedora_repo.insert(conn).unwrap();

    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy = Some("legacy".to_string());
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(conn).unwrap();

    let mut fedora_label = LabelEntry::new(
        "fedora".to_string(),
        "f43".to_string(),
        "stable".to_string(),
    );
    fedora_label.insert(conn).unwrap();
    fedora_label
        .set_repository(conn, Some(fedora_repo_id))
        .unwrap();

    for (name, version) in [("vim", "9.0.1"), ("bash", "5.1.0"), ("zsh", "5.8.0")] {
        let mut trove = Trove::new_with_source(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.label_id = fedora_label.id;
        trove.insert(conn).unwrap();
    }

    for (name, version) in [("vim", "9.1.0"), ("bash", "5.2.0"), ("zsh", "5.9.1")] {
        let mut pkg = RepositoryPackage::new(
            arch_repo_id,
            name.to_string(),
            version.to_string(),
            format!("sha256:{name}"),
            123,
            format!("https://example.test/arch/{name}.pkg.tar.zst"),
        );
        pkg.architecture = Some("x86_64".to_string());
        pkg.insert(conn).unwrap();
    }

    let mut exact_resolution = PackageResolution::new(
        arch_repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/arch/vim-9.1.0.ccs".to_string(),
            checksum: "sha256:vim-exact".to_string(),
            delta_base: None,
        }],
    );
    exact_resolution.primary_strategy = PrimaryStrategy::Binary;
    exact_resolution.version = Some("9.1.0".to_string());
    exact_resolution.insert(conn).unwrap();

    let mut any_version_resolution = PackageResolution::new(
        arch_repo_id,
        "bash".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/arch/bash-latest.ccs".to_string(),
            checksum: "sha256:bash-any".to_string(),
            delta_base: None,
        }],
    );
    any_version_resolution.primary_strategy = PrimaryStrategy::Binary;
    any_version_resolution.insert(conn).unwrap();
}
