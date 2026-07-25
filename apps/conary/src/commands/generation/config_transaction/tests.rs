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
        content,
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

fn state(source: ConfigSource, noreplace: bool, content: &[u8]) -> ConfigPackageState {
    let artifact = regular(content, 0o100640);
    ConfigPackageState {
        source,
        noreplace,
        ghost: false,
        original_sha256: Some(artifact.sha256().to_string()),
        artifact: Some(artifact),
    }
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
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1".to_string(),
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
        content: b"new".to_vec(),
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
        ConfigSource::Rpm,
        &[ConfigFileInfo {
            path: "/etc/demo.conf".to_string(),
            noreplace: true,
            ghost: false,
            remove_on_upgrade: false,
        }],
        &[incoming],
        Some(trove_id),
        &[trove_id],
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
        entry
            .current
            .as_ref()
            .unwrap()
            .regular_content()
            .unwrap()
            .unwrap(),
        b"local"
    );
    assert_eq!(
        entry
            .after
            .as_ref()
            .unwrap()
            .artifact
            .as_ref()
            .unwrap()
            .regular_content()
            .unwrap()
            .unwrap(),
        b"new"
    );
}

#[test]
fn deb_remove_on_upgrade_is_a_durable_generation_operation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/obsolete.conf"), b"locally modified").unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1".to_string(),
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
        ConfigSource::Deb,
        &[ConfigFileInfo {
            path: "/etc/obsolete.conf".to_string(),
            noreplace: true,
            ghost: false,
            remove_on_upgrade: true,
        }],
        &[],
        Some(trove_id),
        &[trove_id],
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
            OverlayMutation::Write(
                "/etc/obsolete.conf.dpkg-old".to_string(),
                regular(b"locally modified", 0o100644),
            ),
            OverlayMutation::Remove("/etc/obsolete.conf".to_string()),
        ]
    );

    let other_owner = capture_install(
        &conn,
        &root,
        ConfigSource::Deb,
        &[ConfigFileInfo {
            path: "/etc/obsolete.conf".to_string(),
            noreplace: true,
            ghost: false,
            remove_on_upgrade: true,
        }],
        &[],
        Some(trove_id + 1),
        &[trove_id],
    )
    .unwrap();
    assert!(other_owner.entries.is_empty());
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
    assert!(mutations.contains(&OverlayMutation::Write(
        "/etc/demo.conf".to_string(),
        primary
    )));
    assert!(mutations.contains(&OverlayMutation::Write(
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
fn materialization_preserves_regular_modes_and_symlink_targets() {
    let temp = tempfile::tempdir().unwrap();
    let runtime_root = ConaryRuntimeRoot::for_test_root(temp.path());
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
            original_sha256: Some(conary_core::hash::sha256(b"old-target")),
            artifact: Some(symlink("old-target", 0o120777)),
        }),
        current: Some(symlink("local-target", 0o120777)),
        after: Some(ConfigPackageState {
            source: ConfigSource::Arch,
            noreplace: true,
            ghost: false,
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
