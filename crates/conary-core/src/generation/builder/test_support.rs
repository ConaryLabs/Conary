// conary-core/src/generation/builder/test_support.rs

#[cfg(unix)]
use std::path::Path;
#[cfg(feature = "composefs-rs")]
use std::path::PathBuf;

#[cfg(feature = "composefs-rs")]
use crate::db::models::{FileEntry, Trove, TroveType};
#[cfg(feature = "composefs-rs")]
use crate::db::schema::migrate;
#[cfg(feature = "composefs-rs")]
use crate::filesystem::CasStore;

#[cfg(unix)]
pub(super) fn write_executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, contents).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(feature = "composefs-rs")]
pub(super) fn runtime_generation_db_with_invalid_regular_file()
-> (tempfile::TempDir, rusqlite::Connection, PathBuf, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let generations_root = tmp.path().join("generations");
    let objects_dir = tmp.path().join("objects");
    let boot_root = tmp.path().join("boot");
    std::fs::create_dir_all(&generations_root).unwrap();
    std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
    std::fs::write(boot_root.join("vmlinuz-6.19.8-conary"), b"kernel").unwrap();
    std::fs::write(boot_root.join("initramfs-6.19.8-conary.img"), b"initramfs").unwrap();
    std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

    let cas = CasStore::new(&objects_dir).unwrap();
    let init_hash = cas.store(b"init").unwrap();
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();
    let mut trove = Trove::new(
        "kernel-core".to_string(),
        "6.19.8-conary".to_string(),
        TroveType::Package,
    );
    trove.architecture = Some("x86_64".to_string());
    let trove_id = trove.insert(&conn).unwrap();
    let mut bad = FileEntry::new(
        "/usr/bin/bad".to_string(),
        "not-a-sha256".to_string(),
        0,
        0o100755,
        trove_id,
    );
    bad.insert(&conn).unwrap();
    let mut init = FileEntry::new(
        "/usr/sbin/init".to_string(),
        init_hash,
        b"init".len() as i64,
        0o100755,
        trove_id,
    );
    init.insert(&conn).unwrap();

    (tmp, conn, generations_root, boot_root)
}

#[cfg(feature = "composefs-rs")]
pub(super) fn runtime_generation_db_with_missing_regular_file_cas_object() -> (
    tempfile::TempDir,
    rusqlite::Connection,
    PathBuf,
    PathBuf,
    String,
) {
    let tmp = tempfile::TempDir::new().unwrap();
    let generations_root = tmp.path().join("generations");
    let objects_dir = tmp.path().join("objects");
    let boot_root = tmp.path().join("boot");
    std::fs::create_dir_all(&generations_root).unwrap();
    std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
    std::fs::write(boot_root.join("vmlinuz-6.19.8-conary"), b"kernel").unwrap();
    std::fs::write(boot_root.join("initramfs-6.19.8-conary.img"), b"initramfs").unwrap();
    std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

    let cas = CasStore::new(&objects_dir).unwrap();
    let init_hash = cas.store(b"init").unwrap();
    let missing_hash = CasStore::compute_sha256(b"missing");
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();
    let mut trove = Trove::new(
        "kernel-core".to_string(),
        "6.19.8-conary".to_string(),
        TroveType::Package,
    );
    trove.architecture = Some("x86_64".to_string());
    let trove_id = trove.insert(&conn).unwrap();
    let mut missing = FileEntry::new(
        "/usr/bin/missing".to_string(),
        missing_hash.clone(),
        b"missing".len() as i64,
        0o100755,
        trove_id,
    );
    missing.insert(&conn).unwrap();
    let mut init = FileEntry::new(
        "/usr/sbin/init".to_string(),
        init_hash,
        b"init".len() as i64,
        0o100755,
        trove_id,
    );
    init.insert(&conn).unwrap();

    (tmp, conn, generations_root, boot_root, missing_hash)
}

pub(super) fn assert_invalid_runtime_input_error(error: &str) {
    for snippet in [
        "exportable runtime generation is not self-contained",
        "package kernel-core",
        "/usr/bin/bad",
        "invalid SHA-256 digest for regular file",
        "conary system adopt --system --full",
        "conary system takeover --up-to generation",
    ] {
        assert!(
            error.contains(snippet),
            "expected error to contain {snippet:?}; got {error}"
        );
    }
}

pub(super) fn assert_missing_cas_object_error(error: &str, hash: &str) {
    for snippet in ["missing CAS object", hash] {
        assert!(
            error.contains(snippet),
            "expected error to contain {snippet:?}; got {error}"
        );
    }
}
