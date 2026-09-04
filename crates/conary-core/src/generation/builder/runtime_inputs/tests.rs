// crates/conary-core/src/generation/builder/runtime_inputs/tests.rs

use super::*;
use std::os::unix::fs::PermissionsExt;

use crate::db::models::{FileEntry, PayloadClaimAnchorPolicy, Trove, TroveType};
use crate::db::testing::create_test_db;
use crate::payload::{
    PayloadContentAuthority, PayloadNode, PayloadNodeKind, PayloadSharingPolicy,
    ResolvedPayloadNode,
};

fn resolved(node: PayloadNode) -> ResolvedPayloadNode {
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

fn trove(id: i64, name: &str, source: InstallSource) -> Trove {
    let mut trove = Trove::new_with_source(
        name.to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        source,
        crate::repository::versioning::VersionScheme::Conary,
    );
    trove.id = Some(id);
    trove
}

fn directory(path: &str, trove_id: i64) -> FileEntry {
    let mut node = PayloadNode::regular(0o755);
    node.kind = PayloadNodeKind::Directory;
    node.mode = libc::S_IFDIR | 0o755;
    FileEntry::new(path.to_string(), resolved(node), None, trove_id)
}

fn regular(path: &str, bytes: &[u8], trove_id: i64) -> FileEntry {
    FileEntry::new(
        path.to_string(),
        resolved(PayloadNode::regular(0o755)),
        Some(PayloadContentAuthority {
            sha256: crate::hash::sha256(bytes),
            size: bytes.len() as u64,
        }),
        trove_id,
    )
}

fn symlink(path: &str, target: &str, trove_id: i64) -> FileEntry {
    let mut node = PayloadNode::regular(0o777);
    node.kind = PayloadNodeKind::Symlink {
        target: target.to_string(),
    };
    node.mode = libc::S_IFLNK | 0o777;
    FileEntry::new(path.to_string(), resolved(node), None, trove_id)
}

fn selected_root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn exact_nodes_split_into_immutable_and_state_domains() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "runtime", InstallSource::Repository)];
    let files = vec![
        directory("/etc", 1),
        regular("/etc/runtime.conf", b"state", 1),
        directory("/opt", 1),
        regular("/opt/runtime", b"immutable", 1),
        directory("/tmp", 1),
        regular("/tmp/ignored", b"ephemeral", 1),
    ];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

    assert_eq!(
        inputs
            .generation
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["/opt", "/opt/runtime"]
    );
    assert_eq!(
        inputs
            .state
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["/etc", "/etc/runtime.conf"]
    );
}

#[test]
fn exact_live_parent_directories_close_immutable_inputs() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    std::fs::create_dir_all(root.path().join("boot/loader/entries")).unwrap();
    std::fs::set_permissions(
        root.path().join("boot/loader"),
        std::fs::Permissions::from_mode(0o711),
    )
    .unwrap();
    let troves = vec![trove(1, "bootloader", InstallSource::AdoptedFull)];
    let files = vec![directory("/boot", 1), directory("/boot/loader/entries", 1)];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

    assert_eq!(
        inputs
            .generation
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["/boot", "/boot/loader", "/boot/loader/entries"]
    );
    let parent = inputs
        .generation
        .entries
        .iter()
        .find(|entry| entry.path == "/boot/loader")
        .unwrap();
    assert!(matches!(
        parent.node.source.kind,
        PayloadNodeKind::Directory
    ));
    assert_eq!(parent.node.source.mode & 0o7777, 0o711);
    assert!(parent.content.is_none());
}

#[test]
fn exact_live_parent_directories_follow_the_child_state_domain() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    std::fs::create_dir_all(root.path().join("etc/runtime")).unwrap();
    let troves = vec![trove(1, "runtime", InstallSource::AdoptedFull)];
    let files = vec![regular("/etc/runtime/config", b"state", 1)];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

    assert!(inputs.generation.entries.is_empty());
    assert_eq!(
        inputs
            .state
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["/etc", "/etc/runtime", "/etc/runtime/config"]
    );
}

#[test]
fn absent_exact_live_parent_directory_is_rejected() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "runtime", InstallSource::AdoptedFull)];
    let files = vec![regular("/opt/missing/tool", b"tool", 1)];

    let error = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("cannot capture exact selected-root parent /opt")
    );
}

#[test]
fn non_directory_exact_live_parent_is_rejected() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    std::fs::write(root.path().join("opt"), b"not a directory").unwrap();
    let troves = vec![trove(1, "runtime", InstallSource::AdoptedFull)];
    let files = vec![regular("/opt/tool", b"tool", 1)];

    let error = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("selected-root parent /opt is regular, not a directory")
    );
}

#[test]
fn special_nodes_remain_typed_generation_inputs() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "runtime", InstallSource::Repository)];
    let mut fifo = PayloadNode::regular(0o640);
    fifo.kind = PayloadNodeKind::Fifo;
    fifo.mode = libc::S_IFIFO | 0o640;
    let files = vec![
        directory("/usr", 1),
        FileEntry::new("/usr/events".to_string(), resolved(fifo), None, 1),
    ];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();
    assert!(matches!(
        inputs.generation.entries[1].node.source.kind,
        PayloadNodeKind::Fifo
    ));
}

#[test]
fn runtime_projection_never_synthesizes_usr_merge_or_abi_links() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "glibc", InstallSource::Repository)];
    let files = vec![
        directory("/usr", 1),
        directory("/usr/lib", 1),
        regular("/usr/lib/ld-linux-x86-64.so.2", b"loader", 1),
    ];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();
    assert!(
        inputs
            .generation
            .entries
            .iter()
            .all(|entry| entry.path != "/lib64")
    );
}

#[test]
fn descendants_of_owned_usr_merge_links_use_materialized_paths() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "filesystem", InstallSource::AdoptedFull)];
    let files = vec![
        symlink("/lib", "usr/lib", 1),
        directory("/lib/modules", 1),
        directory("/usr", 1),
        directory("/usr/lib", 1),
    ];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

    assert_eq!(
        inputs
            .generation
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["/lib", "/usr", "/usr/lib", "/usr/lib/modules"]
    );
}

#[test]
fn nested_owned_symlink_ancestors_resolve_from_exact_manifest_authority() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "filesystem", InstallSource::AdoptedFull)];
    let files = vec![
        symlink("/alias", "real", 1),
        symlink("/alias/link", "nested", 1),
        regular("/alias/link/tool", b"tool", 1),
        directory("/real", 1),
        directory("/real/nested", 1),
    ];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

    assert_eq!(
        inputs
            .generation
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        [
            "/alias",
            "/real",
            "/real/link",
            "/real/nested",
            "/real/nested/tool"
        ]
    );
}

#[test]
fn equivalent_alias_and_materialized_authority_coalesce() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "filesystem", InstallSource::AdoptedFull)];
    let files = vec![
        symlink("/lib", "usr/lib", 1),
        directory("/lib/modules", 1),
        directory("/usr", 1),
        directory("/usr/lib", 1),
        directory("/usr/lib/modules", 1),
    ];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

    assert_eq!(
        inputs
            .generation
            .entries
            .iter()
            .filter(|entry| entry.path == "/usr/lib/modules")
            .count(),
        1
    );
}

#[test]
fn conflicting_alias_and_materialized_authority_is_rejected() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "filesystem", InstallSource::AdoptedFull)];
    let mut alias_directory = directory("/lib/modules", 1);
    alias_directory.node.source.mode = libc::S_IFDIR | 0o700;
    let files = vec![
        symlink("/lib", "usr/lib", 1),
        alias_directory,
        directory("/usr", 1),
        directory("/usr/lib", 1),
        directory("/usr/lib/modules", 1),
    ];

    let error = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("with conflicting exact payload authority")
    );
}

#[test]
fn alias_projection_reclassifies_the_materialized_path_domain() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "configuration", InstallSource::AdoptedFull)];
    let files = vec![
        symlink("/configuration", "etc", 1),
        regular("/configuration/app.conf", b"state", 1),
        directory("/etc", 1),
    ];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

    assert_eq!(
        inputs
            .generation
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["/configuration"]
    );
    assert_eq!(
        inputs
            .state
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["/etc", "/etc/app.conf"]
    );
}

#[test]
fn alias_projection_rejects_manifest_symlink_escape() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "filesystem", InstallSource::AdoptedFull)];
    let files = vec![
        symlink("/lib", "../../outside", 1),
        directory("/lib/modules", 1),
    ];

    let error = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("cannot resolve exact manifest symlink ancestry"),
        "{error}"
    );
}

#[test]
fn exact_symlinks_are_preserved_without_mode_or_hash_inference() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "init", InstallSource::Repository)];
    let files = vec![
        directory("/sbin", 1),
        symlink("/sbin/init", "../usr/lib/systemd/systemd", 1),
    ];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();
    assert!(matches!(
        &inputs.generation.entries[1].node.source.kind,
        PayloadNodeKind::Symlink { target }
            if target == "../usr/lib/systemd/systemd"
    ));
}

#[test]
fn adopted_track_is_counted_but_not_generation_authority() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "tracked", InstallSource::AdoptedTrack)];
    let files = vec![regular("/tracked", b"tracked", 1)];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();
    assert_eq!(inputs.adopted_track_count, 1);
    assert!(inputs.generation.entries.is_empty());
}

#[test]
fn captured_root_is_exact_generation_and_mutable_state_authority() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let troves = vec![trove(1, "captured-root", InstallSource::CapturedRoot)];
    let files = vec![
        directory("/etc", 1),
        regular("/etc/sshd_config", b"configured", 1),
        directory("/usr", 1),
        regular("/usr/local-state", b"captured", 1),
    ];

    let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

    assert_eq!(
        inputs
            .generation
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["/usr", "/usr/local-state"]
    );
    assert_eq!(
        inputs
            .state
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["/etc", "/etc/sshd_config"]
    );
}

#[test]
fn generation_claimant_keeps_a_non_generation_anchor_in_the_root() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let mut anchor_trove = Trove::new_with_source(
        "tracked-anchor".to_string(),
        "1".to_string(),
        TroveType::Package,
        InstallSource::AdoptedTrack,
        crate::repository::versioning::VersionScheme::Arch,
    );
    anchor_trove.architecture = Some("x86_64".to_string());
    anchor_trove.native_package_identity = Some(
        crate::packages::InstalledPackageIdentity::pacman(
            "tracked-anchor",
            "tracked-anchor",
            "1",
            "x86_64",
        )
        .unwrap(),
    );
    let anchor_trove_id = anchor_trove.insert(&conn).unwrap();
    let mut claimant_trove = Trove::new_with_source(
        "repository-claimant".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        crate::repository::versioning::VersionScheme::Conary,
    );
    let claimant_trove_id = claimant_trove.insert(&conn).unwrap();

    let directory_node = directory("/shared-directory", anchor_trove_id).node;
    let mut directory_anchor = FileEntry::new(
        "/shared-directory".to_string(),
        directory_node.clone(),
        None,
        anchor_trove_id,
    );
    directory_anchor.insert(&conn).unwrap();
    PayloadClaim::new_directory(
        "/shared-directory".to_string(),
        claimant_trove_id,
        directory_node,
    )
    .unwrap()
    .insert(&conn)
    .unwrap();

    let symlink_node = symlink("/shared-link", "shared-directory", anchor_trove_id).node;
    let mut symlink_anchor = FileEntry::new(
        "/shared-link".to_string(),
        symlink_node,
        None,
        anchor_trove_id,
    );
    symlink_anchor.insert(&conn).unwrap();
    PayloadClaim::new_directory(
        "/shared-link".to_string(),
        claimant_trove_id,
        directory("/shared-link", claimant_trove_id).node,
    )
    .unwrap()
    .with_anchor_policy(PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory)
    .insert(&conn)
    .unwrap();

    let inputs = collect_runtime_generation_inputs(
        &conn,
        &Trove::list_packages(&conn).unwrap(),
        FileEntry::find_all_ordered(&conn).unwrap(),
        root.path(),
    )
    .unwrap();

    assert_eq!(
        inputs
            .generation
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["/shared-directory", "/shared-link"]
    );
}

#[test]
fn database_recovery_projects_a_cross_package_hardlink_group_coherently() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let first = trove(1, "first", InstallSource::Repository)
        .insert(&conn)
        .unwrap();
    let second = trove(2, "second", InstallSource::Repository)
        .insert(&conn)
        .unwrap();
    directory("/usr", first).insert(&conn).unwrap();
    directory("/usr/share", first).insert(&conn).unwrap();

    let target_path = "/usr/share/shared-target";
    let content = PayloadContentAuthority {
        sha256: crate::hash::sha256(b"shared"),
        size: 6,
    };
    let mut first_target_node = PayloadNode::regular(0o644);
    first_target_node.kind = PayloadNodeKind::Regular {
        hardlink_identity: Some("rpm:1:7".to_string()),
    };
    let mut first_target = FileEntry::new(
        target_path.to_string(),
        resolved(first_target_node),
        Some(content.clone()),
        first,
    )
    .with_claim_policy(PayloadSharingPolicy::Rpm);
    first_target.insert(&conn).unwrap();
    let mut first_edge_node = PayloadNode::regular(0o644);
    first_edge_node.kind = PayloadNodeKind::Hardlink {
        target: target_path.to_string(),
        identity: "rpm:1:7".to_string(),
    };
    let mut first_edge = FileEntry::new(
        "/usr/share/first-edge".to_string(),
        resolved(first_edge_node),
        None,
        first,
    )
    .with_claim_policy(PayloadSharingPolicy::Rpm);
    first_edge.insert(&conn).unwrap();

    let mut second_target_node = PayloadNode::regular(0o644);
    second_target_node.kind = PayloadNodeKind::Regular {
        hardlink_identity: Some("rpm:9:42".to_string()),
    };
    let mut second_target = FileEntry::new(
        target_path.to_string(),
        resolved(second_target_node),
        Some(content),
        second,
    )
    .with_claim_policy(PayloadSharingPolicy::Rpm);
    second_target
        .insert_or_replace(
            &conn,
            crate::db::models::ExistingDirectoryMaterialization::ApplyIncoming,
        )
        .unwrap();
    let mut second_edge_node = PayloadNode::regular(0o644);
    second_edge_node.kind = PayloadNodeKind::Hardlink {
        target: target_path.to_string(),
        identity: "rpm:9:42".to_string(),
    };
    let mut second_edge = FileEntry::new(
        "/usr/share/second-edge".to_string(),
        resolved(second_edge_node),
        None,
        second,
    )
    .with_claim_policy(PayloadSharingPolicy::Rpm);
    second_edge.insert(&conn).unwrap();
    FileEntry::reconcile_hardlink_materialization(&conn, target_path).unwrap();

    let inputs = collect_runtime_generation_inputs(
        &conn,
        &Trove::list_packages(&conn).unwrap(),
        FileEntry::find_all_ordered(&conn).unwrap(),
        root.path(),
    )
    .unwrap();

    inputs.generation.validate().unwrap();
    let identity = format!("path:{target_path}");
    assert!(matches!(
        &inputs
            .generation
            .entries
            .iter()
            .find(|entry| entry.path == target_path)
            .unwrap()
            .node
            .source
            .kind,
        PayloadNodeKind::Regular {
            hardlink_identity: Some(actual)
        } if actual == &identity
    ));
    assert!(
        inputs
            .generation
            .entries
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.node.source.kind,
                    PayloadNodeKind::Hardlink { identity: actual, .. }
                        if actual == &identity
                )
            })
            .count()
            == 2
    );
}

#[test]
fn orphaned_file_authority_is_rejected() {
    let (_tmp, conn) = create_test_db();
    let root = selected_root();
    let error = collect_runtime_generation_inputs(
        &conn,
        &[trove(1, "runtime", InstallSource::Repository)],
        vec![regular("/orphan", b"orphan", 99)],
        root.path(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("orphaned file entry"));
}
