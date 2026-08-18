// apps/conary/src/commands/generation/config_transaction/tests.rs
//! Exact generation config capture, planning, and materialization tests.

use super::*;
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadTimestamp, ResolvedPayloadNode,
};
use std::collections::BTreeMap;

fn resolved_node(kind: PayloadNodeKind, mode: u32) -> ResolvedPayloadNode {
    ResolvedPayloadNode::from_numeric_source(PayloadNode {
        kind,
        mode,
        user: PayloadIdentity::Numeric { id: 0 },
        group: PayloadIdentity::Numeric { id: 0 },
        mtime: PayloadTimestamp::UNIX_EPOCH,
        xattrs: BTreeMap::new(),
    })
    .unwrap()
}

fn regular(content: &[u8], mode: u32) -> ConfigArtifact {
    ConfigArtifact::regular(
        PayloadContentAuthority {
            sha256: conary_core::hash::sha256(content),
            size: content.len() as u64,
        },
        resolved_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            mode,
        ),
    )
    .unwrap()
}

fn symlink(target: &str, mode: u32) -> ConfigArtifact {
    ConfigArtifact::symlink(
        target.to_string(),
        resolved_node(
            PayloadNodeKind::Symlink {
                target: target.to_string(),
            },
            mode,
        ),
    )
    .unwrap()
}

fn regular_fixture_from_path(path: &Path) -> ConfigArtifact {
    let content = fs::read(path).unwrap();
    let node = conary_core::generation::root_manifest::capture_existing_payload_node(path).unwrap();
    ConfigArtifact::regular(
        PayloadContentAuthority {
            sha256: conary_core::hash::sha256(&content),
            size: content.len() as u64,
        },
        node,
    )
    .unwrap()
}

fn state(source: ConfigSource, noreplace: bool, content: &[u8]) -> ConfigPackageState {
    let artifact = regular(content, 0o100640);
    ConfigPackageState {
        source,
        noreplace,
        ghost: false,
        materialized: true,
        original_sha256: Some(artifact.sha256().to_string()),
        artifact: Some(artifact),
    }
}

fn source_config(source: ConfigSource, path: &str) -> SourceConfigDeclaration {
    match source {
        ConfigSource::Arch => SourceConfigDeclaration::Alpm(
            conary_core::packages::arch::authority::AlpmConfigDeclaration {
                pkginfo_index: 0,
                source_path: path.trim_start_matches('/').to_string(),
                path: path.to_string(),
                installed_hash: None,
                payload: ConfigPayloadAssociation::Matched,
            },
        ),
        ConfigSource::Deb => SourceConfigDeclaration::Debian(
            conary_core::packages::deb::authority::DebianConfigDeclaration {
                control_index: 0,
                path: path.to_string(),
                remove_on_upgrade: false,
                payload: ConfigPayloadAssociation::Matched,
            },
        ),
        ConfigSource::Rpm => SourceConfigDeclaration::Rpm(
            conary_core::packages::rpm::authority::RpmConfigDeclaration {
                header_index: 0,
                path: path.to_string(),
                noreplace: true,
                ghost: false,
                missing_ok: false,
                payload: ConfigPayloadAssociation::Matched,
            },
        ),
        ConfigSource::Eopkg => SourceConfigDeclaration::Eopkg(
            conary_core::packages::eopkg::authority::EopkgConfigDeclaration {
                files_index: 0,
                path: path.to_string(),
                permanent: true,
                payload: ConfigPayloadAssociation::Matched,
            },
        ),
        ConfigSource::Auto => SourceConfigDeclaration::Ccs(
            conary_core::packages::config_authority::CcsConfigDeclaration {
                path: path.to_string(),
                noreplace: true,
                payload: ConfigPayloadAssociation::Matched,
            },
        ),
    }
}

fn deb_remove(path: &str) -> SourceConfigDeclaration {
    SourceConfigDeclaration::Debian(
        conary_core::packages::deb::authority::DebianConfigDeclaration {
            control_index: 0,
            path: path.to_string(),
            remove_on_upgrade: true,
            payload: ConfigPayloadAssociation::Absent,
        },
    )
}

fn alpm_absent(path: &str) -> SourceConfigDeclaration {
    SourceConfigDeclaration::Alpm(
        conary_core::packages::arch::authority::AlpmConfigDeclaration {
            pkginfo_index: 0,
            source_path: path.trim_start_matches('/').to_string(),
            path: path.to_string(),
            installed_hash: None,
            payload: ConfigPayloadAssociation::Absent,
        },
    )
}

#[test]
fn unmatched_alpm_declaration_then_matching_payload_preserves_native_sequence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc/demo")).unwrap();
    fs::write(root.join("etc/demo/declared.conf"), b"user-created").unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let cas = CasStore::new(temp.path().join("objects")).unwrap();

    let transaction = capture_install(
        &conn,
        &root,
        &cas,
        ConfigInstallCapture {
            source: ConfigSource::Arch,
            declared: &[alpm_absent("/etc/demo/declared.conf")],
            incoming: &[],
            replacing_trove_id: None,
            replaced_trove_ids: &[],
        },
    )
    .unwrap();

    assert_eq!(transaction.entries.len(), 1);
    let entry = &transaction.entries[0];
    assert_eq!(
        entry.current.as_ref().unwrap().sha256(),
        conary_core::hash::sha256(b"user-created")
    );
    let after = entry.after.as_ref().unwrap();
    assert_eq!(after.source, ConfigSource::Arch);
    assert!(!after.materialized);
    assert!(after.artifact.is_none());
    assert!(plan_entry(entry).unwrap().is_empty());

    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Arch,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut config = ConfigFile::new_declaration("/etc/demo/declared.conf".to_string(), trove_id);
    config.source = ConfigSource::Arch;
    config.insert(&conn).unwrap();
    project_persisted_statuses(&conn, &[transaction]).unwrap();
    let projected = ConfigFile::find_by_path(&conn, "/etc/demo/declared.conf")
        .unwrap()
        .unwrap();
    assert_eq!(projected.status, ConfigStatus::Missing);
    assert!(projected.current_hash.is_none());

    let incoming = crate::commands::LiveRootFile {
        path: "/etc/demo/declared.conf".to_string(),
        content: crate::commands::LiveRootContent::from_in_memory_bytes(b"packaged-v2"),
        node: resolved_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            0o100644,
        ),
    };
    let materialized = capture_install(
        &conn,
        &root,
        &cas,
        ConfigInstallCapture {
            source: ConfigSource::Arch,
            declared: &[source_config(ConfigSource::Arch, "/etc/demo/declared.conf")],
            incoming: &[incoming],
            replacing_trove_id: Some(trove_id),
            replaced_trove_ids: &[trove_id],
        },
    )
    .unwrap();
    let entry = &materialized.entries[0];
    assert!(!entry.before.as_ref().unwrap().materialized);
    let packaged = entry.after.as_ref().unwrap().artifact.as_ref().unwrap();
    assert_eq!(
        plan_entry(entry).unwrap(),
        vec![
            overlay_write(
                "/etc/demo/declared.conf".to_string(),
                entry.current.clone().unwrap(),
            ),
            overlay_write(
                "/etc/demo/declared.conf.pacnew".to_string(),
                packaged.clone(),
            ),
        ]
    );
}

#[test]
fn alpm_materialized_to_absent_preserves_modified_content_and_rolls_back_exactly() {
    let before = state(ConfigSource::Arch, true, b"packaged-v1");
    let current = regular(b"locally-modified", 0o100600);
    let transaction = GenerationConfigTransaction {
        entries: vec![ConfigPathTransaction {
            path: "/etc/demo/declared.conf".to_string(),
            operation: ConfigTransactionOperation::Dematerialize,
            before: Some(before),
            current: Some(current.clone()),
            after: Some(ConfigPackageState {
                source: ConfigSource::Arch,
                noreplace: true,
                ghost: false,
                materialized: false,
                original_sha256: None,
                artifact: None,
            }),
            auxiliaries: Vec::new(),
        }],
        ..Default::default()
    };

    transaction.validate().unwrap();
    assert_eq!(
        plan_entry(&transaction.entries[0]).unwrap(),
        vec![
            overlay_write(
                "/etc/demo/declared.conf.pacsave".to_string(),
                current.clone(),
            ),
            OverlayMutation::Remove("/etc/demo/declared.conf".to_string()),
        ]
    );

    let rollback = transaction.restore_snapshot().unwrap();
    assert_eq!(
        plan_entry(&rollback.entries[0]).unwrap(),
        vec![
            OverlayMutation::Remove("/etc/demo/declared.conf".to_string()),
            OverlayMutation::Remove("/etc/demo/declared.conf.conary-new".to_string()),
            OverlayMutation::Remove("/etc/demo/declared.conf.conary-save".to_string()),
            OverlayMutation::Remove("/etc/demo/declared.conf.dpkg-dist".to_string()),
            OverlayMutation::Remove("/etc/demo/declared.conf.dpkg-old".to_string()),
            OverlayMutation::Remove("/etc/demo/declared.conf.newconfig".to_string()),
            OverlayMutation::Remove("/etc/demo/declared.conf.pacnew".to_string()),
            OverlayMutation::Remove("/etc/demo/declared.conf.pacsave".to_string()),
            OverlayMutation::Remove("/etc/demo/declared.conf.rpmnew".to_string()),
            OverlayMutation::Remove("/etc/demo/declared.conf.rpmorig".to_string()),
            OverlayMutation::Remove("/etc/demo/declared.conf.rpmsave".to_string()),
            overlay_write("/etc/demo/declared.conf".to_string(), current),
        ]
    );
}

#[test]
fn rpm_materialized_to_ghost_preserves_current_and_rolls_back_exactly() {
    let before = state(ConfigSource::Rpm, true, b"packaged-v1");
    let current = regular(b"locally-modified", 0o100600);
    let transaction = GenerationConfigTransaction {
        entries: vec![ConfigPathTransaction {
            path: "/etc/demo/ghost.conf".to_string(),
            operation: ConfigTransactionOperation::Dematerialize,
            before: Some(before),
            current: Some(current.clone()),
            after: Some(ConfigPackageState {
                source: ConfigSource::Rpm,
                noreplace: true,
                ghost: true,
                materialized: false,
                original_sha256: None,
                artifact: None,
            }),
            auxiliaries: Vec::new(),
        }],
        ..Default::default()
    };

    transaction.validate().unwrap();
    assert!(plan_entry(&transaction.entries[0]).unwrap().is_empty());

    let rollback = transaction.restore_snapshot().unwrap();
    let mutations = plan_entry(&rollback.entries[0]).unwrap();
    assert!(mutations.contains(&overlay_write("/etc/demo/ghost.conf".to_string(), current)));
}

fn update_entry(source: ConfigSource, noreplace: bool) -> ConfigPathTransaction {
    ConfigPathTransaction {
        path: "/etc/demo.conf".to_string(),
        operation: ConfigTransactionOperation::Install,
        before: Some(state(source, noreplace, b"old")),
        current: Some(regular(b"local", 0o100600)),
        after: Some(state(source, noreplace, b"new")),
        auxiliaries: Vec::new(),
    }
}

#[test]
fn capture_update_records_exact_old_current_and_new_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/demo.conf"), b"local").unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let old_hash = conary_core::hash::sha256(b"old");
    let mut old_file = FileEntry::new(
        "/etc/demo.conf".to_string(),
        resolved_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            0o100640,
        ),
        Some(PayloadContentAuthority {
            sha256: old_hash.clone(),
            size: 3,
        }),
        trove_id,
    );
    let old_file_id = old_file.insert(&conn).unwrap();
    let mut old_config =
        ConfigFile::new_noreplace("/etc/demo.conf".to_string(), trove_id, old_hash.clone());
    old_config.file_id = Some(old_file_id);
    old_config.source = ConfigSource::Rpm;
    old_config.insert(&conn).unwrap();
    let incoming = crate::commands::LiveRootFile {
        path: "/etc/demo.conf".to_string(),
        content: crate::commands::LiveRootContent::from_in_memory_bytes(b"new"),
        node: resolved_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            0o100644,
        ),
    };

    let transaction = capture_install(
        &conn,
        &root,
        &cas,
        ConfigInstallCapture {
            source: ConfigSource::Rpm,
            declared: &[source_config(ConfigSource::Rpm, "/etc/demo.conf")],
            incoming: &[incoming],
            replacing_trove_id: Some(trove_id),
            replaced_trove_ids: &[trove_id],
        },
    )
    .unwrap();

    assert_eq!(transaction.entries.len(), 1);
    let entry = &transaction.entries[0];
    assert_eq!(entry.before.as_ref().unwrap().source, ConfigSource::Rpm);
    assert_eq!(
        entry.before.as_ref().unwrap().original_sha256.as_deref(),
        Some(old_hash.as_str())
    );
    assert_eq!(
        cas.retrieve(entry.current.as_ref().unwrap().sha256())
            .unwrap(),
        b"local"
    );
    let incoming_artifact = entry.after.as_ref().unwrap().artifact.as_ref().unwrap();
    assert_eq!(cas.retrieve(incoming_artifact.sha256()).unwrap(), b"new");
}

#[test]
fn auxiliary_capture_accepts_only_canonical_numbered_pacsaves() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    fs::create_dir_all(root.join("etc")).unwrap();
    for (suffix, content) in [
        ("2", b"canonical".as_slice()),
        ("0", b"zero".as_slice()),
        ("01", b"leading-zero".as_slice()),
        ("+1", b"plus".as_slice()),
    ] {
        fs::write(
            root.join(format!("etc/demo.conf.pacsave.{suffix}")),
            content,
        )
        .unwrap();
    }

    let auxiliaries = capture_auxiliaries(&root, &cas, "/etc/demo.conf").unwrap();
    let paths = auxiliaries
        .iter()
        .map(|(path, _)| path.as_str())
        .collect::<Vec<_>>();

    assert_eq!(paths, vec!["/etc/demo.conf.pacsave.2"]);
}

#[test]
fn deb_remove_on_upgrade_is_a_durable_generation_operation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    let obsolete = root.join("etc/obsolete.conf");
    fs::write(&obsolete, b"locally modified").unwrap();
    let locally_modified = regular_fixture_from_path(&obsolete);
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut old = ConfigFile::new(
        "/etc/obsolete.conf".to_string(),
        trove_id,
        conary_core::hash::sha256(b"old"),
    );
    old.source = ConfigSource::Deb;
    old.insert(&conn).unwrap();

    let transaction = capture_install(
        &conn,
        &root,
        &cas,
        ConfigInstallCapture {
            source: ConfigSource::Deb,
            declared: &[deb_remove("/etc/obsolete.conf")],
            incoming: &[],
            replacing_trove_id: Some(trove_id),
            replaced_trove_ids: &[trove_id],
        },
    )
    .unwrap();

    assert_eq!(transaction.entries.len(), 1);
    let entry = &transaction.entries[0];
    assert_eq!(entry.operation, ConfigTransactionOperation::RemoveOnUpgrade);
    assert!(entry.after.is_none());
    assert_eq!(
        plan_entry(entry).unwrap(),
        vec![
            OverlayMutation::Remove("/etc/obsolete.conf.dpkg-dist".to_string()),
            overlay_write("/etc/obsolete.conf.dpkg-old".to_string(), locally_modified,),
            OverlayMutation::Remove("/etc/obsolete.conf".to_string()),
        ]
    );

    let other_owner = capture_install(
        &conn,
        &root,
        &cas,
        ConfigInstallCapture {
            source: ConfigSource::Deb,
            declared: &[deb_remove("/etc/obsolete.conf")],
            incoming: &[],
            replacing_trove_id: Some(trove_id + 1),
            replaced_trove_ids: &[trove_id],
        },
    )
    .unwrap();
    assert!(other_owner.entries.is_empty());
}

#[test]
fn deb_remove_on_upgrade_without_prior_identity_performs_no_mutation() {
    // dpkg ignores a remove-on-upgrade conffile that was never a tracked
    // conffile. The declaration remains durable authority without touching
    // the user-created path.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/new.conf"), b"user-created").unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();

    let transaction = capture_install(
        &conn,
        &root,
        &cas,
        ConfigInstallCapture {
            source: ConfigSource::Deb,
            declared: &[deb_remove("/etc/new.conf")],
            incoming: &[],
            replacing_trove_id: Some(trove_id),
            replaced_trove_ids: &[trove_id],
        },
    )
    .unwrap();

    assert!(transaction.entries.is_empty());
    assert!(root.join("etc/new.conf").exists());
}

#[test]
fn generation_and_mutable_paths_share_exact_suffix_decisions() {
    let cases = [
        (ConfigSource::Rpm, true, ConfigSuffix::RpmNew.as_str()),
        (ConfigSource::Rpm, false, ConfigSuffix::RpmSave.as_str()),
        (ConfigSource::Deb, true, ConfigSuffix::DpkgDist.as_str()),
        (ConfigSource::Arch, true, ConfigSuffix::PacNew.as_str()),
        (ConfigSource::Auto, false, ConfigSuffix::ConarySave.as_str()),
    ];
    for (source, noreplace, suffix) in cases {
        let mutations = plan_entry(&update_entry(source, noreplace)).unwrap();
        assert!(
            mutations.iter().any(|mutation| matches!(
                mutation,
                OverlayMutation::Write(path, _) if path == &format!("/etc/demo.conf{suffix}")
            )),
            "{source:?} did not materialize {suffix}: {mutations:?}"
        );
    }
}

#[test]
fn debian_deleted_conffile_gets_whiteout_and_dpkg_dist() {
    let mut entry = update_entry(ConfigSource::Deb, true);
    entry.current = None;
    let mutations = plan_entry(&entry).unwrap();
    assert!(mutations.contains(&OverlayMutation::Whiteout("/etc/demo.conf".to_string())));
    assert!(mutations.iter().any(|mutation| matches!(
        mutation,
        OverlayMutation::Write(path, _) if path == "/etc/demo.conf.dpkg-dist"
    )));
}

#[test]
fn rpm_ghost_install_has_no_overlay_mutation() {
    let mut entry = update_entry(ConfigSource::Rpm, false);
    let after = entry.after.as_mut().unwrap();
    after.ghost = true;
    after.original_sha256 = None;
    after.artifact = None;
    assert!(plan_entry(&entry).unwrap().is_empty());

    entry.operation = ConfigTransactionOperation::Remove;
    entry.before = entry.after.take();
    assert_eq!(
        plan_entry(&entry).unwrap(),
        vec![OverlayMutation::Remove("/etc/demo.conf".to_string())]
    );
}

#[test]
fn arch_remove_rotates_pacsaves_before_current_save() {
    let mut entry = update_entry(ConfigSource::Arch, true);
    entry.operation = ConfigTransactionOperation::Remove;
    entry.after = None;
    entry.auxiliaries = vec![
        (
            "/etc/demo.conf.pacsave".to_string(),
            regular(b"zero", 0o100600),
        ),
        (
            "/etc/demo.conf.pacsave.2".to_string(),
            regular(b"two", 0o100600),
        ),
    ];
    let mutations = plan_entry(&entry).unwrap();
    assert!(mutations.iter().any(|mutation| matches!(
        mutation,
        OverlayMutation::Write(path, _) if path == "/etc/demo.conf.pacsave.3"
    )));
    assert!(mutations.iter().any(|mutation| matches!(
        mutation,
        OverlayMutation::Write(path, _) if path == "/etc/demo.conf.pacsave.1"
    )));
    assert!(mutations.iter().any(|mutation| matches!(
        mutation,
        OverlayMutation::Write(path, _) if path == "/etc/demo.conf.pacsave"
    )));
}

#[test]
fn restore_reinstates_exact_primary_and_auxiliaries_and_removes_rotated_outputs() {
    let primary = regular(b"local-before", 0o100600);
    let pacsave = regular(b"saved-before", 0o100640);
    let entry = ConfigPathTransaction {
        path: "/etc/demo.conf".to_string(),
        operation: ConfigTransactionOperation::Restore,
        before: Some(state(ConfigSource::Arch, true, b"old")),
        current: Some(primary.clone()),
        after: None,
        auxiliaries: vec![("/etc/demo.conf.pacsave".to_string(), pacsave.clone())],
    };

    let mutations = plan_entry(&entry).unwrap();

    assert!(mutations.contains(&OverlayMutation::Remove(
        "/etc/demo.conf.pacsave.1".to_string()
    )));
    assert!(mutations.contains(&overlay_write("/etc/demo.conf".to_string(), primary)));
    assert!(mutations.contains(&overlay_write(
        "/etc/demo.conf.pacsave".to_string(),
        pacsave
    )));
}

#[test]
fn debian_purge_removes_primary_and_backup_artifacts() {
    let mut entry = update_entry(ConfigSource::Deb, true);
    entry.operation = ConfigTransactionOperation::Purge;
    entry.after = None;
    entry.auxiliaries = vec![
        (
            "/etc/demo.conf.dpkg-dist".to_string(),
            regular(b"dist", 0o100600),
        ),
        (
            "/etc/demo.conf.dpkg-old".to_string(),
            regular(b"old-backup", 0o100600),
        ),
    ];

    let mutations = plan_entry(&entry).unwrap();
    for path in [
        "/etc/demo.conf",
        "/etc/demo.conf.dpkg-dist",
        "/etc/demo.conf.dpkg-old",
    ] {
        assert!(
            mutations.contains(&OverlayMutation::Remove(path.to_string())),
            "missing purge mutation for {path}: {mutations:?}"
        );
    }
}

#[test]
fn remove_without_prior_source_fails_before_generation_staging() {
    let temp = tempfile::tempdir().unwrap();
    let runtime_root = ConaryRuntimeRoot::for_test_root(temp.path());
    let transaction = GenerationConfigTransaction {
        entries: vec![ConfigPathTransaction {
            path: "/etc/demo.conf".to_string(),
            operation: ConfigTransactionOperation::Remove,
            before: None,
            current: Some(regular(b"local", 0o100600)),
            after: None,
            auxiliaries: Vec::new(),
        }],
        ..Default::default()
    };

    let error = materialize(&runtime_root, 1, &[transaction])
        .unwrap_err()
        .to_string();

    assert!(error.contains("no exact prior package state"), "{error}");
    assert!(
        !runtime_root.etc_state_dir().exists(),
        "validation must fail before creating the generation config staging root"
    );
}

#[test]
fn atomic_materialization_clones_unaffected_upper() {
    let temp = tempfile::tempdir().unwrap();
    let runtime_root = ConaryRuntimeRoot::for_test_root(temp.path());
    fs::create_dir_all(runtime_root.etc_state_dir().join("1")).unwrap();
    fs::write(runtime_root.etc_state_dir().join("1/unaffected"), b"keep").unwrap();
    fs::create_dir_all(runtime_root.generations_dir().join("1")).unwrap();
    fs::create_dir_all(runtime_root.root()).unwrap();
    std::os::unix::fs::symlink("generations/1", runtime_root.root().join("current")).unwrap();

    let transaction = GenerationConfigTransaction {
        entries: vec![ConfigPathTransaction {
            path: "/etc/link".to_string(),
            operation: ConfigTransactionOperation::Install,
            before: None,
            current: None,
            after: Some(ConfigPackageState {
                source: ConfigSource::Auto,
                noreplace: false,
                ghost: false,
                materialized: true,
                original_sha256: Some(conary_core::hash::sha256(b"target")),
                artifact: Some(symlink("target", 0o120777)),
            }),
            auxiliaries: Vec::new(),
        }],
        ..Default::default()
    };
    materialize(&runtime_root, 2, &[transaction]).unwrap();
    let upper = runtime_root.etc_state_dir().join("2");
    assert_eq!(fs::read(upper.join("unaffected")).unwrap(), b"keep");
    // A clean install is supplied by the generation lower, so no upper
    // copy is expected for the new symlink.
    assert!(!upper.join("link").exists());
}

#[test]
fn materialization_merges_seeded_state_with_current_upper_using_current_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let runtime_root = ConaryRuntimeRoot::for_test_root(temp.path());
    let current_upper = runtime_root.etc_state_dir().join("1");
    let seeded_upper = runtime_root.etc_state_dir().join("2");
    fs::create_dir_all(current_upper.join("nested")).unwrap();
    fs::create_dir_all(seeded_upper.join("nested")).unwrap();
    fs::write(current_upper.join("current-only"), b"current").unwrap();
    fs::write(current_upper.join("nested/overlap"), b"current").unwrap();
    fs::write(seeded_upper.join("seeded-only"), b"seeded").unwrap();
    fs::write(seeded_upper.join("nested/overlap"), b"seeded").unwrap();
    fs::create_dir_all(runtime_root.generations_dir().join("1")).unwrap();
    std::os::unix::fs::symlink("generations/1", runtime_root.root().join("current")).unwrap();

    materialize(&runtime_root, 2, &[]).unwrap();

    assert_eq!(
        fs::read(seeded_upper.join("current-only")).unwrap(),
        b"current"
    );
    assert_eq!(
        fs::read(seeded_upper.join("seeded-only")).unwrap(),
        b"seeded"
    );
    assert_eq!(
        fs::read(seeded_upper.join("nested/overlap")).unwrap(),
        b"current"
    );
}

#[test]
fn materialization_rejects_corrupt_config_cas_content_before_publication() {
    let temp = tempfile::tempdir().unwrap();
    let runtime_root = ConaryRuntimeRoot::for_test_root(temp.path());
    let artifact = regular(b"expected", 0o100600);
    let cas = CasStore::new(runtime_root.objects_dir()).unwrap();
    let object = cas.hash_to_path(artifact.sha256()).unwrap();
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    fs::write(&object, b"corrupt!").unwrap();
    let transaction = GenerationConfigTransaction {
        entries: vec![ConfigPathTransaction {
            path: "/etc/demo.conf".to_string(),
            operation: ConfigTransactionOperation::Restore,
            before: None,
            current: Some(artifact),
            after: None,
            auxiliaries: Vec::new(),
        }],
        ..Default::default()
    };

    let error = materialize(&runtime_root, 1, &[transaction])
        .unwrap_err()
        .to_string();

    assert!(error.contains("digest mismatch"), "{error}");
    assert!(!runtime_root.etc_state_dir().join("1").exists());
    assert!(
        fs::read_dir(runtime_root.etc_state_dir())
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn materialization_preserves_regular_modes_and_symlink_targets() {
    const TEST_NAME: &str = "commands::generation::config_transaction::tests::materialization_preserves_regular_modes_and_symlink_targets";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let runtime_root = ConaryRuntimeRoot::for_test_root(temp.path());
    let cas = CasStore::new(runtime_root.objects_dir()).unwrap();
    for content in [b"old".as_slice(), b"local".as_slice(), b"new".as_slice()] {
        cas.store(content).unwrap();
    }
    fs::create_dir_all(runtime_root.etc_state_dir()).unwrap();
    fs::create_dir_all(runtime_root.root()).unwrap();

    let mut regular_entry = update_entry(ConfigSource::Auto, false);
    regular_entry.current = Some(regular(b"local", 0o100600));
    let symlink = ConfigPathTransaction {
        path: "/etc/demo-link".to_string(),
        operation: ConfigTransactionOperation::Install,
        before: Some(ConfigPackageState {
            source: ConfigSource::Arch,
            noreplace: true,
            ghost: false,
            materialized: true,
            original_sha256: Some(conary_core::hash::sha256(b"old-target")),
            artifact: Some(symlink("old-target", 0o120777)),
        }),
        current: Some(symlink("local-target", 0o120777)),
        after: Some(ConfigPackageState {
            source: ConfigSource::Arch,
            noreplace: true,
            ghost: false,
            materialized: true,
            original_sha256: Some(conary_core::hash::sha256(b"new-target")),
            artifact: Some(symlink("new-target", 0o120777)),
        }),
        auxiliaries: Vec::new(),
    };
    let transaction = GenerationConfigTransaction {
        entries: vec![regular_entry, symlink],
        ..Default::default()
    };

    materialize(&runtime_root, 1, &[transaction]).unwrap();

    let upper = runtime_root.etc_state_dir().join("1");
    assert_eq!(
        fs::read(upper.join("demo.conf.conary-save")).unwrap(),
        b"local"
    );
    assert_eq!(
        fs::metadata(upper.join("demo.conf.conary-save"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
    assert_eq!(
        fs::read_link(upper.join("demo-link")).unwrap(),
        PathBuf::from("local-target")
    );
    assert_eq!(
        fs::read_link(upper.join("demo-link.pacnew")).unwrap(),
        PathBuf::from("new-target")
    );
}
