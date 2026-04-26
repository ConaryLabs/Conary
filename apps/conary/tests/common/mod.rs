// tests/common/mod.rs

//! Shared test utilities and helpers for integration tests.

use conary_core::db;
use conary_core::db::models::{
    Changeset, ChangesetStatus, Component, DependencyEntry, FileEntry, ProvideEntry, Trove,
    TroveType,
};
use tempfile::TempDir;

/// Create an empty test database with schema initialized.
///
/// Returns (TempDir, db_path, Connection) - keep the TempDir alive to prevent cleanup.
#[allow(dead_code)]
pub fn create_test_db() -> (TempDir, String, rusqlite::Connection) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir
        .path()
        .join("test.db")
        .to_str()
        .unwrap()
        .to_string();

    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    (temp_dir, db_path, conn)
}

/// Create a test database with nginx and openssl packages.
///
/// Returns (TempDir, db_path) - keep the TempDir alive to prevent cleanup.
pub fn setup_command_test_db() -> (TempDir, String) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir
        .path()
        .join("test.db")
        .to_str()
        .unwrap()
        .to_string();

    db::init(&db_path).unwrap();
    stage_test_boot_assets(temp_dir.path());
    let cas = conary_core::filesystem::CasStore::new(temp_dir.path().join("objects")).unwrap();
    let nginx_binary = vec![b'n'; 1_024_000];
    let nginx_binary_hash = cas.store(&nginx_binary).unwrap();
    let nginx_binary_size = i64::try_from(nginx_binary.len()).unwrap();
    let nginx_config = vec![b'c'; 2048];
    let nginx_config_hash = cas.store(&nginx_config).unwrap();
    let nginx_config_size = i64::try_from(nginx_config.len()).unwrap();
    let init_binary = b"test init binary";
    let init_binary_hash = cas.store(init_binary).unwrap();
    let init_binary_size = i64::try_from(init_binary.len()).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    db::transaction(&mut conn, |tx| {
        // Create nginx package with files
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

        // Add nginx components
        let mut nginx_runtime = Component::new(nginx_id, "runtime".to_string());
        let runtime_id = nginx_runtime.insert(tx)?;

        let mut nginx_config = Component::new(nginx_id, "config".to_string());
        let config_id = nginx_config.insert(tx)?;

        // Add nginx files
        let mut f1 = FileEntry::new(
            "/usr/sbin/nginx".to_string(),
            nginx_binary_hash.clone(),
            nginx_binary_size,
            0o755,
            nginx_id,
        );
        f1.component_id = Some(runtime_id);
        f1.insert(tx)?;

        let mut f2 = FileEntry::new(
            "/etc/nginx/nginx.conf".to_string(),
            nginx_config_hash.clone(),
            nginx_config_size,
            0o644,
            nginx_id,
        );
        f2.component_id = Some(config_id);
        f2.insert(tx)?;

        // Add nginx provides
        let mut p1 = ProvideEntry::new(nginx_id, "nginx".to_string(), Some("1.24.0".to_string()));
        p1.insert(tx)?;
        let mut p2 = ProvideEntry::new(nginx_id, "webserver".to_string(), None);
        p2.insert(tx)?;

        // Add nginx dependency
        let mut dep = DependencyEntry::new(
            nginx_id,
            "openssl".to_string(),
            Some(">= 3.0".to_string()),
            "runtime".to_string(),
            None,
        );
        dep.insert(tx)?;

        changeset1.update_status(tx, ChangesetStatus::Applied)?;

        // Create openssl package
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
        let openssl_runtime_id = openssl_runtime.insert(tx)?;

        let mut init = FileEntry::new(
            "/usr/sbin/init".to_string(),
            init_binary_hash.clone(),
            init_binary_size,
            0o755,
            openssl_id,
        );
        init.component_id = Some(openssl_runtime_id);
        init.insert(tx)?;

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
