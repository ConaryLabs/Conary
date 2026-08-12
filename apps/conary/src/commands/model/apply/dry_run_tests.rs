// apps/conary/src/commands/model/apply/dry_run_tests.rs

use super::super::test_support::{
    build_test_ccs_package_with_payloads_and_relations, insert_test_repository_package_resolution,
    insert_test_static_ccs_repository, serve_test_file_n,
};
use super::*;
use crate::commands::test_helpers::create_test_db;
use conary_core::db::models::RepositoryPackage;
use conary_core::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementGroup,
    RepositoryRequirementKind,
};
use tempfile::tempdir;

#[derive(Debug, PartialEq, Eq)]
struct LogicalDatabaseSnapshot(Vec<(String, Vec<Vec<Vec<u8>>>)>);

fn logical_database_snapshot(db_path: &str) -> LogicalDatabaseSnapshot {
    use rusqlite::types::ValueRef;

    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let table_names = conn
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    let mut tables = Vec::with_capacity(table_names.len());
    for table in table_names {
        let quoted = table.replace('"', "\"\"");
        let mut statement = conn
            .prepare(&format!("SELECT * FROM \"{quoted}\""))
            .unwrap();
        let column_count = statement.column_count();
        let mut rows = statement
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
        rows.sort();
        tables.push((table, rows));
    }
    LogicalDatabaseSnapshot(tables)
}

#[tokio::test]
async fn model_apply_dry_run_rejects_the_same_invalid_package_transaction_as_apply() {
    let (_temp_file, db_path) = create_test_db();
    let temp_dir = tempdir().unwrap();
    let package_name = "model-invalid-transaction";
    let version = "1-1";
    let missing_path = "/usr/lib/model-missing-provider.so";
    let requirement = RepositoryRequirementGroup::simple(
        RepositoryRequirementKind::Depends,
        RepositoryRequirementClause {
            name: missing_path.to_string(),
            capability_kind: Some(RepositoryCapabilityKind::File),
            version_constraint: None,
            architecture_qualifier: Default::default(),
            native_text: Some(missing_path.to_string()),
        },
    );
    let package_path = build_test_ccs_package_with_payloads_and_relations(
        temp_dir.path(),
        package_name,
        version,
        conary_core::repository::versioning::VersionScheme::Rpm,
        None,
        vec![(
            format!("/usr/bin/{package_name}"),
            b"#!/bin/sh\nexit 0\n".to_vec(),
            0o755,
        )],
        vec![requirement],
        Vec::new(),
    );
    let (package_url, _server) = serve_test_file_n(package_path.clone(), 2);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let repository_id = insert_test_static_ccs_repository(
        &conn,
        "fedora",
        "https://example.test/fedora",
        "fedora-44",
    );
    let checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
    let mut package = RepositoryPackage::new(
        repository_id,
        package_name.to_string(),
        version.to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        checksum,
        std::fs::metadata(&package_path)
            .unwrap()
            .len()
            .try_into()
            .unwrap(),
        package_url,
    );
    package.architecture = Some("x86_64".to_string());
    package.source_profile = Some("fedora-44".to_string());
    let package_id = package.insert(&conn).unwrap();
    insert_test_repository_package_resolution(
        &conn,
        repository_id,
        package_id,
        package_name,
        version,
    );
    drop(conn);

    let model_path = temp_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        format!(
            r#"
[model]
version = 1
install = ["{package_name}"]
"#,
        ),
    )
    .unwrap();
    let options = |dry_run| ApplyOptions {
        model_path: model_path.to_str().unwrap(),
        db_path: &db_path,
        root: temp_dir.path().to_str().unwrap(),
        dry_run,
        skip_optional: false,
        strict: true,
        autoremove: false,
        offline: true,
    };

    let before = logical_database_snapshot(&db_path);
    let dry_run_error = cmd_model_apply(options(true))
        .await
        .expect_err("dry-run must validate the exact prepared package transaction");
    assert_eq!(
        logical_database_snapshot(&db_path),
        before,
        "dry-run must leave every persisted table unchanged"
    );

    let apply_error = cmd_model_apply(options(false))
        .await
        .expect_err("apply must reject the invalid package transaction");
    assert_eq!(
        logical_database_snapshot(&db_path),
        before,
        "rejected apply must fail before persisted mutation"
    );

    let dry_run_error = format!("{dry_run_error:#}");
    assert!(
        dry_run_error
            .contains("selected package transaction does not satisfy exact depends requirement"),
        "{dry_run_error}"
    );
    assert!(dry_run_error.contains(package_name), "{dry_run_error}");
    assert_eq!(dry_run_error, format!("{apply_error:#}"));
}
