// crates/conary-core/src/db/schema/tests.rs
use super::*;

#[test]
fn ensure_current_creates_schema_atomically() {
    let conn = Connection::open_in_memory().unwrap();

    ensure_current(&conn).unwrap();

    assert_eq!(get_schema_version(&conn).unwrap(), SCHEMA_VERSION);
    assert_eq!(
        get_schema_identity(&conn).unwrap(),
        Some((SCHEMA_EPOCH.to_string(), SCHEMA_VERSION))
    );
    for table in [
        "troves",
        "repositories",
        "converted_packages",
        "installed_file_capabilities",
        "generation_publications",
        "selected_root_snapshots",
        "selected_root_snapshot_entries",
        "activation_requests",
        "lifecycle_events",
        "generation_activation_intents",
        "debian_alternative_groups",
        "repository_sync_runs",
        "repository_sync_scopes",
        "repository_sync_run_members",
        "remi_client_sync_state",
        "remi_sparse_projection_revisions",
        "remi_catalog_resources",
        "remi_catalog_gc_deletions",
        "remi_profile_revision_members",
        "remi_active_profile_revisions",
        "remi_runtime_sessions",
        "remi_profile_revision_pins",
    ] {
        assert!(table_exists(&conn, table).unwrap(), "missing {table}");
    }
}

#[test]
fn current_schema_has_no_mutable_authenticated_snapshot_authority() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let column_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('repositories')
             WHERE name = 'authenticated_snapshot_sha256'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(column_count, 0);
}

#[test]
fn ensure_current_is_idempotent_for_current_epoch() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    ensure_current(&conn).unwrap();
    assert_eq!(get_schema_version(&conn).unwrap(), SCHEMA_VERSION);
}

#[test]
fn revision_45_requires_rebuild_for_revision_49() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_identity (
            epoch TEXT NOT NULL,
            revision INTEGER NOT NULL
        );
        INSERT INTO schema_identity (epoch, revision)
            VALUES ('conary-current-v1', 45);
        CREATE TABLE schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        INSERT INTO schema_version (version) VALUES (45);",
    )
    .unwrap();

    let error = ensure_current(&conn).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Database schema rebuild required: database uses schema epoch conary-current-v1 revision 45; this pre-alpha build supports only schema epoch conary-current-v1 revision 49"
    );
}

/// The revision, not the table text, is what separates the two provenance
/// shapes. `provides.provenance` is JSON inside a UNIQUE contract index and
/// no resync rebuilds installed rows, so a database carrying the previous
/// revision must be refused rather than silently mixed.
#[test]
fn a_database_holding_duplicated_path_provenance_cannot_pass_the_version_gate() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    // Exactly the shape revision 26 wrote for one package-owned path.
    let retired = r#"{"role":"source-derived-file","format":"rpm","source_path":"/usr/bin/sh"}"#;
    let current = r#"{"role":"source-derived-file","format":"rpm"}"#;
    conn.execute(
        "INSERT INTO troves
         (name, version, type, install_source, install_reason, version_scheme)
         VALUES ('tool', '1', 'package', 'repository', 'explicit', 'rpm')",
        [],
    )
    .unwrap();
    let trove_id = conn.last_insert_rowid();
    for provenance in [retired, current] {
        conn.execute(
            "INSERT INTO provides
             (trove_id, capability, kind, version_scheme, architecture_qualifier_kind, provenance)
             VALUES (?1, '/usr/bin/sh', 'file', 'rpm', 'implicit', ?2)",
            params![trove_id, provenance],
        )
        .unwrap();
    }

    // The UNIQUE contract index admits both, so one path has two providers:
    // this is the state the version gate exists to keep out.
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM provides WHERE trove_id = ?1 AND capability = '/usr/bin/sh'",
            params![trove_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        2,
    );

    // Revision 26 is the last revision that wrote this shape. Naming it
    // absolutely keeps the test pinned to that database, not to whatever
    // revision happens to sit one below the current constant.
    conn.execute("UPDATE schema_identity SET revision = 26", [])
        .unwrap();
    conn.execute("UPDATE schema_version SET version = 26", [])
        .unwrap();

    let error = ensure_current(&conn).unwrap_err();
    assert!(
        matches!(error, Error::SchemaRebuildRequired { supported_revision, .. }
            if supported_revision == SCHEMA_VERSION),
        "{error}"
    );
}

#[test]
fn read_only_inspection_reports_schema_compatibility_without_mutation() {
    let missing_dir = tempfile::tempdir().unwrap();
    let missing = missing_dir.path().join("missing.db");
    assert_eq!(inspect(&missing).unwrap(), SchemaCompatibility::Fresh);

    let current = tempfile::NamedTempFile::new().unwrap();
    let conn = Connection::open(current.path()).unwrap();
    ensure_current(&conn).unwrap();
    drop(conn);
    assert_eq!(
        inspect(current.path()).unwrap(),
        SchemaCompatibility::Current
    );

    let retired = tempfile::NamedTempFile::new().unwrap();
    let conn = Connection::open(retired.path()).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        INSERT INTO schema_version (version) VALUES (79);",
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        inspect(retired.path()).unwrap(),
        SchemaCompatibility::RebuildRequired {
            observed: "retired migration-chain schema version 79".to_string()
        }
    );
}

#[test]
fn current_schema_seeds_no_heuristic_package_triggers() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let retired_builtin_columns: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('triggers') WHERE name = 'builtin'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM triggers", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(retired_builtin_columns, 0);
    assert_eq!(
        conn.query_row(
            "SELECT value FROM server_metadata WHERE key = 'canonical_map_revision'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "0"
    );
}

#[test]
fn current_schema_has_only_exact_canonical_mapping_authority() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let retired_repo_id: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM pragma_table_info('package_implementations')
             WHERE name = 'repo_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retired_repo_id, 0);
    assert!(!table_exists(&conn, "package_overrides").unwrap());
}

#[test]
fn current_schema_rejects_invalid_trigger_and_derived_states() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('status checks', 'pending')",
        [],
    )
    .unwrap();
    let changeset_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO triggers (name, pattern, handler) VALUES ('fixture', '/usr/*', '/bin/true')",
        [],
    )
    .unwrap();
    let trigger_id = conn.last_insert_rowid();

    for (sql, params) in [
        (
            "INSERT INTO changeset_triggers (changeset_id, trigger_id, status)
             VALUES (?1, ?2, 'unknown')",
            [changeset_id, trigger_id],
        ),
        (
            "INSERT INTO changeset_triggers (changeset_id, trigger_id, status)
             VALUES (?1, ?2, 'already-ran-maybe')",
            [changeset_id, trigger_id],
        ),
    ] {
        let error = conn.execute(sql, params).unwrap_err();
        assert!(
            error.to_string().contains("CHECK constraint failed"),
            "{error}"
        );
    }

    for sql in [
        "INSERT INTO derived_packages (name, parent_name, status)
         VALUES ('invalid-status', 'parent', 'maybe-built')",
        "INSERT INTO derived_packages (name, parent_name, version_policy)
         VALUES ('invalid-policy', 'parent', 'invented')",
        "INSERT INTO derived_packages (name, parent_name, version_policy)
         VALUES ('missing-suffix', 'parent', 'suffix')",
    ] {
        let error = conn.execute(sql, []).unwrap_err();
        assert!(
            error.to_string().contains("CHECK constraint failed"),
            "{error}"
        );
    }
}

#[test]
fn current_schema_requires_explicit_installation_semantics() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut trove_columns = conn.prepare("PRAGMA table_info('troves')").unwrap();
    let trove_columns = trove_columns
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, bool>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for required in ["install_source", "install_reason", "version_scheme"] {
        let (_, not_null, default) = trove_columns
            .iter()
            .find(|(name, _, _)| name == required)
            .unwrap_or_else(|| panic!("missing troves.{required}"));
        assert!(*not_null, "troves.{required} must be NOT NULL");
        assert_eq!(
            default, &None,
            "troves.{required} must not invent a default"
        );
    }

    for sql in [
        "INSERT INTO troves (
             name, version, type, install_reason, version_scheme
         ) VALUES ('missing-source', '1', 'package', 'explicit', 'conary')",
        "INSERT INTO troves (
             name, version, type, install_source, version_scheme
         ) VALUES ('missing-reason', '1', 'package', 'file', 'conary')",
        "INSERT INTO troves (
             name, version, type, install_source, install_reason
         ) VALUES ('missing-scheme', '1', 'package', 'file', 'explicit')",
    ] {
        let error = conn.execute(sql, []).unwrap_err();
        assert!(
            error.to_string().contains("NOT NULL constraint failed"),
            "{error}"
        );
    }

    conn.execute(
        "INSERT INTO system_states (state_number, summary, package_count)
         VALUES (1, 'explicit semantics', 0)",
        [],
    )
    .unwrap();
    let error = conn
        .execute(
            "INSERT INTO state_members (state_id, trove_name, trove_version)
             VALUES (1, 'missing-reason', '1')",
            [],
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("state_members.install_reason"),
        "{error}"
    );
}

#[test]
fn current_schema_enforces_exact_null_architecture_trove_identity() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let insert = |name: &str, architecture: Option<&str>| {
        conn.execute(
            "INSERT INTO troves (
                 name, version, type, architecture, install_source,
                 install_reason, version_scheme
             ) VALUES (?1, '1', 'package', ?2, 'file', 'explicit', 'conary')",
            params![name, architecture],
        )
    };

    insert("null-architecture", None).unwrap();
    let duplicate = insert("null-architecture", None).unwrap_err().to_string();
    assert!(
        duplicate.contains("UNIQUE constraint failed")
            && duplicate.contains("idx_troves_exact_identity"),
        "{duplicate}"
    );
    insert("null-architecture", Some("x86_64")).unwrap();

    let empty = insert("empty-architecture", Some(""))
        .unwrap_err()
        .to_string();
    assert!(
        empty.contains("CHECK constraint failed")
            && empty.contains("architecture IS NULL OR length(architecture) > 0"),
        "{empty}"
    );
}

#[test]
fn current_schema_requires_exact_repository_artifact_architecture() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let insert = |checksum: &str, architecture: Option<&str>| {
        conn.execute(
            "INSERT INTO converted_packages (
                 artifact_kind, original_format, original_checksum,
                 package_name, package_version, source_profile, profile_revision_sha256,
                 package_architecture,
                 repository_provides_digest, transport_json, total_size,
                 content_hash, ccs_path
             ) VALUES (
                 'repository', 'rpm', ?1, 'fixture', '1.0-1', 'fedora-44',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 ?2, 'sha256:4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945',
                 '{}', 42, 'sha256:content',
                 '/tmp/fixture.ccs'
             )",
            params![checksum, architecture],
        )
    };

    for (checksum, architecture) in [
        ("sha256:missing-architecture", None),
        ("sha256:empty-architecture", Some("")),
    ] {
        let error = insert(checksum, architecture).unwrap_err().to_string();
        assert!(error.contains("CHECK constraint failed"), "{error}");
    }

    insert("sha256:exact-architecture", Some("x86_64")).unwrap();
}

#[test]
fn current_schema_requires_exact_repository_profile_revision_identity() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let insert = |checksum: &str, profile_revision: Option<&str>| {
        conn.execute(
            "INSERT INTO converted_packages (
                 artifact_kind, original_format, original_checksum,
                 package_name, package_version, source_profile,
                 profile_revision_sha256, package_architecture,
                 repository_provides_digest, transport_json, total_size,
                 content_hash, ccs_path
             ) VALUES (
                 'repository', 'rpm', ?1, 'fixture', '1.0-1', 'fedora-44',
                 ?2, 'x86_64',
                 'sha256:4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945',
                 '{}', 42, 'sha256:content', '/tmp/fixture.ccs'
             )",
            params![checksum, profile_revision],
        )
    };

    assert!(insert("sha256:missing-revision", None).is_err());
    assert!(
        insert(
            "sha256:uppercase-revision",
            Some("Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        )
        .is_err()
    );

    let first = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let second = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    insert("sha256:revision-scoped", Some(first)).unwrap();
    insert("sha256:revision-scoped", Some(second)).unwrap();
    assert!(insert("sha256:revision-scoped", Some(first)).is_err());
}

#[test]
fn current_schema_requires_exact_native_identity_for_native_owned_troves() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    for sql in [
        "INSERT INTO troves (
             name, version, type, install_source, install_reason, version_scheme
         ) VALUES (
             'missing-native-id', '1', 'package', 'adopted-full', 'explicit', 'rpm'
         )",
        "INSERT INTO troves (
             name, version, type, install_source, install_reason, version_scheme,
             native_package_identity_json
         ) VALUES (
             'malformed-native-id', '1', 'package', 'taken', 'explicit', 'rpm', '{bad'
         )",
        "INSERT INTO troves (
             name, version, type, install_source, install_reason, version_scheme,
             native_package_identity_json
         ) VALUES (
             'conary-with-native-id', '1', 'package', 'file', 'explicit', 'conary',
             '{\"manager\":\"pacman\",\"selector\":\"bash\",\"name\":\"bash\",
               \"version\":\"5.3-1\",\"architecture\":\"x86_64\"}'
         )",
    ] {
        assert!(conn.execute(sql, []).is_err(), "{sql}");
    }

    conn.execute(
        "INSERT INTO troves (
             name, version, type, architecture, install_source, install_reason,
             version_scheme, native_package_identity_json
         ) VALUES (
             'perl', '4:5.40.0-1.fc44', 'package', 'x86_64',
             'adopted-full', 'explicit', 'rpm',
             '{\"manager\":\"rpm\",
               \"selector\":\"perl-4:5.40.0-1.fc44.x86_64\",
               \"name\":\"perl\",\"epoch\":4,\"version\":\"5.40.0\",
               \"release\":\"1.fc44\",\"architecture\":\"x86_64\"}'
         )",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO troves (
             name, version, type, architecture, install_source, install_reason,
             source_profile, version_scheme, native_package_identity_json
         ) VALUES (
             'jq', '1.8.2-13', 'package', 'x86_64',
             'adopted-full', 'explicit', 'solus', 'eopkg',
             '{\"manager\":\"eopkg\",\"selector\":\"jq\",\"name\":\"jq\",
               \"version\":\"1.8.2\",\"release\":13,\"architecture\":\"x86_64\"}'
         )",
        [],
    )
    .unwrap();

    for sql in [
        "INSERT INTO troves (
             name, version, type, architecture, install_source, install_reason,
             version_scheme, native_package_identity_json
         ) VALUES (
             'wrong-name', '2.41-12', 'package', 'i386',
             'adopted-track', 'explicit', 'debian',
             '{\"manager\":\"dpkg\",\"selector\":\"libc6:i386\",\"name\":\"libc6\",
               \"version\":\"2.41-12\",\"architecture\":\"i386\"}'
         )",
        "INSERT INTO troves (
             name, version, type, architecture, install_source, install_reason,
             version_scheme, native_package_identity_json
         ) VALUES (
             'libc6', '2.41-11', 'package', 'i386',
             'adopted-track', 'explicit', 'debian',
             '{\"manager\":\"dpkg\",\"selector\":\"libc6:i386\",\"name\":\"libc6\",
               \"version\":\"2.41-12\",\"architecture\":\"i386\"}'
         )",
        "INSERT INTO troves (
             name, version, type, architecture, install_source, install_reason,
             version_scheme, native_package_identity_json
         ) VALUES (
             'libc6', '2.41-12', 'package', 'amd64',
             'adopted-track', 'explicit', 'debian',
             '{\"manager\":\"dpkg\",\"selector\":\"libc6:i386\",\"name\":\"libc6\",
               \"version\":\"2.41-12\",\"architecture\":\"i386\"}'
         )",
        "INSERT INTO troves (
             name, version, type, architecture, install_source, install_reason,
             version_scheme, native_package_identity_json
         ) VALUES (
             'libc6', '2.41-12', 'package', 'i386',
             'adopted-track', 'explicit', 'debian',
             '{\"manager\":\"dpkg\",\"selector\":\"libc6:amd64\",\"name\":\"libc6\",
               \"version\":\"2.41-12\",\"architecture\":\"i386\"}'
         )",
        "INSERT INTO troves (
             name, version, type, architecture, install_source, install_reason,
             source_profile, version_scheme, native_package_identity_json
         ) VALUES (
             'missing-eopkg-release', '1.0-1', 'package', 'x86_64',
             'adopted-full', 'explicit', 'solus', 'eopkg',
             '{\"manager\":\"eopkg\",\"selector\":\"missing-eopkg-release\",
               \"name\":\"missing-eopkg-release\",\"version\":\"1.0\",
               \"architecture\":\"x86_64\"}'
         )",
        "INSERT INTO troves (
             name, version, type, architecture, install_source, install_reason,
             source_profile, version_scheme, native_package_identity_json
         ) VALUES (
             'wrong-eopkg-release', '1.0-2', 'package', 'x86_64',
             'adopted-full', 'explicit', 'solus', 'eopkg',
             '{\"manager\":\"eopkg\",\"selector\":\"wrong-eopkg-release\",
               \"name\":\"wrong-eopkg-release\",\"version\":\"1.0\",\"release\":1,
               \"architecture\":\"x86_64\"}'
         )",
    ] {
        assert!(conn.execute(sql, []).is_err(), "{sql}");
    }
}

#[test]
fn current_schema_separates_installed_and_repository_conversions() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    ensure_current(&conn).unwrap();
    conn.execute(
        "INSERT INTO troves (
             name, version, type, install_source, install_reason, version_scheme
         ) VALUES ('installed', '1', 'package', 'file', 'explicit', 'conary')",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO converted_packages (
             artifact_kind, trove_id, original_format, original_checksum,
             conversion_version
         ) VALUES ('installed', 1, 'rpm', 'sha256:installed', 11)",
        [],
    )
    .unwrap();

    for sql in [
        "INSERT INTO converted_packages (
             artifact_kind, original_format, original_checksum, conversion_version
         ) VALUES ('installed', 'rpm', 'sha256:installed-without-trove', 11)",
        "INSERT INTO converted_packages (
             artifact_kind, trove_id, original_format, original_checksum,
             conversion_version, package_name
         ) VALUES ('installed', 1, 'rpm', 'sha256:installed-with-repo-field', 11, 'pkg')",
        "INSERT INTO converted_packages (
             artifact_kind, original_format, original_checksum, conversion_version,
             package_version, source_profile, transport_json, total_size, content_hash, ccs_path
         ) VALUES (
             'repository', 'rpm', 'sha256:missing-name', 11,
             '1', 'fedora', '{}', 0, 'sha256:content', '/tmp/pkg.ccs'
         )",
        "INSERT INTO converted_packages (
             artifact_kind, original_format, original_checksum, conversion_version,
             package_name, source_profile, transport_json, total_size, content_hash, ccs_path
         ) VALUES (
             'repository', 'rpm', 'sha256:missing-version', 11,
             'pkg', 'fedora', '{}', 0, 'sha256:content', '/tmp/pkg.ccs'
         )",
        "INSERT INTO converted_packages (
             artifact_kind, original_format, original_checksum, conversion_version,
             package_name, package_version, source_profile, total_size, content_hash, ccs_path
         ) VALUES (
             'repository', 'rpm', 'sha256:missing-chunks', 11,
             'pkg', '1', 'fedora', 0, 'sha256:content', '/tmp/pkg.ccs'
         )",
        "INSERT INTO converted_packages (
             artifact_kind, original_format, original_checksum, conversion_version,
             package_name, package_version, source_profile, transport_json, content_hash, ccs_path
         ) VALUES (
             'repository', 'rpm', 'sha256:missing-size', 11,
             'pkg', '1', 'fedora', '{}', 'sha256:content', '/tmp/pkg.ccs'
         )",
        "INSERT INTO converted_packages (
             artifact_kind, original_format, original_checksum, conversion_version,
             package_name, package_version, source_profile, transport_json, total_size, ccs_path
         ) VALUES (
             'repository', 'rpm', 'sha256:missing-hash', 11,
             'pkg', '1', 'fedora', '{}', 0, '/tmp/pkg.ccs'
         )",
        "INSERT INTO converted_packages (
             artifact_kind, original_format, original_checksum, conversion_version,
             package_name, package_version, source_profile, transport_json, total_size, content_hash
         ) VALUES (
             'repository', 'rpm', 'sha256:missing-path', 11,
             'pkg', '1', 'fedora', '{}', 0, 'sha256:content'
         )",
    ] {
        assert!(conn.execute(sql, []).is_err(), "{sql}");
    }
}

#[test]
fn current_schema_persists_only_executable_daemon_job_kinds() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    for kind in ["install", "remove", "update", "enhance"] {
        conn.execute(
            "INSERT INTO daemon_jobs (id, kind, spec_json) VALUES (?1, ?2, '{}')",
            params![format!("job-{kind}"), kind],
        )
        .unwrap();
    }
    for kind in ["dry_run", "rollback", "verify", "garbage_collect", "gc"] {
        assert!(
            conn.execute(
                "INSERT INTO daemon_jobs (id, kind, spec_json) VALUES (?1, ?2, '{}')",
                params![format!("job-{kind}"), kind],
            )
            .is_err(),
            "unsupported daemon job {kind} was persisted"
        );
    }
}

#[test]
fn ensure_current_rejects_every_retired_incremental_schema_version() {
    for version in [1, 79] {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            INSERT INTO schema_version (version) VALUES ({version});"
        ))
        .unwrap();

        let error = ensure_current(&conn).unwrap_err().to_string();
        assert!(
            error.contains(&format!("migration-chain schema version {version}")),
            "{error}"
        );
        assert!(error.contains("schema rebuild required"), "{error}");
    }
}

#[test]
fn ensure_current_rejects_nonempty_unversioned_database() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE stray (id INTEGER PRIMARY KEY)", [])
        .unwrap();

    let error = ensure_current(&conn).unwrap_err().to_string();
    assert!(error.contains("unversioned non-empty"));
    assert!(!table_exists(&conn, "troves").unwrap());
}

#[test]
fn ensure_current_rejects_unversioned_schema_objects_without_tables() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE VIEW stray AS SELECT 1 AS id", [])
        .unwrap();

    let error = ensure_current(&conn).unwrap_err().to_string();
    assert!(error.contains("unversioned non-empty"));
    assert!(error.contains("schema rebuild required"), "{error}");
    assert!(!table_exists(&conn, "troves").unwrap());
}

#[test]
fn ensure_current_rejects_unversioned_indexes_and_triggers() {
    for ddl in [
        "CREATE TABLE stray (id INTEGER PRIMARY KEY);
         CREATE INDEX stray_index ON stray(id);",
        "CREATE TABLE stray (id INTEGER PRIMARY KEY);
         CREATE TRIGGER stray_trigger AFTER INSERT ON stray
         BEGIN
             SELECT NEW.id;
         END;",
    ] {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(ddl).unwrap();

        let error = ensure_current(&conn).unwrap_err().to_string();
        assert!(error.contains("unversioned non-empty"), "{error}");
        assert!(error.contains("schema rebuild required"), "{error}");
        assert!(!table_exists(&conn, "troves").unwrap());
    }
}

#[test]
fn current_schema_has_no_scriptlet_review_queue_objects() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    for name in [
        "scriptlet_evidence_clusters",
        "scriptlet_evidence_cluster_samples",
        "scriptlet_evidence_state_events",
        "scriptlet_evidence_notes",
        "scriptlet_evidence_backfill_runs",
        "idx_scriptlet_evidence_clusters_state_last_seen",
        "idx_scriptlet_evidence_clusters_class",
        "idx_scriptlet_evidence_samples_cluster",
        "idx_scriptlet_evidence_samples_package",
        "idx_scriptlet_evidence_samples_unique_observation",
        "idx_scriptlet_evidence_backfill_status",
    ] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_schema WHERE name = ?1
                )",
                [name],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !exists,
            "retired scriptlet review queue object remains: {name}"
        );
    }

    let mut stmt = conn
        .prepare("PRAGMA table_info('converted_packages')")
        .unwrap();
    let converted_columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!converted_columns.contains(&"conversion_fidelity".to_string()));
    assert!(!converted_columns.contains(&"detected_hooks".to_string()));
}

#[test]
fn current_schema_uses_exact_source_profile_columns() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    for table in [
        "repository_packages",
        "converted_packages",
        "native_package_publications",
        "download_stats",
        "download_counts",
        "delta_manifests",
    ] {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info('{table}')"))
            .unwrap();
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            columns.contains(&"source_profile".to_string()),
            "{table} must persist exact source_profile authority"
        );
        assert!(
            !columns.contains(&"distro".to_string()),
            "{table} retains the ambiguous distro column"
        );
    }
}

#[test]
fn current_schema_enforces_typed_unique_rollback_lineage() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut mutation = crate::db::models::Changeset::new("forward mutation".to_string());
    let mutation_id = mutation.insert(&conn).unwrap();
    let mut rollback =
        crate::db::models::Changeset::new_rollback("compensation".to_string(), mutation_id);
    let rollback_id = rollback.insert(&conn).unwrap();
    let stored = crate::db::models::Changeset::find_by_id(&conn, rollback_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.kind, crate::db::models::ChangesetKind::Rollback);
    assert_eq!(stored.reverts_changeset_id, Some(mutation_id));

    let mut duplicate =
        crate::db::models::Changeset::new_rollback("duplicate".to_string(), mutation_id);
    assert!(duplicate.insert(&conn).is_err());
    assert!(
        conn.execute(
            "INSERT INTO changesets (description, kind, status)
             VALUES ('unbound rollback', 'rollback', 'pending')",
            [],
        )
        .is_err()
    );
}
