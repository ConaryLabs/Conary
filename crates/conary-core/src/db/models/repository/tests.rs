// conary-core/src/db/models/repository/tests.rs

use super::*;

use rusqlite::Connection;

#[test]
fn repository_requires_typed_parser_config_for_declared_format() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    let mut repository = Repository::new(
        "missing-parser".to_string(),
        "https://example.test/repository".to_string(),
    );
    repository.package_format = RepositoryFormat::Fedora;

    let error = repository.insert(&conn).unwrap_err();
    assert!(matches!(error, Error::InitError(_)));
}

#[test]
fn duplicate_repository_name_is_a_typed_conflict() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    let mut first = Repository::new(
        "exact-name".to_string(),
        "https://one.example.test".to_string(),
    );
    first.insert(&conn).unwrap();
    let mut duplicate = Repository::new(
        "exact-name".to_string(),
        "https://two.example.test".to_string(),
    );

    assert!(matches!(
        duplicate.insert(&conn),
        Err(Error::ConflictError(_))
    ));
}

#[test]
fn repository_rejects_retired_default_strategy() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    let mut repository = Repository::new(
        "retired-strategy".to_string(),
        "https://example.test/repository".to_string(),
    );
    repository.default_strategy = Some("legacy".to_string());

    assert!(matches!(
        repository.insert(&conn),
        Err(Error::ConfigError(_))
    ));
}

#[test]
fn repository_security_advisory_support_defaults_to_unknown() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "security-default".to_string(),
        "https://example.test".to_string(),
    );
    let id = repo.insert(&conn).unwrap();

    let loaded = Repository::find_by_id(&conn, id).unwrap().unwrap();
    assert_eq!(
        loaded.security_advisory_support,
        SecurityAdvisorySupport::Unknown
    );
}

#[test]
fn repository_security_advisory_support_round_trips() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "security-supported".to_string(),
        "https://example.test".to_string(),
    );
    repo.security_advisory_support = SecurityAdvisorySupport::Supported;
    let id = repo.insert(&conn).unwrap();

    let loaded = Repository::find_by_id(&conn, id).unwrap().unwrap();
    assert_eq!(
        loaded.security_advisory_support,
        SecurityAdvisorySupport::Supported
    );
}

#[test]
fn repository_package_round_trips_package_release() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "release-test".to_string(),
        "https://example.test".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();
    let mut package = RepositoryPackage::new(
        repo_id,
        "hello".to_string(),
        "1.0.0".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:hello".to_string(),
        42,
        "/v1/chunks/hello".to_string(),
    );
    package.package_release = "2".to_string();
    let id = package.insert(&conn).unwrap();
    let loaded = RepositoryPackage::find_by_id(&conn, id).unwrap().unwrap();
    assert_eq!(loaded.package_release, "2");
    assert_eq!(
        loaded.version_scheme,
        crate::repository::versioning::VersionScheme::Rpm
    );
}

#[test]
fn repository_package_rejects_unknown_persisted_version_scheme() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "invalid-scheme-test".to_string(),
        "https://example.test".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();
    let mut package = RepositoryPackage::new(
        repo_id,
        "hello".to_string(),
        "1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:hello".to_string(),
        42,
        "/v1/chunks/hello".to_string(),
    );
    let id = package.insert(&conn).unwrap();

    conn.execute_batch("PRAGMA ignore_check_constraints = ON")
        .unwrap();
    conn.execute(
        "UPDATE repository_packages SET version_scheme = 'unknown' WHERE id = ?1",
        [id],
    )
    .unwrap();

    let error = RepositoryPackage::find_by_id(&conn, id).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported persisted native version scheme 'unknown'")
    );
}

#[test]
fn repository_package_rejects_missing_persisted_version_scheme() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "missing-scheme-test".to_string(),
        "https://example.test".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();
    let mut package = RepositoryPackage::new(
        repo_id,
        "hello".to_string(),
        "1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:hello".to_string(),
        42,
        "/v1/chunks/hello".to_string(),
    );
    let id = package.insert(&conn).unwrap();

    let columns = RepositoryPackage::COLUMNS.replace(
        "source_profile, version_scheme, canonical_id",
        "source_profile, NULL AS version_scheme, canonical_id",
    );
    let sql = format!("SELECT {columns} FROM repository_packages WHERE id = ?1");
    let error = conn
        .query_row(&sql, [id], RepositoryPackage::from_row)
        .unwrap_err();

    assert!(matches!(
        error,
        rusqlite::Error::InvalidColumnType(19, _, rusqlite::types::Type::Null)
    ));
}

#[test]
fn repository_package_schema_has_no_flat_dependency_payload() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info('repository_packages')")
        .unwrap();
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();

    assert!(!columns.iter().any(|column| column == "dependencies"));
}

#[test]
fn repository_schema_rejects_package_format_as_source_profile() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    let error = conn
        .execute(
            "INSERT INTO repositories (name, url, source_profile)
             VALUES ('invalid-profile', 'https://example.test', 'rpm')",
            [],
        )
        .unwrap_err();

    assert!(error.to_string().contains("CHECK constraint failed"));
}

#[test]
fn repository_package_schema_rejects_route_alias_as_source_profile() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    conn.execute(
        "INSERT INTO repositories (name, url, source_profile)
         VALUES ('fedora', 'https://example.test', 'fedora-44')",
        [],
    )
    .unwrap();

    let error = conn
        .execute(
            "INSERT INTO repository_packages (
                repository_id, name, version, architecture, checksum, size,
                download_url, source_profile, version_scheme
             ) VALUES (1, 'demo', '1-1', 'x86_64', 'sha256:demo', 1,
                       'https://example.test/demo.rpm', 'fedora', 'rpm')",
            [],
        )
        .unwrap_err();

    assert!(error.to_string().contains("CHECK constraint failed"));
}
