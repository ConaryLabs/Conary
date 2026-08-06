// apps/conary/src/commands/system/tests/rollback.rs

use super::*;

#[path = "rollback/directories.rs"]
mod directories;
#[path = "rollback/generation.rs"]
mod generation;
#[path = "rollback/lineage.rs"]
mod lineage;

fn resolved_node(kind: PayloadNodeKind, mode: u32) -> ResolvedPayloadNode {
    ResolvedPayloadNode::from_numeric_source(PayloadNode {
        kind,
        mode,
        user: PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::geteuid() }),
        },
        group: PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::getegid() }),
        },
        mtime: PayloadTimestamp::UNIX_EPOCH,
        xattrs: BTreeMap::new(),
    })
    .unwrap()
}

fn empty_rollback_root() -> CapturedSelectedRoot {
    CapturedSelectedRoot {
        generation: GenerationRootManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            root: resolved_node(PayloadNodeKind::Directory, libc::S_IFDIR | 0o755),
            entries: Vec::new(),
        },
        state: MutableStateManifest::empty(),
    }
}

fn active_rollback_root(db_path: &str) -> CapturedSelectedRoot {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    let generation = conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap()
        .unwrap();
    let artifact = conary_core::generation::artifact::load_generation_artifact(
        &runtime_root.generation_path(generation),
    )
    .unwrap();
    CapturedSelectedRoot {
        generation: artifact.generation_root,
        state: artifact.mutable_state,
    }
}

fn with_usr_bin_file(
    mut captured: CapturedSelectedRoot,
    path: &str,
    sha256: String,
    size: u64,
) -> CapturedSelectedRoot {
    for directory in ["/usr", "/usr/bin"] {
        captured.generation.entries.push(GenerationRootEntry {
            path: directory.to_string(),
            node: resolved_node(PayloadNodeKind::Directory, libc::S_IFDIR | 0o755),
            content: None,
        });
    }
    captured.generation.entries.push(GenerationRootEntry {
        path: path.to_string(),
        node: resolved_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            libc::S_IFREG | 0o755,
        ),
        content: Some(PayloadContentAuthority { sha256, size }),
    });
    captured
        .generation
        .entries
        .sort_by(|left, right| left.path.cmp(&right.path));
    captured.generation.validate().unwrap();
    captured
}

fn regular_snapshot(path: &str, sha256: String, size: u64, mode: u32) -> FileSnapshot {
    FileSnapshot {
        path: path.to_string(),
        node: resolved_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            libc::S_IFREG | (mode & 0o7777),
        ),
        content: Some(PayloadContentAuthority { sha256, size }),
        installed_at: "2026-01-01T00:00:00Z".to_string(),
        component: None,
    }
}

fn symlink_snapshot(path: &str, target: &str) -> FileSnapshot {
    FileSnapshot {
        path: path.to_string(),
        node: resolved_node(
            PayloadNodeKind::Symlink {
                target: target.to_string(),
            },
            libc::S_IFLNK | 0o777,
        ),
        content: None,
        installed_at: "2026-01-01T00:00:00Z".to_string(),
        component: None,
    }
}

fn store_test_object(conn: &rusqlite::Connection, db_path: &Path, content: &[u8]) -> String {
    let cas = CasStore::new(objects_dir(&db_path.to_string_lossy())).unwrap();
    let hash = cas.store(content).unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size)
             VALUES (?1, ?2, ?3)",
        params![
            &hash,
            format!("objects/{}/{}", &hash[0..2], &hash[2..]),
            content.len() as i64
        ],
    )
    .unwrap();
    hash
}

fn insert_test_trove(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    name: &str,
    version: &str,
    files: &[(&str, &str, i64)],
) -> i64 {
    let mut trove = Trove::new_with_source(
        name.to_string(),
        version.to_string(),
        TroveType::Package,
        InstallSource::File,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    trove.installed_by_changeset_id = Some(changeset_id);
    let trove_id = trove.insert(conn).unwrap();

    for (path, hash, size) in files {
        let mut file = FileEntry::new(
            (*path).to_string(),
            resolved_node(
                PayloadNodeKind::Regular {
                    hardlink_identity: None,
                },
                libc::S_IFREG | 0o644,
            ),
            Some(PayloadContentAuthority {
                sha256: (*hash).to_string(),
                size: u64::try_from(*size).unwrap(),
            }),
            trove_id,
        );
        file.insert(conn).unwrap();
    }

    trove_id
}

fn rollback_typed_rpm_entry() -> NativeLifecycleEntry {
    let body = "print('rollback-post-remove')\n";
    NativeLifecycleEntry {
        id: "rpm:%postun".to_string(),
        native_slot: "%postun".to_string(),
        kind: NativeLifecycleEntryKind::Executable,
        phase: LifecyclePath::PostRemove,
        lifecycle_paths: vec!["remove:last".to_string()],
        interpreter: "<lua>".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "after-payload".to_string(),
            before: Vec::new(),
            after: vec!["payload".to_string()],
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        rpm_trigger: None,
        rpm_runtime: Some(RpmRuntimeMetadata {
            program: RpmProgram::EmbeddedLua,
            body_transforms: Vec::new(),
            critical: true,
            criticality: RpmCriticality::Header,
            raw_flags: 0,
            unknown_flags: 0,
            install_prefixes: Vec::new(),
            macro_context: Default::default(),
            header_context: Default::default(),
            package_rpm_version: None,
        }),
        rpm_sysusers: None,
        deb_maintainer: None,
        arch_install: None,
        arch_hook: None,
        residual_lifecycle: None,
    }
}

fn rollback_typed_rpm_bundle() -> NativeLifecycleBundle {
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: conary_core::ccs::native_lifecycle::NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora".to_string(),
        source_profile: Some("fedora-44".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: "rollback-rpm-fixture".to_string(),
        source_version: "1.0-1".to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "typed-runtime-test".to_string(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(b"rollback-evidence")),
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![rollback_typed_rpm_entry()],
    }
}

fn set_trove_rpm_provenance(conn: &rusqlite::Connection, trove_id: i64) {
    conn.execute(
        "UPDATE troves SET source_profile = 'fedora-44', version_scheme = 'rpm' WHERE id = ?1",
        [trove_id],
    )
    .unwrap();
}

fn record_additive_changeset_with_system_authority(
    conn: &rusqlite::Connection,
    db_path: &str,
    authority: RollbackSystemAuthority,
    fixture: &str,
) -> i64 {
    let mut changeset = Changeset::new(format!("Install {fixture}"));
    let changeset_id = changeset.insert(conn).unwrap();
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        params![
            metadata_with_removed_troves(
                Vec::new(),
                Vec::new(),
                active_rollback_root(db_path),
                authority,
            )
            .unwrap(),
            changeset_id,
        ],
    )
    .unwrap();
    changeset
        .update_status(conn, ChangesetStatus::Applied)
        .unwrap();
    insert_test_trove(conn, changeset_id, fixture, "1.0.0", &[]);
    changeset_id
}

#[tokio::test]
async fn additive_rollback_restores_unaffected_native_runtime_projection() {
    let (_temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(Path::new(&db_path_str), 1);
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let mut peer = Trove::new(
        "native-runtime-peer".to_string(),
        "1.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    peer.architecture = Some("x86_64".to_string());
    let peer_id = peer.insert(&conn).unwrap();
    let mut runtime =
        InstalledNativeLifecycleBundle::new(peer_id, None, &rollback_typed_rpm_bundle()).unwrap();
    runtime.lifecycle_state =
        conary_core::ccs::native_transaction::DebPackageState::TriggersAwaited;
    runtime.pending_triggers = vec!["before-trigger".to_string()];
    runtime.awaited_packages = vec![
        conary_core::ccs::native_transaction::NativePackageIdentity::new(
            "before-peer",
            "2.0",
            Some("x86_64"),
        ),
    ];
    runtime.insert_or_replace(&conn).unwrap();
    let authority = RollbackSystemAuthority::capture(&conn).unwrap();
    let changeset_id = record_additive_changeset_with_system_authority(
        &conn,
        &db_path_str,
        authority,
        "native-runtime-addition",
    );
    conn.execute(
        "UPDATE installed_native_lifecycle_bundles
         SET lifecycle_state = 'installed',
             pending_triggers_json = '[]',
             awaited_packages_json = '[]'
         WHERE trove_id = ?1",
        [peer_id],
    )
    .unwrap();
    drop(conn);

    cmd_rollback(changeset_id, &db_path_str).await.unwrap();

    let conn = conary_core::db::open(&db_path_str).unwrap();
    let restored = InstalledNativeLifecycleBundle::find_by_trove(&conn, peer_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        restored.lifecycle_state,
        conary_core::ccs::native_transaction::DebPackageState::TriggersAwaited
    );
    assert_eq!(restored.pending_triggers, vec!["before-trigger"]);
    assert_eq!(restored.awaited_packages[0].package_name, "before-peer");
}

#[tokio::test]
async fn additive_rollback_restores_unaffected_config_runtime_projection() {
    let (_temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(Path::new(&db_path_str), 1);
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let mut peer = Trove::new(
        "config-runtime-peer".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let peer_id = peer.insert(&conn).unwrap();
    let mut config = conary_core::db::models::ConfigFile::new(
        "/etc/config-runtime-peer.conf".to_string(),
        peer_id,
        "a".repeat(64),
    );
    config.current_hash = Some("b".repeat(64));
    config.status = conary_core::db::models::ConfigStatus::Modified;
    config.modified_at = Some("2026-01-02T03:04:05Z".to_string());
    config.insert(&conn).unwrap();
    let authority = RollbackSystemAuthority::capture(&conn).unwrap();
    let changeset_id = record_additive_changeset_with_system_authority(
        &conn,
        &db_path_str,
        authority,
        "config-runtime-addition",
    );
    conn.execute(
        "UPDATE config_files
         SET current_hash = NULL, status = 'missing', modified_at = NULL
         WHERE path = '/etc/config-runtime-peer.conf'",
        [],
    )
    .unwrap();
    drop(conn);

    cmd_rollback(changeset_id, &db_path_str).await.unwrap();

    let conn = conary_core::db::open(&db_path_str).unwrap();
    let restored =
        conary_core::db::models::ConfigFile::find_by_path(&conn, "/etc/config-runtime-peer.conf")
            .unwrap()
            .unwrap();
    assert_eq!(restored.current_hash, Some("b".repeat(64)));
    assert_eq!(
        restored.status,
        conary_core::db::models::ConfigStatus::Modified
    );
    assert_eq!(
        restored.modified_at.as_deref(),
        Some("2026-01-02T03:04:05Z")
    );
}

#[tokio::test]
async fn additive_rollback_restores_unaffected_derived_runtime_projection() {
    let (_temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(Path::new(&db_path_str), 1);
    let conn = conary_core::db::open(&db_path_str).unwrap();

    conn.execute(
        "INSERT INTO derived_packages (name, parent_name, status, updated_at)
         VALUES ('derived-runtime-peer', 'external-parent', 'built',
                 '2026-01-03T04:05:06Z')",
        [],
    )
    .unwrap();
    let authority = RollbackSystemAuthority::capture(&conn).unwrap();
    let changeset_id = record_additive_changeset_with_system_authority(
        &conn,
        &db_path_str,
        authority,
        "derived-runtime-addition",
    );
    conn.execute(
        "UPDATE derived_packages
         SET status = 'stale', updated_at = '2030-01-01T00:00:00Z'
         WHERE name = 'derived-runtime-peer'",
        [],
    )
    .unwrap();
    drop(conn);

    cmd_rollback(changeset_id, &db_path_str).await.unwrap();

    let conn = conary_core::db::open(&db_path_str).unwrap();
    let restored: (String, String) = conn
        .query_row(
            "SELECT status, updated_at FROM derived_packages
             WHERE name = 'derived-runtime-peer'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        restored,
        ("built".to_string(), "2026-01-03T04:05:06Z".to_string())
    );
}

#[tokio::test]
async fn rollback_without_v5_authority_fails_closed_and_remains_retryable() {
    let (_temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(
        std::path::Path::new(&db_path_str),
        1,
    );
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let mut changeset = Changeset::new("Install rollback RPM fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();
    let trove_id = insert_test_trove(&conn, changeset_id, "rollback-rpm-fixture", "1.0.1", &[]);
    set_trove_rpm_provenance(&conn, trove_id);
    drop(conn);

    for _ in 0..2 {
        let error = cmd_rollback(changeset_id, &db_path_str)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("no exact v5 rollback authority"), "{error}");
    }

    let conn = conary_core::db::open(&db_path_str).unwrap();
    assert!(
        Trove::find_by_id(&conn, trove_id).unwrap().is_some(),
        "failed authority validation must not remove installed state"
    );
    let reversed_by: Option<i64> = conn
        .query_row(
            "SELECT reversed_by_changeset_id FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reversed_by, None);
}

#[tokio::test]
async fn rollback_with_retired_metadata_schema_fails_closed_and_remains_retryable() {
    let (_temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(
        std::path::Path::new(&db_path_str),
        1,
    );
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let mut changeset = Changeset::new("Install retired metadata fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        params![
            r#"{"schema":"conary.changeset.metadata.v5","removed_troves":[]}"#,
            changeset_id
        ],
    )
    .unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();
    let trove_id = insert_test_trove(
        &conn,
        changeset_id,
        "retired-metadata-fixture",
        "1.0.0",
        &[],
    );
    drop(conn);

    for _ in 0..2 {
        let error = cmd_rollback(changeset_id, &db_path_str)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Unsupported changeset metadata schema")
                && error.contains("conary.changeset.metadata.v6"),
            "{error}"
        );
    }

    let conn = conary_core::db::open(&db_path_str).unwrap();
    assert!(Trove::find_by_id(&conn, trove_id).unwrap().is_some());
    let reversed_by: Option<i64> = conn
        .query_row(
            "SELECT reversed_by_changeset_id FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reversed_by, None);
}

#[tokio::test]
async fn rollback_snapshot_restores_exact_prior_typed_rpm_authority() {
    let (_temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(
        std::path::Path::new(&db_path_str),
        1,
    );
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let mut old_bundle = rollback_typed_rpm_bundle();
    old_bundle.source_version = "0.9-1".to_string();
    let mut old_snapshot = TroveSnapshot::test_package("rollback-rpm-fixture", "0.9-1", Vec::new());
    old_snapshot.architecture = Some("x86_64".to_string());
    old_snapshot.source_profile = Some("fedora-44".to_string());
    old_snapshot.version_scheme = conary_core::repository::versioning::VersionScheme::Rpm;
    old_snapshot.native_lifecycle = Some(NativeLifecycleSnapshot {
        source_format: old_bundle.source_format.as_str().to_string(),
        source_family: old_bundle.source_family.clone(),
        source_profile: old_bundle.source_profile.clone(),
        source_release: old_bundle.source_release.clone(),
        source_arch: old_bundle.source_arch.clone(),
        source_package: old_bundle.source_package.clone(),
        source_version: old_bundle.source_version.clone(),
        scriptlet_fidelity: old_bundle.scriptlet_fidelity.as_str().to_string(),
        evidence_digest: old_bundle.evidence_digest.clone(),
        bundle_toml: toml::to_string_pretty(&old_bundle).unwrap(),
        lifecycle_state: "installed".to_string(),
        pending_triggers: Vec::new(),
        awaited_packages: Vec::new(),
        installed_at: "2026-01-01T00:00:00Z".to_string(),
    });

    let mut changeset = Changeset::new("Upgrade rollback RPM fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        params![
            metadata_with_removed_troves(
                vec![old_snapshot],
                Vec::new(),
                active_rollback_root(&db_path_str),
                RollbackSystemAuthority::default(),
            )
            .unwrap(),
            changeset_id
        ],
    )
    .unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();
    let trove_id = insert_test_trove(&conn, changeset_id, "rollback-rpm-fixture", "1.0.1", &[]);
    set_trove_rpm_provenance(&conn, trove_id);
    let bundle = rollback_typed_rpm_bundle();
    let mut installed =
        InstalledNativeLifecycleBundle::new(trove_id, Some(changeset_id), &bundle).unwrap();
    installed.insert_or_replace(&conn).unwrap();
    drop(conn);

    cmd_rollback(changeset_id, &db_path_str)
        .await
        .expect("snapshot rollback must restore exact prior typed RPM authority");

    let conn = conary_core::db::open(&db_path_str).unwrap();
    assert!(
        Trove::find_by_id(&conn, trove_id).unwrap().is_none(),
        "snapshot rollback must remove the reverted trove"
    );
    assert!(
        InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .is_none(),
        "snapshot rollback must remove the reverted lifecycle bundle"
    );
    let restored = Trove::find_by_name(&conn, "rollback-rpm-fixture").unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].version, "0.9-1");
    assert_eq!(restored[0].source_profile.as_deref(), Some("fedora-44"));
    assert_eq!(
        restored[0].version_scheme,
        conary_core::repository::versioning::VersionScheme::Rpm
    );
    let restored_bundle = InstalledNativeLifecycleBundle::find_by_trove(
        &conn,
        restored[0].id.expect("restored trove identity"),
    )
    .unwrap()
    .expect("rollback must restore the previous native lifecycle bundle");
    assert_eq!(restored_bundle.lifecycle_state.as_str(), "installed");
    assert_eq!(restored_bundle.bundle().unwrap().source_version, "0.9-1");
    let reversed_by: Option<i64> = conn
        .query_row(
            "SELECT reversed_by_changeset_id FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(reversed_by.is_some());
}

#[tokio::test]
async fn rollback_publishes_exact_pre_upgrade_parent_directory_authority() {
    let (_temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let db_path = Path::new(&db_path_str);
    crate::commands::test_helpers::create_active_test_generation(db_path, 1);
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let v1_bytes = b"first generation payload";
    let v2_bytes = b"second generation payload";
    let v1_hash = store_test_object(&conn, db_path, v1_bytes);
    let v2_hash = store_test_object(&conn, db_path, v2_bytes);
    let base = active_rollback_root(&db_path_str);
    let rollback_root = with_usr_bin_file(
        base.clone(),
        "/usr/bin/first-generation",
        v1_hash.clone(),
        v1_bytes.len() as u64,
    );
    let current_root = with_usr_bin_file(
        base,
        "/usr/bin/first-generation",
        v2_hash.clone(),
        v2_bytes.len() as u64,
    );

    let mut changeset = Changeset::new("Upgrade first-generation fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    let mut old_snapshot = TroveSnapshot::test_package(
        "first-generation-fixture",
        "1.0.0",
        vec![regular_snapshot(
            "/usr/bin/first-generation",
            v1_hash.clone(),
            v1_bytes.len() as u64,
            0o755,
        )],
    );
    old_snapshot.architecture = Some("x86_64".to_string());
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        params![
            metadata_with_removed_troves(
                vec![old_snapshot],
                Vec::new(),
                rollback_root.clone(),
                RollbackSystemAuthority::default(),
            )
            .unwrap(),
            changeset_id
        ],
    )
    .unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();
    insert_test_trove(
        &conn,
        changeset_id,
        "first-generation-fixture",
        "2.0.0",
        &[("/usr/bin/first-generation", &v2_hash, v2_bytes.len() as i64)],
    );

    let built = crate::commands::composefs_ops::build_captured_generation_for_publication(
        &conn,
        &db_path_str,
        "current second generation",
        &[],
        current_root,
    )
    .unwrap();
    crate::commands::composefs_ops::publish_generation_link(&db_path_str, built.generation_number)
        .unwrap();
    crate::commands::composefs_ops::mark_generation_state_active(&conn, built.generation_number)
        .unwrap();
    drop(conn);

    cmd_rollback(changeset_id, &db_path_str)
        .await
        .expect("rollback must publish the exact pre-upgrade selected root");

    let conn = conary_core::db::open(&db_path_str).unwrap();
    let restored = Trove::find_by_name(&conn, "first-generation-fixture").unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].version, "1.0.0");
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    let current_generation =
        conary_core::generation::mount::current_generation(runtime_root.root())
            .unwrap()
            .unwrap();
    assert!(current_generation > built.generation_number);
    let artifact = conary_core::generation::artifact::load_generation_artifact(
        &runtime_root.generation_path(current_generation),
    )
    .unwrap();
    assert_eq!(artifact.generation_root, rollback_root.generation);
    assert_eq!(artifact.mutable_state, rollback_root.state);
    let restored_file = artifact
        .generation_root
        .entries
        .iter()
        .find(|entry| entry.path == "/usr/bin/first-generation")
        .unwrap();
    assert_eq!(
        restored_file.content.as_ref().unwrap().sha256,
        v1_hash,
        "rollback generation must use first-generation content authority"
    );
}

#[tokio::test]
async fn rollback_update_without_active_generation_fails_closed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_string_lossy().to_string();
    conary_core::db::init(&db_path_str).unwrap();
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let root = temp_dir.path().join("root");
    std::fs::create_dir_all(root.join("usr/share/conary-test")).unwrap();
    let hello_path = root.join("usr/share/conary-test/hello.txt");
    let added_path = root.join("usr/share/conary-test/added.txt");
    std::fs::write(&hello_path, b"hello from v2\n").unwrap();
    std::fs::write(&added_path, b"added in v2\n").unwrap();

    let v1_hash = store_test_object(&conn, &db_path, b"hello from v1\n");
    let v2_hash = store_test_object(&conn, &db_path, b"hello from v2\n");
    let added_hash = store_test_object(&conn, &db_path, b"added in v2\n");

    let mut old_snapshot = TroveSnapshot::test_package(
        "conary-test-fixture",
        "1.0.0",
        vec![regular_snapshot(
            "/usr/share/conary-test/hello.txt",
            v1_hash,
            "hello from v1\n".len() as u64,
            0o644,
        )],
    );
    old_snapshot.architecture = Some("x86_64".to_string());

    let mut update_changeset =
        Changeset::new("CCS upgrade conary-test-fixture 1.0.0 -> 2.0.0".to_string());
    let update_changeset_id = update_changeset.insert(&conn).unwrap();
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        params![
            metadata_with_removed_troves(
                vec![old_snapshot],
                Vec::new(),
                empty_rollback_root(),
                RollbackSystemAuthority::default(),
            )
            .unwrap(),
            update_changeset_id
        ],
    )
    .unwrap();
    update_changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();

    insert_test_trove(
        &conn,
        update_changeset_id,
        "conary-test-fixture",
        "2.0.0",
        &[
            (
                "/usr/share/conary-test/hello.txt",
                &v2_hash,
                "hello from v2\n".len() as i64,
            ),
            (
                "/usr/share/conary-test/added.txt",
                &added_hash,
                "added in v2\n".len() as i64,
            ),
        ],
    );
    drop(conn);

    let err = cmd_rollback(update_changeset_id, &db_path_str)
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains(&format!(
        "Cannot roll back changeset {update_changeset_id} without an active composefs generation"
    )));
    assert_eq!(
        std::fs::read_to_string(&hello_path).unwrap(),
        "hello from v2\n"
    );
    assert!(added_path.exists());

    let conn = conary_core::db::open(&db_path_str).unwrap();
    let troves = Trove::find_by_name(&conn, "conary-test-fixture").unwrap();
    assert_eq!(troves.len(), 1);
    assert_eq!(troves[0].version, "2.0.0");
    let status: String = conn
        .query_row(
            "SELECT status FROM changesets WHERE id = ?1",
            [update_changeset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "applied");
}

#[test]
fn direct_live_root_restore_recreates_regular_files_and_symlinks() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_string_lossy().to_string();
    conary_core::db::init(&db_path_str).unwrap();
    let conn = conary_core::db::open(&db_path_str).unwrap();
    let file_hash = store_test_object(&conn, &db_path, b"restored\n");
    let link_hash = {
        let cas = CasStore::new(objects_dir(&db_path_str)).unwrap();
        cas.store_symlink("tool").unwrap()
    };
    conn.execute(
        "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size)
             VALUES (?1, ?2, ?3)",
        params![
            &link_hash,
            format!("objects/{}/{}", &link_hash[0..2], &link_hash[2..]),
            "tool".len() as i64
        ],
    )
    .unwrap();

    let root = temp_dir.path().join("root");
    let snapshot = TroveSnapshot::test_package(
        "fixture",
        "1.0.0",
        vec![
            regular_snapshot("/usr/bin/tool", file_hash, "restored\n".len() as u64, 0o755),
            symlink_snapshot("/usr/bin/tool-link", "tool"),
        ],
    );

    let stats = restore_snapshots_to_live_root(&root, &db_path_str, &[snapshot]).unwrap();

    assert_eq!(stats.files_restored, 2);
    assert_eq!(
        std::fs::read_to_string(root.join("usr/bin/tool")).unwrap(),
        "restored\n"
    );
    assert_eq!(
        std::fs::read_link(root.join("usr/bin/tool-link")).unwrap(),
        Path::new("tool")
    );
}

#[test]
fn rollback_snapshot_restores_exact_ccs_remove_hook() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let mut changeset = Changeset::new("Restore CCS hook fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    let mut snapshot = TroveSnapshot::test_package("ccs-hook-fixture", "1.0.0", Vec::new());
    snapshot.ccs_remove_hook = Some(crate::commands::CcsRemoveHookSnapshot {
        script: "echo removing\n".to_string(),
        reversible: Some(true),
    });

    let tx = conn.transaction().unwrap();
    restore_snapshot(&tx, changeset_id, &snapshot).unwrap();
    tx.commit().unwrap();

    let trove = Trove::find_by_name(&conn, "ccs-hook-fixture")
        .unwrap()
        .pop()
        .unwrap();
    let hook =
        conary_core::db::models::InstalledCcsRemoveHook::find_by_trove(&conn, trove.id.unwrap())
            .unwrap()
            .unwrap();
    assert_eq!(hook.script, "echo removing\n");
    assert_eq!(hook.reversible, Some(true));
}
