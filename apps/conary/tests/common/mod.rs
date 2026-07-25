// tests/common/mod.rs

//! Shared test utilities and helpers for integration tests.

use conary_core::db;
use conary_core::db::models::{
    Changeset, ChangesetStatus, Component, FileEntry, InstalledRequirementGroup, ProvideEntry,
    Trove, TroveType,
};
use conary_core::payload::{PayloadContentAuthority, PayloadNode, ResolvedPayloadNode};
use conary_core::repository::dependency_model::RepositoryRequirementKind;
use conary_core::repository::requirement::parse_native_requirement;
use conary_core::repository::versioning::VersionScheme;
use tempfile::TempDir;

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
pub struct DatabaseSnapshot {
    schema: Vec<(String, String, Option<String>)>,
    rows: Vec<(String, Vec<Vec<Vec<u8>>>)>,
}

pub fn regular_file_entry(
    path: impl Into<String>,
    content: &[u8],
    permissions: u32,
    trove_id: i64,
) -> FileEntry {
    FileEntry::new(
        path.into(),
        ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(permissions)).unwrap(),
        Some(PayloadContentAuthority {
            sha256: conary_core::hash::sha256(content),
            size: content.len() as u64,
        }),
        trove_id,
    )
}

/// Capture every user-visible SQLite schema entry and table cell.
///
/// SQLite may update file-level bookkeeping while opening a database. Comparing
/// the logical contents avoids mistaking that bookkeeping for a row or schema
/// mutation while still covering every table.
#[allow(dead_code)]
pub fn database_snapshot(db_path: &str) -> DatabaseSnapshot {
    use rusqlite::types::ValueRef;

    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let mut schema = conn
        .prepare(
            "SELECT type, name, sql FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_autoindex_%'
             ORDER BY type, name",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    schema.sort();

    let table_names = schema
        .iter()
        .filter(|(kind, _, _)| kind == "table")
        .map(|(_, name, _)| name.clone())
        .collect::<Vec<_>>();
    let mut rows = Vec::with_capacity(table_names.len());
    for table in table_names {
        let quoted = table.replace('"', "\"\"");
        let mut statement = conn
            .prepare(&format!("SELECT * FROM \"{quoted}\""))
            .unwrap();
        let column_count = statement.column_count();
        let mut table_rows = statement
            .query_map([], |row| {
                (0..column_count)
                    .map(|column| {
                        Ok(match row.get_ref(column)? {
                            ValueRef::Null => vec![0],
                            ValueRef::Integer(value) => {
                                let mut encoded = vec![1];
                                encoded.extend_from_slice(&value.to_be_bytes());
                                encoded
                            }
                            ValueRef::Real(value) => {
                                let mut encoded = vec![2];
                                encoded.extend_from_slice(&value.to_bits().to_be_bytes());
                                encoded
                            }
                            ValueRef::Text(value) => {
                                let mut encoded = vec![3];
                                encoded.extend_from_slice(value);
                                encoded
                            }
                            ValueRef::Blob(value) => {
                                let mut encoded = vec![4];
                                encoded.extend_from_slice(value);
                                encoded
                            }
                        })
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        table_rows.sort();
        rows.push((table, table_rows));
    }

    DatabaseSnapshot { schema, rows }
}

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
#[allow(dead_code)]
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
    cas.store(&nginx_binary).unwrap();
    let nginx_config = vec![b'c'; 2048];
    cas.store(&nginx_config).unwrap();
    let init_binary = b"test init binary";
    cas.store(init_binary).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    db::transaction(&mut conn, |tx| {
        // Create nginx package with files
        let mut changeset1 = Changeset::new("Install nginx-1.24.0".to_string());
        let changeset1_id = changeset1.insert(tx)?;

        let mut nginx = Trove::new(
            "nginx".to_string(),
            "1.24.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        nginx.architecture = Some("x86_64".to_string());
        nginx.description = Some("High performance web server".to_string());
        nginx.installed_by_changeset_id = Some(changeset1_id);
        let nginx_id = nginx.insert(tx)?;

        // Add nginx components
        let mut nginx_runtime = Component::new(nginx_id, "runtime".to_string());
        let runtime_id = nginx_runtime.insert(tx)?;

        let mut nginx_config_component = Component::new(nginx_id, "config".to_string());
        let config_id = nginx_config_component.insert(tx)?;

        // Add nginx files
        let mut f1 = regular_file_entry("/usr/sbin/nginx", &nginx_binary, 0o755, nginx_id);
        f1.component_id = Some(runtime_id);
        f1.insert(tx)?;

        let mut f2 = regular_file_entry("/etc/nginx/nginx.conf", &nginx_config, 0o644, nginx_id);
        f2.component_id = Some(config_id);
        f2.insert(tx)?;

        // Add nginx provides
        let mut p1 = ProvideEntry::new(nginx_id, "nginx".to_string(), Some("1.24.0".to_string()));
        p1.insert(tx)?;
        let mut p2 = ProvideEntry::new(nginx_id, "webserver".to_string(), None);
        p2.insert(tx)?;

        // Add nginx dependency
        let dependency = parse_native_requirement(
            RepositoryRequirementKind::Depends,
            VersionScheme::Conary,
            "openssl >= 3.0.0",
        )
        .map_err(conary_core::Error::ParseError)?;
        InstalledRequirementGroup::insert_groups(
            tx,
            nginx_id,
            VersionScheme::Conary,
            &[dependency],
        )?;

        changeset1.update_status(tx, ChangesetStatus::Applied)?;

        // Create openssl package
        let mut changeset2 = Changeset::new("Install openssl-3.0.0".to_string());
        let changeset2_id = changeset2.insert(tx)?;

        let mut openssl = Trove::new(
            "openssl".to_string(),
            "3.0.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        openssl.architecture = Some("x86_64".to_string());
        openssl.description = Some("Cryptography and SSL/TLS toolkit".to_string());
        openssl.installed_by_changeset_id = Some(changeset2_id);
        let openssl_id = openssl.insert(tx)?;

        let mut openssl_runtime = Component::new(openssl_id, "runtime".to_string());
        let openssl_runtime_id = openssl_runtime.insert(tx)?;

        let mut init = regular_file_entry("/usr/sbin/init", init_binary, 0o755, openssl_id);
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

#[allow(dead_code)]
fn stage_test_boot_assets(root: &std::path::Path) {
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
