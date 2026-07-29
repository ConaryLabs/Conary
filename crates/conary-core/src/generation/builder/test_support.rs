// conary-core/src/generation/builder/test_support.rs

#[cfg(any(unix, feature = "composefs-rs"))]
use std::path::Path;
#[cfg(feature = "composefs-rs")]
use std::path::PathBuf;

#[cfg(feature = "composefs-rs")]
use crate::db::models::{FileEntry, Trove, TroveType};
#[cfg(feature = "composefs-rs")]
use crate::db::schema::ensure_current;
#[cfg(feature = "composefs-rs")]
use crate::filesystem::CasStore;
#[cfg(feature = "composefs-rs")]
use crate::payload::{PayloadContentAuthority, PayloadNode, PayloadNodeKind, ResolvedPayloadNode};

#[cfg(feature = "composefs-rs")]
pub(crate) fn persist_test_host_capabilities(conn: &rusqlite::Connection) {
    crate::ccs::HostCapabilityInventory::default()
        .persist(conn)
        .unwrap();
}

#[cfg(feature = "composefs-rs")]
pub(super) fn regular_file_entry(
    path: &str,
    sha256: String,
    size: usize,
    mode: u32,
    trove_id: i64,
) -> FileEntry {
    FileEntry::new(
        path.to_string(),
        ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(mode & 0o7777)).unwrap(),
        Some(PayloadContentAuthority {
            sha256,
            size: size as u64,
        }),
        trove_id,
    )
}

#[cfg(feature = "composefs-rs")]
pub(crate) fn insert_regular_file_with_parents(
    conn: &rusqlite::Connection,
    path: &str,
    sha256: String,
    size: usize,
    mode: u32,
    trove_id: i64,
) {
    let path = Path::new(path);
    assert!(path.is_absolute(), "fixture path must be absolute");

    let mut parents = path
        .ancestors()
        .skip(1)
        .filter(|parent| *parent != Path::new("/"))
        .collect::<Vec<_>>();
    parents.reverse();
    for parent in parents {
        let parent = parent.to_str().unwrap();
        if let Some(existing) = FileEntry::find_by_path(conn, parent).unwrap() {
            assert!(
                matches!(existing.node.source.kind, PayloadNodeKind::Directory),
                "fixture parent {parent} must be a directory"
            );
            continue;
        }

        let mut node = PayloadNode::regular(0o755);
        node.kind = PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | 0o755;
        let mut entry = FileEntry::new(
            parent.to_string(),
            ResolvedPayloadNode::from_numeric_source(node).unwrap(),
            None,
            trove_id,
        );
        entry.insert(conn).unwrap();
    }

    let mut entry = regular_file_entry(path.to_str().unwrap(), sha256, size, mode, trove_id);
    entry.insert(conn).unwrap();
}

#[cfg(unix)]
pub(super) fn write_executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, contents).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(feature = "composefs-rs")]
pub(super) fn runtime_generation_db_with_wrong_sized_regular_file_cas_object() -> (
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
    let bad_bytes = b"bad";
    let bad_hash = cas.store(bad_bytes).unwrap();
    let init_hash = cas.store(b"init").unwrap();
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    persist_test_host_capabilities(&conn);
    let mut trove = Trove::new(
        "kernel-core".to_string(),
        "6.19.8-conary".to_string(),
        TroveType::Package,
        crate::repository::versioning::VersionScheme::Conary,
    );
    trove.architecture = Some("x86_64".to_string());
    let trove_id = trove.insert(&conn).unwrap();
    insert_regular_file_with_parents(
        &conn,
        "/usr/bin/bad",
        bad_hash.clone(),
        bad_bytes.len() + 1,
        0o100755,
        trove_id,
    );
    insert_regular_file_with_parents(
        &conn,
        "/sbin/init",
        init_hash,
        b"init".len(),
        0o100755,
        trove_id,
    );

    (tmp, conn, generations_root, boot_root, bad_hash)
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
    ensure_current(&conn).unwrap();
    persist_test_host_capabilities(&conn);
    let mut trove = Trove::new(
        "kernel-core".to_string(),
        "6.19.8-conary".to_string(),
        TroveType::Package,
        crate::repository::versioning::VersionScheme::Conary,
    );
    trove.architecture = Some("x86_64".to_string());
    let trove_id = trove.insert(&conn).unwrap();
    insert_regular_file_with_parents(
        &conn,
        "/usr/bin/missing",
        missing_hash.clone(),
        b"missing".len(),
        0o100755,
        trove_id,
    );
    insert_regular_file_with_parents(
        &conn,
        "/sbin/init",
        init_hash,
        b"init".len(),
        0o100755,
        trove_id,
    );

    (tmp, conn, generations_root, boot_root, missing_hash)
}

pub(super) fn assert_cas_size_mismatch_error(error: &str, hash: &str) {
    for snippet in ["CAS object", hash, "size mismatch"] {
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
