// conary-core/src/db/models/repository/tests.rs

use super::*;

use crate::Error;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryFormat, RepositoryParserConfig, RepositoryTrustPolicy,
    RpmMetadataAuthority,
};
use rusqlite::Connection;

fn rpm_repository(
    name: &str,
    repository_identity: &str,
    scope: RepositoryPolicyScope,
    stream: NativeSourceStream,
    update_mode: RepositoryUpdateMode,
    pin: Option<&str>,
) -> Repository {
    let metadata_key = OpenPgpTrustRoot {
        url: "https://keys.example.test/metadata.gpg".to_string(),
        fingerprint: "A".repeat(40),
    };
    let package_key = OpenPgpTrustRoot {
        url: "https://keys.example.test/packages.gpg".to_string(),
        fingerprint: "B".repeat(40),
    };
    let mut repository = Repository::new(
        name.to_string(),
        format!("https://packages.example.test/{name}"),
    );
    repository
        .set_parser_config(RepositoryParserConfig::Rpm {
            architecture: "x86_64".to_string(),
        })
        .unwrap();
    repository
        .set_trust_policy(RepositoryTrustPolicy::Rpm {
            metadata: RpmMetadataAuthority::OpenPgp {
                keys: vec![metadata_key],
            },
            package_keys: vec![package_key],
        })
        .unwrap();
    let policy = RepositorySourcePolicy::new(
        "example-publisher",
        scope,
        NativeSourceEcosystem::Rpm,
        stream,
        update_mode,
    )
    .unwrap();
    repository
        .set_native_source_policy(
            policy,
            repository_identity,
            pin.map(AuthenticatedSnapshotIdentity::from_sha256)
                .transpose()
                .unwrap(),
        )
        .unwrap();
    repository
}

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
fn repository_model_rejects_package_format_as_source_profile() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    let mut repository = Repository::new(
        "invalid-profile".to_string(),
        "https://example.test".to_string(),
    );
    repository.source_profile = Some("rpm".to_string());
    let error = repository.insert(&conn).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("unsupported source profile 'rpm'")
    );
}

#[test]
fn repository_schema_has_no_fixed_source_profile_membership() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    for table in ["repositories", "repository_packages"] {
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!sql.contains("'fedora-44', 'ubuntu-26.04', 'arch'"));
    }
}

#[test]
fn native_source_policy_round_trips_uncatalogued_identity() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    let identity = "third-party:widgets:x86_64";
    let mut repository = rpm_repository(
        "third-party-widgets",
        identity,
        RepositoryPolicyScope::repository(identity).unwrap(),
        NativeSourceStream::channel("stable").unwrap(),
        RepositoryUpdateMode::Follow,
        None,
    );
    repository.insert(&conn).unwrap();

    let loaded = Repository::find_by_name(&conn, "third-party-widgets")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.repository_identity.as_deref(), Some(identity));
    assert_eq!(loaded.source_profile, None);
    let policy = loaded.source_policy.unwrap();
    assert_eq!(policy.source_identity, "example-publisher");
    assert_eq!(
        policy.stream,
        NativeSourceStream::channel("stable").unwrap()
    );
    assert_eq!(policy.update_mode, RepositoryUpdateMode::Follow);
}

#[test]
fn group_policy_is_shared_and_conflicting_reuse_is_rejected() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    let scope = RepositoryPolicyScope::group("official").unwrap();
    let stream = NativeSourceStream::release("44").unwrap();
    let mut base = rpm_repository(
        "base",
        "base:x86_64",
        scope.clone(),
        stream.clone(),
        RepositoryUpdateMode::Follow,
        None,
    );
    let mut updates = rpm_repository(
        "updates",
        "updates:x86_64",
        scope.clone(),
        stream,
        RepositoryUpdateMode::Follow,
        None,
    );
    base.insert(&conn).unwrap();
    updates.insert(&conn).unwrap();
    let base = Repository::find_by_name(&conn, "base").unwrap().unwrap();
    let updates = Repository::find_by_name(&conn, "updates").unwrap().unwrap();
    assert_eq!(
        base.source_policy.unwrap().id,
        updates.source_policy.unwrap().id
    );

    let mut conflicting = rpm_repository(
        "testing",
        "testing:x86_64",
        scope,
        NativeSourceStream::release("45").unwrap(),
        RepositoryUpdateMode::Follow,
        None,
    );
    let error = conflicting.insert(&conn).unwrap_err();
    assert!(matches!(error, Error::ConflictError(_)));
}

#[test]
fn unknown_persisted_source_policy_mode_fails_closed() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    let identity = "unknown-mode:x86_64";
    let mut repository = rpm_repository(
        "unknown-mode",
        identity,
        RepositoryPolicyScope::repository(identity).unwrap(),
        NativeSourceStream::channel("stable").unwrap(),
        RepositoryUpdateMode::Follow,
        None,
    );
    repository.insert(&conn).unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON")
        .unwrap();
    conn.execute(
        "UPDATE repository_source_policies SET update_mode = 'future'",
        [],
    )
    .unwrap();

    let error = Repository::find_by_name(&conn, "unknown-mode").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown persisted repository update mode 'future'")
    );
}

#[test]
fn stream_binding_rejects_endpoint_drift_but_ignores_rpm_package_key_rotation() {
    let identity = "binding:x86_64";
    let mut repository = rpm_repository(
        "binding",
        identity,
        RepositoryPolicyScope::repository(identity).unwrap(),
        NativeSourceStream::channel("stable").unwrap(),
        RepositoryUpdateMode::Follow,
        None,
    );
    repository.validate_stream_binding().unwrap();
    let RepositoryTrustPolicy::Rpm { package_keys, .. } = repository.trust_policy.as_mut().unwrap()
    else {
        unreachable!();
    };
    package_keys[0].fingerprint = "C".repeat(40);
    repository.validate_stream_binding().unwrap();

    repository.url = "https://other.example.test/binding".to_string();
    let error = repository.validate_stream_binding().unwrap_err();
    assert!(error.to_string().contains("explicit re-enrollment"));
}

#[test]
fn stream_binding_encoding_is_stable_for_revision_31() {
    let identity = "binding:x86_64";
    let repository = rpm_repository(
        "binding",
        identity,
        RepositoryPolicyScope::repository(identity).unwrap(),
        NativeSourceStream::channel("stable").unwrap(),
        RepositoryUpdateMode::Follow,
        None,
    );

    assert_eq!(
        repository.stream_binding_sha256.as_deref(),
        Some("d1c40eb96eb4184704fa17447c410362d098a96b64f072ff8e6eafa406b962b1")
    );
}

#[test]
fn persisted_pin_policy_without_member_pin_fails_closed() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    let identity = "pinned:x86_64";
    let mut repository = rpm_repository(
        "pinned",
        identity,
        RepositoryPolicyScope::repository(identity).unwrap(),
        NativeSourceStream::release("1").unwrap(),
        RepositoryUpdateMode::Pin,
        Some(&"a".repeat(64)),
    );
    let id = repository.insert(&conn).unwrap();
    conn.execute(
        "DELETE FROM repository_source_pins WHERE repository_id = ?1",
        [id],
    )
    .unwrap();

    let error = Repository::find_by_id(&conn, id).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("pin policy without a member pin")
    );
}

#[test]
fn same_native_policy_update_retains_normalized_policy_identity() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    let identity = "updates:x86_64";
    let mut repository = rpm_repository(
        "updates",
        identity,
        RepositoryPolicyScope::repository(identity).unwrap(),
        NativeSourceStream::channel("stable").unwrap(),
        RepositoryUpdateMode::Follow,
        None,
    );
    let id = repository.insert(&conn).unwrap();
    let policy = repository.source_policy.clone().unwrap();

    repository
        .set_native_source_policy(policy, identity, None)
        .unwrap();
    repository.priority = 42;
    repository.update(&conn).unwrap();

    let loaded = Repository::find_by_id(&conn, id).unwrap().unwrap();
    assert_eq!(loaded.priority, 42);
    assert!(loaded.source_policy.unwrap().id.is_some());
}

#[test]
fn native_stream_change_requires_explicit_reenrollment() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    let identity = "stream-change:x86_64";
    let mut repository = rpm_repository(
        "stream-change",
        identity,
        RepositoryPolicyScope::repository(identity).unwrap(),
        NativeSourceStream::channel("stable").unwrap(),
        RepositoryUpdateMode::Follow,
        None,
    );
    repository.insert(&conn).unwrap();

    repository.url = "https://mirror.example.test/stream-change".to_string();
    let policy = repository.source_policy.clone().unwrap();
    repository
        .set_native_source_policy(policy, identity, None)
        .unwrap();
    let error = repository.update(&conn).unwrap_err();

    assert!(error.to_string().contains("explicit re-enrollment"));
}

#[test]
fn native_member_pin_change_requires_explicit_reenrollment() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    let identity = "pin-change:x86_64";
    let mut repository = rpm_repository(
        "pin-change",
        identity,
        RepositoryPolicyScope::repository(identity).unwrap(),
        NativeSourceStream::release("1").unwrap(),
        RepositoryUpdateMode::Pin,
        Some(&"a".repeat(64)),
    );
    repository.insert(&conn).unwrap();

    let policy = repository.source_policy.clone().unwrap();
    repository
        .set_native_source_policy(
            policy,
            identity,
            Some(AuthenticatedSnapshotIdentity::from_sha256("b".repeat(64)).unwrap()),
        )
        .unwrap();
    let error = repository.update(&conn).unwrap_err();

    assert!(error.to_string().contains("explicit re-enrollment"));
}

#[test]
fn schema_rejects_repository_scope_identity_mismatch() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    let identity = "base:x86_64";
    let mut repository = rpm_repository(
        "base",
        identity,
        RepositoryPolicyScope::repository(identity).unwrap(),
        NativeSourceStream::release("44").unwrap(),
        RepositoryUpdateMode::Follow,
        None,
    );
    let id = repository.insert(&conn).unwrap();

    let error = conn
        .execute(
            "UPDATE repositories SET repository_identity = 'updates:x86_64' WHERE id = ?1",
            [id],
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("repository-scope source policy identity mismatch")
    );
}

#[test]
fn repository_delete_removes_only_an_orphaned_source_policy() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    let scope = RepositoryPolicyScope::group("official").unwrap();
    let stream = NativeSourceStream::release("44").unwrap();
    let mut base = rpm_repository(
        "base-delete",
        "base:x86_64",
        scope.clone(),
        stream.clone(),
        RepositoryUpdateMode::Follow,
        None,
    );
    let mut updates = rpm_repository(
        "updates-delete",
        "updates:x86_64",
        scope,
        stream,
        RepositoryUpdateMode::Follow,
        None,
    );
    let base_id = base.insert(&conn).unwrap();
    let updates_id = updates.insert(&conn).unwrap();
    let policy_id = base.source_policy.unwrap().id.unwrap();

    Repository::delete(&conn, base_id).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM repository_source_policies WHERE id = ?1",
            [policy_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );

    Repository::delete(&conn, updates_id).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM repository_source_policies WHERE id = ?1",
            [policy_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
}
