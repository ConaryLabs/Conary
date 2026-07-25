// apps/conary/src/commands/install/config_files/tests.rs

use super::*;
use conary_core::payload::{
    PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp, ResolvedPayloadNode,
};
use std::collections::BTreeMap;

const OLD: &str = "old";
const LOCAL: &str = "local";
const NEW: &str = "new";

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

fn live_regular(path: &str, content: &[u8], mode: u32) -> LiveRootFile {
    LiveRootFile {
        path: path.to_string(),
        content: content.to_vec(),
        node: resolved_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            mode,
        ),
    }
}

#[test]
fn alpm_three_hash_matrix_matches_documented_actions() {
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(OLD), OLD),
        ConfigInstallDecision::Install
    );
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(OLD), NEW),
        ConfigInstallDecision::Install
    );
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(LOCAL), OLD),
        ConfigInstallDecision::Keep
    );
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(NEW), NEW),
        ConfigInstallDecision::Install
    );
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(LOCAL), NEW),
        ConfigInstallDecision::InstallAlternative(ConfigSuffix::PacNew)
    );
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, None, Some(LOCAL), NEW),
        ConfigInstallDecision::InstallAlternative(ConfigSuffix::PacNew)
    );
}

#[test]
fn rpm_distinguishes_noreplace_save_and_first_install_backup() {
    assert_eq!(
        decide_config_install(ConfigSource::Rpm, true, Some(OLD), Some(LOCAL), NEW),
        ConfigInstallDecision::InstallAlternative(ConfigSuffix::RpmNew)
    );
    assert_eq!(
        decide_config_install(ConfigSource::Rpm, false, Some(OLD), Some(LOCAL), NEW),
        ConfigInstallDecision::SaveCurrentAndInstall(ConfigSuffix::RpmSave)
    );
    assert_eq!(
        decide_config_install(ConfigSource::Rpm, false, None, Some(LOCAL), NEW),
        ConfigInstallDecision::SaveCurrentAndInstall(ConfigSuffix::RpmOrig)
    );
}

#[test]
fn dpkg_default_keeps_dual_edits_and_user_deletions() {
    assert_eq!(
        decide_config_install(ConfigSource::Deb, true, Some(OLD), Some(LOCAL), NEW),
        ConfigInstallDecision::InstallAlternative(ConfigSuffix::DpkgDist)
    );
    assert_eq!(
        decide_config_install(ConfigSource::Deb, true, Some(OLD), None, OLD),
        ConfigInstallDecision::Keep
    );
    assert_eq!(
        decide_config_install(ConfigSource::Deb, true, Some(OLD), None, NEW),
        ConfigInstallDecision::InstallAlternative(ConfigSuffix::DpkgDist)
    );
}

#[test]
fn debian_conffile_md5_is_exact_and_never_integrity_authority() {
    assert_eq!(
        debian_original_md5(ConfigSource::Deb, true, b""),
        Some("d41d8cd98f00b204e9800998ecf8427e".to_string())
    );
    assert_eq!(
        debian_original_md5(ConfigSource::Rpm, true, b"content"),
        None
    );
    assert_eq!(
        debian_original_md5(ConfigSource::Deb, false, b"content"),
        None
    );
}

#[test]
fn remove_on_upgrade_requires_and_preserves_prior_debian_identity() {
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("conary.db");
    conary_core::db::init(&database_path).unwrap();
    let conn = conary_core::db::open(&database_path).unwrap();
    let mut old_trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let old_trove_id = old_trove.insert(&conn).unwrap();
    let mut new_trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "2".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let new_trove_id = new_trove.insert(&conn).unwrap();
    let mut config = ConfigFile::new(
        "/etc/obsolete.conf".to_string(),
        old_trove_id,
        conary_core::hash::sha256(b"old"),
    );
    config.source = ConfigSource::Deb;
    config.original_md5 = Some("149603e6c03516362a8da23f624db945".to_string());
    config.insert(&conn).unwrap();

    persist_debian_remove_on_upgrade(&conn, "/etc/obsolete.conf", new_trove_id).unwrap();

    let stored = ConfigFile::find_by_path(&conn, "/etc/obsolete.conf")
        .unwrap()
        .unwrap();
    assert_eq!(stored.trove_id, Some(new_trove_id));
    assert_eq!(
        stored.original_md5.as_deref(),
        Some("149603e6c03516362a8da23f624db945")
    );
    assert!(stored.remove_on_upgrade);
    assert_eq!(stored.status, ConfigStatus::Missing);

    let mut invalid = ConfigFile::new(
        "/etc/untyped.conf".to_string(),
        old_trove_id,
        conary_core::hash::sha256(b"old"),
    );
    invalid.source = ConfigSource::Deb;
    invalid.insert(&conn).unwrap();
    assert!(persist_debian_remove_on_upgrade(&conn, "/etc/untyped.conf", new_trove_id).is_err());
}

#[test]
fn prepared_arch_conflict_keeps_local_and_writes_pacnew() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/demo.conf"), b"local").unwrap();
    let db = temp.path().join("db.sqlite");
    conary_core::db::init(&db).unwrap();
    let conn = conary_core::db::open(&db).unwrap();
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut old = ConfigFile::new(
        "/etc/demo.conf".to_string(),
        trove_id,
        conary_core::hash::sha256(b"old"),
    );
    old.source = ConfigSource::Arch;
    old.insert(&conn).unwrap();

    let plan = prepare_config_install(
        &conn,
        &root,
        ConfigSource::Arch,
        &[ConfigFileInfo {
            path: "/etc/demo.conf".to_string(),
            noreplace: true,
            ghost: false,
            remove_on_upgrade: false,
        }],
        None,
        vec![live_regular("/etc/demo.conf", b"new", 0o100644)],
    )
    .unwrap();

    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].path, "/etc/demo.conf.pacnew");
    assert_eq!(plan.files[0].content, b"new");
}

#[test]
fn undeclared_etc_payload_uses_deterministic_conary_suffixes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/demo.conf"), b"local").unwrap();
    let db = temp.path().join("db.sqlite");
    conary_core::db::init(&db).unwrap();
    let conn = conary_core::db::open(&db).unwrap();

    let plan = prepare_config_install(
        &conn,
        &root,
        ConfigSource::Rpm,
        &[],
        None,
        vec![live_regular("/etc/demo.conf", b"new", 0o100640)],
    )
    .unwrap();

    assert_eq!(plan.files.len(), 2);
    assert_eq!(plan.files[0].path, "/etc/demo.conf.conary-save");
    assert_eq!(plan.files[0].content, b"local");
    assert_eq!(plan.files[1].path, "/etc/demo.conf");
    assert_eq!(plan.files[1].content, b"new");
}

#[test]
fn deb_remove_on_upgrade_removes_pristine_and_saves_modified_conffiles() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    let db = temp.path().join("db.sqlite");
    conary_core::db::init(&db).unwrap();
    let conn = conary_core::db::open(&db).unwrap();
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
    let declaration = ConfigFileInfo {
        path: "/etc/obsolete.conf".to_string(),
        noreplace: true,
        ghost: false,
        remove_on_upgrade: true,
    };

    fs::write(root.join("etc/obsolete.conf"), b"old").unwrap();
    fs::write(root.join("etc/obsolete.conf.dpkg-dist"), b"stale").unwrap();
    let pristine = prepare_config_install(
        &conn,
        &root,
        ConfigSource::Deb,
        std::slice::from_ref(&declaration),
        Some(trove_id),
        Vec::new(),
    )
    .unwrap();
    assert!(pristine.files.is_empty());
    assert_eq!(
        pristine.remove_paths,
        vec![
            "/etc/obsolete.conf".to_string(),
            "/etc/obsolete.conf.dpkg-dist".to_string()
        ]
    );

    fs::write(root.join("etc/obsolete.conf"), b"locally modified").unwrap();
    let modified = prepare_config_install(
        &conn,
        &root,
        ConfigSource::Deb,
        &[declaration],
        Some(trove_id),
        Vec::new(),
    )
    .unwrap();
    assert_eq!(modified.files.len(), 1);
    assert_eq!(modified.files[0].path, "/etc/obsolete.conf.dpkg-old");
    assert_eq!(modified.files[0].content, b"locally modified");

    let other_owner = prepare_config_install(
        &conn,
        &root,
        ConfigSource::Deb,
        &[ConfigFileInfo {
            path: "/etc/obsolete.conf".to_string(),
            noreplace: true,
            ghost: false,
            remove_on_upgrade: true,
        }],
        Some(trove_id + 1),
        Vec::new(),
    )
    .unwrap();
    assert!(other_owner.files.is_empty());
    assert!(other_owner.remove_paths.is_empty());
}

#[test]
fn deb_remove_retains_conffile_state_until_purge() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/demo.conf"), b"local").unwrap();
    let db = temp.path().join("db.sqlite");
    conary_core::db::init(&db).unwrap();
    let conn = conary_core::db::open(&db).unwrap();
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut config = ConfigFile::new(
        "/etc/demo.conf".to_string(),
        trove_id,
        conary_core::hash::sha256(b"old"),
    );
    config.source = ConfigSource::Deb;
    config.insert(&conn).unwrap();

    let mut plan = prepare_config_removal(
        &conn,
        &root,
        trove_id,
        ["/etc/demo.conf".to_string()],
        false,
    )
    .unwrap();
    assert!(plan.files.is_empty());
    assert!(plan.remove_paths.is_empty());

    let tx = conn.unchecked_transaction().unwrap();
    plan.persist_retained(&tx).unwrap();
    conary_core::db::models::Trove::delete(&tx, trove_id).unwrap();
    tx.commit().unwrap();
    let residual = ConfigFile::find_by_path(&conn, "/etc/demo.conf")
        .unwrap()
        .unwrap();
    assert_eq!(residual.trove_id, None);
    assert_eq!(residual.status, ConfigStatus::Modified);
    assert_eq!(
        residual.current_hash.as_deref(),
        Some(conary_core::hash::sha256(b"local").as_str())
    );
}

#[test]
fn deb_purge_removes_conffile_and_package_manager_backups() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/demo.conf"), b"local").unwrap();
    fs::write(root.join("etc/demo.conf.dpkg-dist"), b"incoming").unwrap();
    fs::write(root.join("etc/demo.conf.dpkg-old"), b"previous").unwrap();
    let db = temp.path().join("db.sqlite");
    conary_core::db::init(&db).unwrap();
    let conn = conary_core::db::open(&db).unwrap();
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut config = ConfigFile::new(
        "/etc/demo.conf".to_string(),
        trove_id,
        conary_core::hash::sha256(b"old"),
    );
    config.source = ConfigSource::Deb;
    config.insert(&conn).unwrap();

    let plan = prepare_config_removal(&conn, &root, trove_id, ["/etc/demo.conf".to_string()], true)
        .unwrap();

    assert_eq!(
        plan.remove_paths,
        vec![
            "/etc/demo.conf.dpkg-dist",
            "/etc/demo.conf.dpkg-old",
            "/etc/demo.conf",
        ]
    );
    assert!(plan.files.is_empty());
}

#[test]
fn deb_purge_plan_deletes_the_exact_rows_captured_before_trove_removal() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/demo.conf"), b"local").unwrap();
    let db = temp.path().join("db.sqlite");
    conary_core::db::init(&db).unwrap();
    let conn = conary_core::db::open(&db).unwrap();
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut config = ConfigFile::new(
        "/etc/demo.conf".to_string(),
        trove_id,
        conary_core::hash::sha256(b"old"),
    );
    config.source = ConfigSource::Deb;
    config.insert(&conn).unwrap();

    let plan = prepare_debian_config_purge(&conn, &root, trove_id).unwrap();
    conary_core::db::models::Trove::delete(&conn, trove_id).unwrap();
    assert!(
        ConfigFile::find_by_path(&conn, "/etc/demo.conf")
            .unwrap()
            .is_some()
    );

    plan.delete_rows(&conn).unwrap();
    assert!(
        ConfigFile::find_by_path(&conn, "/etc/demo.conf")
            .unwrap()
            .is_none()
    );
}

#[test]
fn rpm_ghost_config_is_owned_without_payload_and_erased_without_backup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("run")).unwrap();
    fs::write(root.join("run/demo.pid"), b"1234").unwrap();
    let db = temp.path().join("db.sqlite");
    conary_core::db::init(&db).unwrap();
    let conn = conary_core::db::open(&db).unwrap();
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut config = ConfigFile::new_ghost("/run/demo.pid".to_string(), trove_id);
    record_live_state(&mut config, &root).unwrap();
    config.insert(&conn).unwrap();

    let plan = prepare_config_removal(&conn, &root, trove_id, Vec::<String>::new(), false).unwrap();

    assert_eq!(plan.remove_paths, vec!["/run/demo.pid"]);
    assert!(plan.files.is_empty());
}

#[test]
fn arch_remove_rotates_existing_pacsaves_before_saving_local_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/demo.conf"), b"local").unwrap();
    fs::write(root.join("etc/demo.conf.pacsave"), b"save-zero").unwrap();
    fs::write(root.join("etc/demo.conf.pacsave.2"), b"save-two").unwrap();
    let db = temp.path().join("db.sqlite");
    conary_core::db::init(&db).unwrap();
    let conn = conary_core::db::open(&db).unwrap();
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut config = ConfigFile::new(
        "/etc/demo.conf".to_string(),
        trove_id,
        conary_core::hash::sha256(b"old"),
    );
    config.source = ConfigSource::Arch;
    config.insert(&conn).unwrap();

    let plan = prepare_config_removal(
        &conn,
        &root,
        trove_id,
        ["/etc/demo.conf".to_string()],
        false,
    )
    .unwrap();
    assert_eq!(
        plan.remove_paths,
        vec![
            "/etc/demo.conf.pacsave.2",
            "/etc/demo.conf.pacsave",
            "/etc/demo.conf"
        ]
    );
    let files = plan
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.content.as_slice()))
        .collect::<HashMap<_, _>>();
    assert_eq!(files["/etc/demo.conf.pacsave.3"], b"save-two".as_slice());
    assert_eq!(files["/etc/demo.conf.pacsave.1"], b"save-zero".as_slice());
    assert_eq!(files["/etc/demo.conf.pacsave"], b"local".as_slice());
}
