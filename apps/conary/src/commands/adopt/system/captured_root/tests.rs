// apps/conary/src/commands/adopt/system/captured_root/tests.rs

use super::*;
use conary_core::db::models::{Changeset, ChangesetStatus};
use conary_core::filesystem::CasStore;
use conary_core::generation::root_manifest::{
    SelectedRootCaptureExclusions, scan_selected_root_with_exclusions,
};
use conary_core::packages::InstalledFileAbsencePolicy;
use conary_core::payload::PayloadNodeKind;
use std::os::unix::fs::PermissionsExt;

fn insert_changeset(tx: &Transaction<'_>, description: &str) -> i64 {
    let mut changeset = Changeset::new(description.to_string());
    let id = changeset.insert(tx).unwrap();
    changeset
        .update_status(tx, ChangesetStatus::Applied)
        .unwrap();
    id
}

#[test]
fn full_capture_partitions_package_and_unowned_authority_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    for directory in [
        "conary/objects",
        "etc/systemd/system/multi-user.target.wants",
        "run",
        "usr/bin",
        "var/lib/conary",
    ] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(root.join("conary/objects/private"), b"runtime").unwrap();
    std::fs::write(root.join("run/private"), b"ephemeral").unwrap();
    std::fs::write(root.join("var/lib/conary/conary.db"), b"runtime").unwrap();
    std::fs::write(root.join("etc/owned-primary"), b"hardlinked").unwrap();
    std::fs::hard_link(
        root.join("etc/owned-primary"),
        root.join("etc/owned-secondary"),
    )
    .unwrap();
    std::fs::hard_link(
        root.join("etc/owned-primary"),
        root.join("etc/unowned-link"),
    )
    .unwrap();
    std::fs::set_permissions(
        root.join("etc/owned-primary"),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        "/usr/sbin/sshd",
        root.join("etc/systemd/system/multi-user.target.wants/sshd.service"),
    )
    .unwrap();
    std::fs::write(root.join("usr/bin/owned"), b"package").unwrap();

    let exclusions = SelectedRootCaptureExclusions::new(vec![
        "/conary".to_string(),
        "/var/lib/conary".to_string(),
    ])
    .unwrap();
    let captured = scan_selected_root_with_exclusions(&root, &cas, &exclusions).unwrap();
    let entries = captured
        .generation
        .entries
        .iter()
        .chain(&captured.state.entries)
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<std::collections::HashMap<_, _>>();

    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let tx = conn.transaction().unwrap();
    let changeset_id = insert_changeset(&tx, "seed package");
    let mut package = Trove::new_with_source(
        "native-package".to_string(),
        "1-1".to_string(),
        TroveType::Package,
        InstallSource::AdoptedFull,
        VersionScheme::Rpm,
    );
    package.architecture = Some("x86_64".to_string());
    package.native_package_identity = Some(
        conary_core::packages::InstalledPackageIdentity::rpm(
            "native-package-1-1.x86_64",
            "native-package",
            None,
            "1",
            "1",
            "x86_64",
        )
        .unwrap(),
    );
    package.installed_by_changeset_id = Some(changeset_id);
    let package_id = package.insert(&tx).unwrap();
    for path in ["/etc", "/usr", "/usr/bin", "/usr/bin/owned"] {
        let entry = entries[path];
        let mut file = FileEntry::new(
            path.to_string(),
            entry.node.clone(),
            entry.content.clone(),
            package_id,
        );
        file.insert_or_replace(&tx, ExistingDirectoryMaterialization::ApplyIncoming)
            .unwrap();
    }

    let mut package_hardlinks = ["/etc/owned-primary", "/etc/owned-secondary"].map(|path| {
        let entry = entries[path];
        let mut node = entry.node.clone();
        node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: None,
        };
        CapturedAdoptionFile {
            source: (
                path.to_string(),
                0,
                0,
                None,
                None,
                None,
                None,
                InstalledFileAbsencePolicy::Required,
            ),
            node,
            content: entries["/etc/owned-primary"].content.clone(),
        }
    });
    bind_package_payloads_to_selected_root(&captured, package_hardlinks.iter_mut()).unwrap();
    for captured in package_hardlinks {
        let mut file = captured.file_entry(package_id);
        file.insert_or_replace(&tx, ExistingDirectoryMaterialization::ApplyIncoming)
            .unwrap();
    }

    let first_changeset = insert_changeset(&tx, "capture selected root");
    let first = synchronize_captured_root(&tx, first_changeset, &captured).unwrap();
    assert!(first.changed);
    assert!(first.captured_entries > 0);
    assert!(first.package_entries >= 6);

    let captured_trove = Trove::list_all(&tx)
        .unwrap()
        .into_iter()
        .find(|trove| trove.install_source == InstallSource::CapturedRoot)
        .unwrap();
    let captured_id = captured_trove.id.unwrap();
    assert_eq!(captured_trove.version, CAPTURED_ROOT_VERSION);
    conary_core::repository::versioning::validate_repo_version(
        captured_trove.version_scheme,
        &captured_trove.version,
    )
    .unwrap();
    let captured_provides = ProvideEntry::find_by_trove(&tx, captured_id).unwrap();
    assert_eq!(captured_provides.len(), 1);
    assert_eq!(
        captured_provides[0].version.as_deref(),
        Some(CAPTURED_ROOT_VERSION)
    );
    conary_core::repository::versioning::validate_repo_version(
        captured_provides[0].version_scheme,
        captured_provides[0].version.as_deref().unwrap(),
    )
    .unwrap();
    assert_eq!(
        FileEntry::find_by_path(&tx, "/etc/owned-primary")
            .unwrap()
            .unwrap()
            .trove_id,
        package_id
    );
    assert_eq!(
        FileEntry::find_by_path(&tx, "/etc/unowned-link")
            .unwrap()
            .unwrap()
            .trove_id,
        captured_id
    );
    let primary = FileEntry::find_by_path(&tx, "/etc/owned-primary")
        .unwrap()
        .unwrap();
    let linked = FileEntry::find_by_path(&tx, "/etc/unowned-link")
        .unwrap()
        .unwrap();
    let package_linked = FileEntry::find_by_path(&tx, "/etc/owned-secondary")
        .unwrap()
        .unwrap();
    let PayloadNodeKind::Regular {
        hardlink_identity: Some(primary_identity),
    } = primary.node.source.kind
    else {
        panic!("package-owned hardlink primary lost its typed identity");
    };
    let PayloadNodeKind::Hardlink { identity, target } = linked.node.source.kind else {
        panic!("captured-root hardlink lost its typed link authority");
    };
    assert_eq!(identity, primary_identity);
    assert_eq!(target, "/etc/owned-primary");
    let PayloadNodeKind::Hardlink { identity, target } = package_linked.node.source.kind else {
        panic!("package-owned hardlink lost its typed link authority");
    };
    assert_eq!(identity, primary_identity);
    assert_eq!(target, "/etc/owned-primary");
    assert!(
        FileEntry::find_by_path(
            &tx,
            "/etc/systemd/system/multi-user.target.wants/sshd.service"
        )
        .unwrap()
        .is_some()
    );
    for excluded in ["/conary", "/run", "/var/lib/conary"] {
        assert!(
            FileEntry::find_all_ordered(&tx).unwrap().iter().all(
                |file| file.path != excluded && !file.path.starts_with(&format!("{excluded}/"))
            )
        );
    }

    let second_changeset = insert_changeset(&tx, "repeat selected-root capture");
    let second = synchronize_captured_root(&tx, second_changeset, &captured).unwrap();
    assert!(!second.changed);
    assert_eq!(second.captured_entries, first.captured_entries);
    assert_eq!(second.package_entries, first.package_entries);
    assert_eq!(
        Trove::list_all(&tx)
            .unwrap()
            .iter()
            .filter(|trove| trove.install_source == InstallSource::CapturedRoot)
            .count(),
        1
    );
    tx.commit().unwrap();
}

#[test]
fn runtime_capture_exclusions_follow_default_and_isolated_database_roots() {
    let default = capture_exclusions("/var/lib/conary/conary.db").unwrap();
    assert!(default.excludes("/conary/objects/aa"));
    assert!(default.excludes("/var/lib/conary/conary.db"));
    assert!(default.excludes("/var/lib/conary/conary.db-wal"));
    assert!(!default.excludes("/var/lib/conary-adjacent/state"));

    let isolated = capture_exclusions("target/captured-root-test/conary.db").unwrap();
    let cwd = std::env::current_dir().unwrap();
    let isolated_root = cwd.join("target/captured-root-test");
    assert!(isolated.excludes(isolated_root.to_str().unwrap()));
}

#[test]
fn runtime_capture_exclusions_cover_lexical_and_resolved_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let resolved = temp.path().join("resolved-runtime");
    let alias = temp.path().join("runtime-alias");
    std::fs::create_dir_all(&resolved).unwrap();
    std::fs::write(resolved.join("conary.db"), b"database").unwrap();
    std::os::unix::fs::symlink(&resolved, &alias).unwrap();

    let exclusions = capture_exclusions(alias.join("conary.db").to_str().unwrap()).unwrap();

    assert!(exclusions.excludes(alias.join("objects").to_str().unwrap()));
    assert!(exclusions.excludes(resolved.join("objects").to_str().unwrap()));
    assert!(exclusions.excludes(resolved.join("conary.db-wal").to_str().unwrap()));
}

#[test]
fn runtime_capture_rejects_a_root_runtime_authority() {
    let error = capture_exclusions("/conary.db").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("runtime root cannot be / during selected-root capture"),
        "{error:#}"
    );
}

#[test]
fn complete_root_capture_detects_stale_track_only_authority() {
    fn trove(name: &str, source: InstallSource) -> Trove {
        Trove::new_with_source(
            name.to_string(),
            "1-1".to_string(),
            TroveType::Package,
            source,
            VersionScheme::Rpm,
        )
    }

    let tracked = HashMap::from([
        (
            "current-1-1.x86_64".to_string(),
            trove("current", InstallSource::AdoptedTrack),
        ),
        (
            "stale-1-1.x86_64".to_string(),
            trove("stale", InstallSource::AdoptedTrack),
        ),
        (
            "repository-1-1.x86_64".to_string(),
            trove("repository", InstallSource::Repository),
        ),
    ]);

    let error = ensure_complete_native_partition(&tracked, ["current-1-1.x86_64"]).unwrap_err();

    assert!(error.to_string().contains("stale-1-1.x86_64"), "{error:#}");
    assert!(
        !error.to_string().contains("repository-1-1.x86_64"),
        "{error:#}"
    );
}
