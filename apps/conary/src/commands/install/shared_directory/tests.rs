// apps/conary/src/commands/install/shared_directory/tests.rs

use super::*;
use conary_core::db::models::{PayloadClaimAnchorPolicy, TroveType};
use conary_core::payload::{
    PayloadContentAuthority, PayloadNode, PayloadSharingPolicy, ResolvedPayloadNode,
};
use conary_core::repository::versioning::VersionScheme;
use std::collections::BTreeSet;
use std::os::unix::fs::symlink;

fn insert_trove(conn: &rusqlite::Connection, name: &str) -> i64 {
    Trove::new(
        name.to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    )
    .insert(conn)
    .unwrap()
}

fn resolved_directory(mode: u32) -> ResolvedPayloadNode {
    let mut node = PayloadNode::regular(mode);
    node.kind = PayloadNodeKind::Directory;
    node.mode = libc::S_IFDIR | mode;
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

fn resolved_file(path: &str, node: ResolvedPayloadNode) -> ResolvedInstallFile {
    ResolvedInstallFile {
        path: path.to_string(),
        node,
        content: None,
        cas_hash: None,
    }
}

fn empty_regular_entry(path: &str, trove_id: i64) -> FileEntry {
    FileEntry::new(
        path.to_string(),
        ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
        Some(PayloadContentAuthority {
            sha256: conary_core::hash::sha256(b""),
            size: 0,
        }),
        trove_id,
    )
}

fn physical_directory(root: &std::path::Path, path: &str) {
    std::fs::create_dir_all(root.join(path.trim_start_matches('/'))).unwrap();
}

fn physical_symlink(root: &std::path::Path, path: &str, target: &str) {
    let leaf = root.join(path.trim_start_matches('/'));
    if let Some(parent) = leaf.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    symlink(target, leaf).unwrap();
}

#[test]
fn conflicting_directory_metadata_is_supported_but_node_type_overlap_is_not() {
    let (_root, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(db_path).unwrap();
    let owner_id = insert_trove(&conn, "owner");
    FileEntry::new(
        "/etc".to_string(),
        resolved_directory(0o755),
        None,
        owner_id,
    )
    .with_claim_policy(PayloadSharingPolicy::Rpm)
    .insert(&conn)
    .unwrap();
    let selected_root = tempfile::tempdir().unwrap();
    std::fs::create_dir(selected_root.path().join("etc")).unwrap();

    preflight_resolved_file_ownership(
        &conn,
        selected_root.path(),
        &[resolved_file("/etc", resolved_directory(0o700))],
        "incoming",
        &[],
        InstallSemantics::native_package(PackageFormatType::Rpm),
    )
    .unwrap();

    let error = preflight_resolved_file_ownership(
        &conn,
        selected_root.path(),
        &[ResolvedInstallFile {
            path: "/etc".to_string(),
            node: ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
            content: Some(PayloadContentAuthority {
                sha256: conary_core::hash::sha256(b""),
                size: 0,
            }),
            cas_hash: None,
        }],
        "incoming",
        &[],
        InstallSemantics::native_package(PackageFormatType::Rpm),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains(
            "incompatible with package owner: node kinds differ (regular versus directory)"
        ),
        "{error:#}"
    );
}

#[test]
fn real_directory_behavior_is_source_format_exact() {
    let selected_root = tempfile::tempdir().unwrap();
    physical_directory(selected_root.path(), "/shared");

    for (format, expected_materialization) in [
        (
            PackageFormatType::Deb,
            ExistingDirectoryMaterialization::PreserveExistingDirectoryOrSymlinkToDirectory,
        ),
        (
            PackageFormatType::Arch,
            ExistingDirectoryMaterialization::PreserveExistingDirectory,
        ),
    ] {
        assert!(matches!(
            plan_directory_path(
                selected_root.path(),
                "/shared",
                InstallSemantics::native_package(format),
            )
            .unwrap(),
            DirectoryPathPlan::PreserveLeaf {
                leaf_node,
                materialization,
            }
                if matches!(
                    leaf_node.source.kind,
                    PayloadNodeKind::Directory
                ) && materialization == expected_materialization
        ));
    }
    assert!(matches!(
        plan_directory_path(
            selected_root.path(),
            "/shared",
            InstallSemantics::native_package(PackageFormatType::Rpm),
        )
        .unwrap(),
        DirectoryPathPlan::ApplyDirectoryMetadata {
            materialization:
                ExistingDirectoryMaterialization::ApplyIncomingDirectoryOrSymlinkToDirectory,
            ..
        }
    ));
    assert!(matches!(
        plan_directory_path(
            selected_root.path(),
            "/shared",
            InstallSemantics::ccs(VersionScheme::Conary),
        )
        .unwrap(),
        DirectoryPathPlan::ApplyDirectoryMetadata {
            materialization: ExistingDirectoryMaterialization::ApplyIncoming,
            ..
        }
    ));
}

#[test]
fn symlink_to_directory_behavior_is_source_format_exact() {
    let selected_root = tempfile::tempdir().unwrap();
    physical_directory(selected_root.path(), "/real");
    physical_symlink(selected_root.path(), "/shared", "/real");

    assert!(matches!(
        plan_directory_path(
            selected_root.path(),
            "/shared",
            InstallSemantics::native_package(PackageFormatType::Deb),
        )
        .unwrap(),
        DirectoryPathPlan::PreserveLeaf {
            leaf_node,
            materialization:
                ExistingDirectoryMaterialization::
                    PreserveExistingDirectoryOrSymlinkToDirectory,
        }
            if matches!(
                leaf_node.source.kind,
                PayloadNodeKind::Symlink { .. }
            )
    ));
    assert_eq!(
        plan_directory_path(
            selected_root.path(),
            "/shared",
            InstallSemantics::native_package(PackageFormatType::Arch),
        )
        .unwrap(),
        DirectoryPathPlan::CreateOrReplace
    );
    assert!(matches!(
        plan_directory_path(
            selected_root.path(),
            "/shared",
            InstallSemantics::native_package(PackageFormatType::Rpm),
        )
        .unwrap(),
        DirectoryPathPlan::ApplyThroughSymlink {
            resolved_target_path,
            ..
        } if resolved_target_path == "/real"
    ));
    assert!(rpm_symlink_owner_may_apply(0, 1234));
    assert!(rpm_symlink_owner_may_apply(1234, 1234));
    assert!(!rpm_symlink_owner_may_apply(1234, 5678));
}

#[test]
fn dangling_symlink_is_replaced_instead_of_treated_as_directory() {
    let selected_root = tempfile::tempdir().unwrap();
    physical_symlink(selected_root.path(), "/shared", "/missing");

    for format in [
        PackageFormatType::Rpm,
        PackageFormatType::Deb,
        PackageFormatType::Arch,
    ] {
        assert_eq!(
            plan_directory_path(
                selected_root.path(),
                "/shared",
                InstallSemantics::native_package(format),
            )
            .unwrap(),
            DirectoryPathPlan::CreateOrReplace
        );
    }
}

#[test]
fn live_directory_never_reclassifies_a_peer_owned_regular_database_row() {
    let (_db_root, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(db_path).unwrap();
    let owner_id = insert_trove(&conn, "owner");
    let mut regular = empty_regular_entry("/shared", owner_id);
    regular.insert(&conn).unwrap();
    let selected_root = tempfile::tempdir().unwrap();
    physical_directory(selected_root.path(), "/shared");

    let error = preflight_resolved_file_ownership(
        &conn,
        selected_root.path(),
        &[resolved_file("/shared", resolved_directory(0o755))],
        "incoming",
        &[],
        InstallSemantics::native_package(PackageFormatType::Deb),
    )
    .unwrap_err();

    assert!(
        error.to_string().contains(&format!(
            "violates package {owner_id} payload anchor policy 'exact'"
        )),
        "{error:#}"
    );
    assert!(matches!(
        FileEntry::find_by_path(&conn, "/shared")
            .unwrap()
            .unwrap()
            .node
            .source
            .kind,
        PayloadNodeKind::Regular { .. }
    ));
}

#[test]
fn compatible_rpm_target_extends_one_materialized_hardlink_graph() {
    let (_db_root, db_path) = crate::commands::test_helpers::create_test_db();
    let mut conn = conary_core::db::open(db_path).unwrap();
    let anchor_owner = insert_trove(&conn, "anchor-owner");
    let incoming_owner = insert_trove(&conn, "incoming");
    let target = "/usr/share/shared-target";
    let content = PayloadContentAuthority {
        sha256: conary_core::hash::sha256(b"shared"),
        size: 6,
    };
    let regular = ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap();
    let mut anchor = FileEntry::new(
        target.to_string(),
        regular.clone(),
        Some(content.clone()),
        anchor_owner,
    )
    .with_claim_policy(PayloadSharingPolicy::Rpm);
    anchor.insert(&conn).unwrap();

    let mut edge_node = regular.clone();
    edge_node.source.kind = PayloadNodeKind::Hardlink {
        target: target.to_string(),
        identity: "rpm:9:42".to_string(),
    };
    let incoming = vec![
        ResolvedInstallFile {
            path: target.to_string(),
            node: regular,
            content: Some(content.clone()),
            cas_hash: Some(content.sha256.clone()),
        },
        ResolvedInstallFile {
            path: "/usr/share/incoming-edge".to_string(),
            node: edge_node,
            content: None,
            cas_hash: None,
        },
    ];
    let selected_root = tempfile::tempdir().unwrap();
    let plan = preflight_resolved_file_ownership(
        &conn,
        selected_root.path(),
        &incoming,
        "incoming",
        &[],
        InstallSemantics::native_package(PackageFormatType::Rpm),
    )
    .unwrap();
    assert!(plan.preserves_leaf(target));

    let all_live_files = vec![
        crate::commands::LiveRootFile {
            path: target.to_string(),
            content: crate::commands::LiveRootContent::from_in_memory_bytes(b"shared"),
            node: incoming[0].node.clone(),
        },
        crate::commands::LiveRootFile {
            path: incoming[1].path.clone(),
            content: crate::commands::LiveRootContent::absent(),
            node: incoming[1].node.clone(),
        },
    ];
    let mut mutation_files = all_live_files
        .iter()
        .filter(|file| !plan.preserves_leaf(&file.path))
        .cloned()
        .collect::<Vec<_>>();
    let references = super::super::execute::prepare_preserved_hardlink_references(
        &conn,
        &plan,
        &all_live_files,
        &mut mutation_files,
    )
    .unwrap();
    let identity = format!("path:{target}");
    assert_eq!(references.len(), 1);
    assert_eq!(
        references[0].node.source.kind,
        PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.clone())
        }
    );
    assert_eq!(
        mutation_files[0].node.source.kind,
        PayloadNodeKind::Hardlink {
            target: target.to_string(),
            identity: identity.clone()
        }
    );

    let tx = conn.transaction().unwrap();
    for file in &incoming {
        let mut entry = FileEntry::new(
            file.path.clone(),
            file.node.clone(),
            file.content.clone(),
            incoming_owner,
        )
        .with_claim_policy(PayloadSharingPolicy::Rpm);
        super::super::inner::insert_file_entry_claiming_live_root_overlap(
            &tx,
            &mut entry,
            "incoming",
            &[],
            plan.materialization_for_path(&file.path),
            plan.leaf_before(&file.path),
        )
        .unwrap();
    }
    super::super::inner::reconcile_installed_hardlink_materializations(&tx, &incoming).unwrap();
    tx.commit().unwrap();

    assert_eq!(PayloadClaim::find_by_path(&conn, target).unwrap().len(), 2);
    assert_eq!(
        FileEntry::find_by_path(&conn, target)
            .unwrap()
            .unwrap()
            .node
            .source
            .kind,
        PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.clone())
        }
    );
    assert_eq!(
        FileEntry::find_by_path(&conn, "/usr/share/incoming-edge")
            .unwrap()
            .unwrap()
            .node
            .source
            .kind,
        PayloadNodeKind::Hardlink {
            target: target.to_string(),
            identity
        }
    );
    conary_core::db::models::PackagePayloadOwnership::load(&conn, anchor_owner).unwrap();
    conary_core::db::models::PackagePayloadOwnership::load(&conn, incoming_owner).unwrap();
}

#[test]
fn peer_owned_symlink_anchor_accepts_a_debian_payload_claim() {
    let (_db_root, db_path) = crate::commands::test_helpers::create_test_db();
    let mut conn = conary_core::db::open(db_path).unwrap();
    let symlink_owner = insert_trove(&conn, "symlink-owner");
    let incoming_owner = insert_trove(&conn, "incoming");
    let selected_root = tempfile::tempdir().unwrap();
    physical_directory(selected_root.path(), "/real");
    physical_symlink(selected_root.path(), "/shared", "/real");
    let leaf = conary_core::filesystem::selected_root::capture_selected_root_node(
        selected_root.path(),
        "/shared",
    )
    .unwrap()
    .unwrap();
    let mut anchor = FileEntry::new("/shared".to_string(), leaf.clone(), None, symlink_owner);
    anchor.insert(&conn).unwrap();
    let incoming = resolved_file("/shared", resolved_directory(0o700));
    let plan = preflight_resolved_file_ownership(
        &conn,
        selected_root.path(),
        std::slice::from_ref(&incoming),
        "incoming",
        &[],
        InstallSemantics::native_package(PackageFormatType::Deb),
    )
    .unwrap();

    let tx = conn.transaction().unwrap();
    let mut entry = FileEntry::new(
        incoming.path.clone(),
        incoming.node.clone(),
        None,
        incoming_owner,
    );
    super::super::inner::insert_file_entry_claiming_live_root_overlap(
        &tx,
        &mut entry,
        "incoming",
        &[],
        plan.materialization_for_path(&incoming.path),
        plan.leaf_before(&incoming.path),
    )
    .unwrap();
    plan.persist_through_symlink_target(&tx, &incoming, incoming_owner)
        .unwrap();
    tx.commit().unwrap();

    let materialized = FileEntry::find_by_path(&conn, "/shared").unwrap().unwrap();
    assert_eq!(materialized.trove_id, symlink_owner);
    assert_eq!(materialized.node, leaf);
    let claims = PayloadClaim::find_by_path(&conn, "/shared").unwrap();
    assert_eq!(claims.len(), 2);
    let symlink_claim = claims
        .iter()
        .find(|claim| claim.trove_id == symlink_owner)
        .unwrap();
    assert_eq!(symlink_claim.anchor_policy, PayloadClaimAnchorPolicy::Exact);
    let incoming_claim = claims
        .iter()
        .find(|claim| claim.trove_id == incoming_owner)
        .unwrap();
    assert_eq!(
        incoming_claim.anchor_policy,
        PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory
    );
}

#[test]
fn rpm_through_symlink_reconciles_tracked_target_and_retains_leaf() {
    let (_db_root, db_path) = crate::commands::test_helpers::create_test_db();
    let mut conn = conary_core::db::open(db_path).unwrap();
    let target_owner = insert_trove(&conn, "target-owner");
    let symlink_owner = insert_trove(&conn, "symlink-owner");
    let incoming_owner = insert_trove(&conn, "incoming");
    let selected_root = tempfile::tempdir().unwrap();
    physical_directory(selected_root.path(), "/real");
    physical_symlink(selected_root.path(), "/shared", "/real");
    let target_before = conary_core::filesystem::selected_root::capture_selected_root_node(
        selected_root.path(),
        "/real",
    )
    .unwrap()
    .unwrap();
    let leaf = conary_core::filesystem::selected_root::capture_selected_root_node(
        selected_root.path(),
        "/shared",
    )
    .unwrap()
    .unwrap();
    let mut target_anchor = FileEntry::new(
        "/real".to_string(),
        target_before.clone(),
        None,
        target_owner,
    );
    target_anchor.insert(&conn).unwrap();
    let mut symlink_anchor =
        FileEntry::new("/shared".to_string(), leaf.clone(), None, symlink_owner);
    symlink_anchor.insert(&conn).unwrap();
    let incoming = resolved_file("/shared", resolved_directory(0o700));
    let plan = preflight_resolved_file_ownership(
        &conn,
        selected_root.path(),
        std::slice::from_ref(&incoming),
        "incoming",
        &[],
        InstallSemantics::native_package(PackageFormatType::Rpm),
    )
    .unwrap();

    let tx = conn.transaction().unwrap();
    plan.reconcile_through_symlink_target(&tx, &incoming)
        .unwrap();
    let mut entry = FileEntry::new(
        incoming.path.clone(),
        incoming.node.clone(),
        None,
        incoming_owner,
    );
    super::super::inner::insert_file_entry_claiming_live_root_overlap(
        &tx,
        &mut entry,
        "incoming",
        &[],
        plan.materialization_for_path(&incoming.path),
        plan.leaf_before(&incoming.path),
    )
    .unwrap();
    plan.persist_through_symlink_target(&tx, &incoming, incoming_owner)
        .unwrap();
    tx.commit().unwrap();

    assert_eq!(
        FileEntry::find_by_path(&conn, "/real")
            .unwrap()
            .unwrap()
            .node,
        incoming.node
    );
    assert_eq!(
        FileEntry::find_by_path(&conn, "/shared")
            .unwrap()
            .unwrap()
            .node,
        leaf
    );
    let incoming_claim = PayloadClaim::find_by_path(&conn, "/shared")
        .unwrap()
        .into_iter()
        .find(|claim| claim.trove_id == incoming_owner)
        .unwrap();
    assert_eq!(
        incoming_claim.materialization_target_path.as_deref(),
        Some("/real")
    );
    assert!(
        conary_core::db::models::PackagePayloadOwnership::load(&conn, target_owner)
            .unwrap()
            .materialized_removal_paths()
            .is_empty(),
        "the original target owner cannot remove a directory retained by the RPM through-link claim"
    );

    Trove::delete(&conn, target_owner).unwrap();
    assert_eq!(
        FileEntry::find_by_path(&conn, "/real")
            .unwrap()
            .unwrap()
            .trove_id,
        incoming_owner
    );
    assert_eq!(
        conary_core::db::models::PackagePayloadOwnership::load(&conn, incoming_owner)
            .unwrap()
            .materialized_removal_paths(),
        &["/real".to_string()]
    );

    Trove::delete(&conn, incoming_owner).unwrap();
    assert!(FileEntry::find_by_path(&conn, "/real").unwrap().is_none());
    assert!(FileEntry::find_by_path(&conn, "/shared").unwrap().is_some());
}

#[test]
fn rollback_capture_precedes_a_later_batch_symlink_target_mutation() {
    let (_db_root, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(db_path).unwrap();
    let owner = insert_trove(&conn, "owner");
    let selected_root = tempfile::tempdir().unwrap();
    physical_directory(selected_root.path(), "/real");
    let selected_node = conary_core::filesystem::selected_root::capture_selected_root_node(
        selected_root.path(),
        "/real",
    )
    .unwrap()
    .unwrap();
    let mut stale = selected_node.clone();
    stale.source.mode = libc::S_IFDIR | 0o700;
    let mut anchor = FileEntry::new("/real".to_string(), stale, None, owner);
    anchor.insert(&conn).unwrap();

    let captured =
        capture_existing_directory_materializations(&conn, selected_root.path()).unwrap();

    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].path, "/real");
    assert_eq!(captured[0].node, selected_node);

    physical_symlink(selected_root.path(), "/later-link", "/real");
    let incoming = resolved_file("/later-link", resolved_directory(0o711));
    let plan = preflight_resolved_file_ownership(
        &conn,
        selected_root.path(),
        std::slice::from_ref(&incoming),
        "later-rpm",
        &[],
        InstallSemantics::native_package(PackageFormatType::Rpm),
    )
    .unwrap();
    assert!(matches!(
        plan.path("/later-link"),
        Some(DirectoryPathPlan::ApplyThroughSymlink { .. })
    ));
    let tx = conn.unchecked_transaction().unwrap();
    plan.reconcile_through_symlink_target(&tx, &incoming)
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(
        FileEntry::find_by_path(&conn, "/real")
            .unwrap()
            .unwrap()
            .node,
        incoming.node
    );
    assert_eq!(
        captured[0].node, selected_node,
        "rollback authority remains the pre-batch target node"
    );
}

#[test]
fn rollback_capture_excludes_only_generation_ephemeral_payload_claims() {
    let (_db_root, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(db_path).unwrap();
    let owner = insert_trove(&conn, "root-layout");
    for path in ["/dev", "/usr"] {
        FileEntry::new(path.to_string(), resolved_directory(0o755), None, owner)
            .insert(&conn)
            .unwrap();
    }
    let selected_root = tempfile::tempdir().unwrap();
    physical_directory(selected_root.path(), "/usr");

    let captured =
        capture_existing_directory_materializations(&conn, selected_root.path()).unwrap();

    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].path, "/usr");
    assert!(
        classify_root_path("/dev").unwrap() == RootPathDomain::EphemeralMountOrUser,
        "the exclusion must remain bound to the exact generation root-domain contract"
    );
}

#[test]
fn rollback_capture_classifies_effective_selected_root_alias_domains() {
    let (_db_root, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(db_path).unwrap();
    let owner = insert_trove(&conn, "aliased-layout");
    for path in ["/var/run/faillock", "/lib/vendor"] {
        FileEntry::new(path.to_string(), resolved_directory(0o755), None, owner)
            .insert(&conn)
            .unwrap();
    }

    let selected_root = tempfile::tempdir().unwrap();
    physical_directory(selected_root.path(), "/var");
    physical_symlink(selected_root.path(), "/var/run", "../run");
    physical_directory(selected_root.path(), "/usr/lib/vendor");
    physical_symlink(selected_root.path(), "/lib", "usr/lib");

    let captured =
        capture_existing_directory_materializations(&conn, selected_root.path()).unwrap();

    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].path, "/lib/vendor");
    assert_eq!(
        captured[0].node,
        conary_core::filesystem::selected_root::capture_selected_root_node(
            selected_root.path(),
            "/lib/vendor",
        )
        .unwrap()
        .unwrap()
    );
}

#[test]
fn rollback_capture_still_rejects_missing_non_ephemeral_materialization() {
    let (_db_root, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(db_path).unwrap();
    let owner = insert_trove(&conn, "missing-layout");
    FileEntry::new(
        "/usr/lib/missing".to_string(),
        resolved_directory(0o755),
        None,
        owner,
    )
    .insert(&conn)
    .unwrap();

    let selected_root = tempfile::tempdir().unwrap();
    physical_directory(selected_root.path(), "/usr/lib");

    let error =
        capture_existing_directory_materializations(&conn, selected_root.path()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Tracked directory materialization /usr/lib/missing is absent"),
        "{error:#}"
    );
}

#[test]
fn pinned_upstream_directory_sources_are_exact() {
    assert_eq!(UPSTREAM_DIRECTORY_MATERIALIZATION_INPUTS.len(), 4);
    let mut formats = BTreeSet::new();
    let mut behaviors = BTreeSet::new();
    let mut urls = BTreeSet::new();
    for source in UPSTREAM_DIRECTORY_MATERIALIZATION_INPUTS {
        formats.insert(source.format.name());
        assert!(behaviors.insert(source.behavior));
        assert!(urls.insert(source.url));
        assert!(!source.implementation.is_empty());
        assert!(source.url.contains(source.revision));
        assert!(source.url.ends_with(source.path));
        assert_eq!(source.sha256.len(), 64);
        assert!(source.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(matches!(
            (source.format, source.behavior),
            (
                PackageFormatType::Rpm,
                UpstreamDirectoryBehavior::RpmInstallAppliesDirectoryMetadata
            ) | (
                PackageFormatType::Deb,
                UpstreamDirectoryBehavior::DpkgUnpackPreservesDirectoryOrSymlinkToDirectory
            ) | (
                PackageFormatType::Deb,
                UpstreamDirectoryBehavior::DpkgRemoveDeletesOnlyUnsharedDirectoryOrSymlink
            ) | (
                PackageFormatType::Arch,
                UpstreamDirectoryBehavior::LibalpmInstallPreservesDirectoryAndReplacesSymlink
            )
        ));
    }
    assert_eq!(formats, BTreeSet::from(["arch", "deb", "rpm"]));
    assert_eq!(behaviors.len(), 4);
}

#[tokio::test]
#[ignore = "network conformance proof for immutable upstream package-manager sources"]
async fn pinned_upstream_directory_source_digests_match() {
    for source in UPSTREAM_DIRECTORY_MATERIALIZATION_INPUTS {
        let response = reqwest::get(source.url)
            .await
            .unwrap_or_else(|error| panic!("fetch {}: {error}", source.url))
            .error_for_status()
            .unwrap_or_else(|error| panic!("fetch {}: {error}", source.url));
        let bytes = response
            .bytes()
            .await
            .unwrap_or_else(|error| panic!("read {}: {error}", source.url));
        assert_eq!(
            conary_core::hash::sha256(&bytes),
            source.sha256,
            "{} {} changed at {}",
            source.implementation,
            source.revision,
            source.path
        );
    }
}

#[test]
fn captured_root_authority_is_typed_and_cannot_be_spoofed_by_package_name() {
    let (_root, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(db_path).unwrap();
    let spoofed_id = insert_trove(&conn, "conary-live-root");
    let mut spoofed = empty_regular_entry("/boot/spoofed", spoofed_id);
    spoofed.insert(&conn).unwrap();
    assert!(!owner_allows_replacement(&conn, &spoofed, "incoming", &[]).unwrap());

    let mut captured = Trove::new_with_source(
        "captured-root-authority".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::CapturedRoot,
        VersionScheme::Conary,
    );
    let captured_id = captured.insert(&conn).unwrap();
    let captured_file = empty_regular_entry("/boot/captured", captured_id);
    assert!(owner_allows_replacement(&conn, &captured_file, "incoming", &[]).unwrap());
}
