// apps/conary/src/commands/system/rollback_restore/tests.rs

use super::{restore_snapshot, restore_snapshots};
use crate::commands::installed_authority_snapshot::{
    DirectoryAnchorSnapshot, DirectoryClaimSnapshot, ProvideSnapshot, TroveSnapshot,
};
use crate::commands::remove::snapshot_trove;
use conary_core::capability::{CapabilityDeclaration, store_capabilities};
use conary_core::ccs::manifest::FileCapability;
use conary_core::ccs::native_lifecycle::{
    NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeLifecycleBundle,
    ScriptletFidelity, SourceFormat,
};
use conary_core::ccs::native_transaction::DebPackageState;
use conary_core::db::models::{
    Changeset, CollectionMember, Component, ComponentDepType, ComponentDependency,
    ComponentProvide, ConfigBackup, ConfigFile, ConfigSource, ConfigStatus, ConvertedPackage,
    DirectoryClaim, DirectoryClaimAnchorPolicy, FileEntry, Flavor, InstallReason, InstallSource,
    InstalledCcsRemoveHook, InstalledFileCapability, InstalledNativeLifecycleBundle,
    InstalledRequirementGroup, LabelEntry, NativeLifecycleResidualState, ProvideEntry, Repository,
    Trove, TroveType,
};
use conary_core::packages::InstalledPackageIdentity;
use conary_core::payload::{
    PayloadContentAuthority, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
};
use conary_core::repository::dependency_model::{
    DebianMultiArch, ProvideArchitectureQualifier, ProvideVersionRelation,
    RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementGroup,
    RepositoryRequirementKind,
};
use conary_core::repository::versioning::VersionScheme;
use rusqlite::params;

const INSTALLED_AT: &str = "2025-02-03T04:05:06Z";

#[test]
fn rollback_restores_release_multi_arch_and_provider_relation_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let mut rollback_changeset = Changeset::new("restore exact package identity".to_string());
    let rollback_changeset_id = rollback_changeset.insert(&conn).unwrap();

    let mut snapshot = TroveSnapshot::test_package("portable-data", "1", Vec::new());
    snapshot.package_release = Some("7".to_string());
    snapshot.version_scheme = VersionScheme::Debian;
    snapshot.architecture = Some("amd64".to_string());
    snapshot.debian_multi_arch = Some(DebianMultiArch::Same);
    snapshot.provides = vec![ProvideSnapshot {
        capability: "portable-data-abi".to_string(),
        version: Some("1".to_string()),
        version_relation: Some(ProvideVersionRelation::Equal),
        kind: RepositoryCapabilityKind::Virtual,
        version_scheme: VersionScheme::Debian,
        architecture_qualifier: ProvideArchitectureQualifier::Any,
    }];

    let tx = conn.transaction().unwrap();
    restore_snapshot(&tx, rollback_changeset_id, &snapshot).unwrap();
    tx.commit().unwrap();

    let restored = Trove::find_one_by_name(&conn, "portable-data")
        .unwrap()
        .unwrap();
    assert_eq!(restored.package_release.as_deref(), Some("7"));
    assert_eq!(restored.architecture.as_deref(), Some("amd64"));
    assert_eq!(restored.debian_multi_arch, Some(DebianMultiArch::Same));
    let provide = ProvideEntry::find_by_trove(&conn, restored.id.unwrap()).unwrap();
    assert_eq!(provide.len(), 1);
    assert_eq!(
        provide[0].version_relation,
        Some(ProvideVersionRelation::Equal)
    );
    assert_eq!(snapshot_trove(&conn, &restored).unwrap(), snapshot);
}

#[test]
fn verified_symlink_directory_anchor_policy_round_trips_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let mut rollback_changeset = Changeset::new("restore symlink directory anchor".to_string());
    let rollback_changeset_id = rollback_changeset.insert(&conn).unwrap();
    let symlink_node = ResolvedPayloadNode::from_numeric_source(PayloadNode {
        kind: PayloadNodeKind::Symlink {
            target: "usr/lib".to_string(),
        },
        mode: libc::S_IFLNK | 0o777,
        ..PayloadNode::regular(0o777)
    })
    .unwrap();
    let claim_node = ResolvedPayloadNode::from_numeric_source(PayloadNode {
        kind: PayloadNodeKind::Directory,
        mode: libc::S_IFDIR | 0o755,
        ..PayloadNode::regular(0o755)
    })
    .unwrap();
    let mut snapshot = TroveSnapshot::test_package(
        "symlink-anchor",
        "1",
        vec![crate::commands::FileSnapshot {
            path: "/lib".to_string(),
            node: symlink_node.clone(),
            content: None,
            installed_at: INSTALLED_AT.to_string(),
            component: None,
        }],
    );
    snapshot.directory_claims = vec![DirectoryClaimSnapshot {
        path: "/lib".to_string(),
        node: claim_node,
        component: None,
        anchor_policy: DirectoryClaimAnchorPolicy::DirectoryOrSymlinkToDirectory,
        materialization_target_path: None,
        claimed_at: INSTALLED_AT.to_string(),
        anchor: Some(DirectoryAnchorSnapshot {
            materialized_node: symlink_node,
            installed_at: INSTALLED_AT.to_string(),
            component: None,
        }),
    }];
    let mut invalid = snapshot.clone();
    invalid.directory_claims[0].anchor_policy = DirectoryClaimAnchorPolicy::Directory;
    assert!(
        invalid
            .validate()
            .unwrap_err()
            .to_string()
            .contains("not accepted by policy")
    );
    let mut invalid_identity = snapshot.clone();
    invalid_identity.architecture = Some(String::new());
    assert!(
        invalid_identity
            .validate()
            .unwrap_err()
            .to_string()
            .contains("empty architecture")
    );

    let tx = conn.transaction().unwrap();
    restore_snapshots(&tx, rollback_changeset_id, std::slice::from_ref(&snapshot)).unwrap();
    tx.commit().unwrap();

    let restored = Trove::find_one_by_name(&conn, "symlink-anchor")
        .unwrap()
        .unwrap();
    assert!(matches!(
        FileEntry::find_by_path(&conn, "/lib")
            .unwrap()
            .unwrap()
            .node
            .source
            .kind,
        PayloadNodeKind::Symlink { .. }
    ));
    assert_eq!(
        DirectoryClaim::find_by_path(&conn, "/lib").unwrap()[0].anchor_policy,
        DirectoryClaimAnchorPolicy::DirectoryOrSymlinkToDirectory
    );
    assert_eq!(snapshot_trove(&conn, &restored).unwrap(), snapshot);
}

#[test]
fn rollback_reclaims_an_exact_peer_reanchored_symlink_without_unique_path_failure() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let mut symlink_owner = Trove::new(
        "symlink-owner".to_string(),
        "1".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    let symlink_owner_id = symlink_owner.insert(&conn).unwrap();
    let mut claimant = Trove::new(
        "directory-claimant".to_string(),
        "1".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    let claimant_id = claimant.insert(&conn).unwrap();
    let symlink_node = ResolvedPayloadNode::from_numeric_source(PayloadNode {
        kind: PayloadNodeKind::Symlink {
            target: "/real".to_string(),
        },
        mode: libc::S_IFLNK | 0o777,
        ..PayloadNode::regular(0o777)
    })
    .unwrap();
    let directory_node = ResolvedPayloadNode::from_numeric_source(PayloadNode {
        kind: PayloadNodeKind::Directory,
        mode: libc::S_IFDIR | 0o755,
        ..PayloadNode::regular(0o755)
    })
    .unwrap();
    FileEntry::new(
        "/shared".to_string(),
        symlink_node.clone(),
        None,
        symlink_owner_id,
    )
    .insert(&conn)
    .unwrap();
    DirectoryClaim::new("/shared".to_string(), claimant_id, directory_node)
        .unwrap()
        .with_anchor_policy(DirectoryClaimAnchorPolicy::DirectoryOrSymlinkToDirectory)
        .insert(&conn)
        .unwrap();
    let before = snapshot_trove(
        &conn,
        &Trove::find_by_id(&conn, symlink_owner_id).unwrap().unwrap(),
    )
    .unwrap();

    Trove::delete(&conn, symlink_owner_id).unwrap();
    assert_eq!(
        FileEntry::find_by_path(&conn, "/shared")
            .unwrap()
            .unwrap()
            .trove_id,
        claimant_id
    );

    let mut rollback_changeset = Changeset::new("restore symlink owner".to_string());
    let rollback_changeset_id = rollback_changeset.insert(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    restore_snapshot(&tx, rollback_changeset_id, &before).unwrap();
    tx.commit().unwrap();

    let restored = Trove::find_one_by_name(&conn, "symlink-owner")
        .unwrap()
        .unwrap();
    assert_eq!(
        FileEntry::find_by_path(&conn, "/shared")
            .unwrap()
            .unwrap()
            .trove_id,
        restored.id.unwrap()
    );
    assert_eq!(
        DirectoryClaim::find_by_path(&conn, "/shared")
            .unwrap()
            .into_iter()
            .map(|claim| claim.trove_id)
            .collect::<Vec<_>>(),
        vec![claimant_id]
    );
    assert_eq!(snapshot_trove(&conn, &restored).unwrap(), before);
}

#[test]
fn rollback_restores_a_target_owned_only_by_an_rpm_through_symlink_edge() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let mut target_owner = Trove::new(
        "target-owner".to_string(),
        "1".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    let target_owner_id = target_owner.insert(&conn).unwrap();
    let mut symlink_owner = Trove::new(
        "symlink-owner".to_string(),
        "1".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    let symlink_owner_id = symlink_owner.insert(&conn).unwrap();
    let mut rpm_claimant = Trove::new(
        "rpm-claimant".to_string(),
        "1".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    let rpm_claimant_id = rpm_claimant.insert(&conn).unwrap();
    let directory_node = ResolvedPayloadNode::from_numeric_source(PayloadNode {
        kind: PayloadNodeKind::Directory,
        mode: libc::S_IFDIR | 0o700,
        ..PayloadNode::regular(0o700)
    })
    .unwrap();
    FileEntry::new(
        "/real".to_string(),
        directory_node.clone(),
        None,
        target_owner_id,
    )
    .insert(&conn)
    .unwrap();
    FileEntry::new(
        "/shared".to_string(),
        ResolvedPayloadNode::from_numeric_source(PayloadNode {
            kind: PayloadNodeKind::Symlink {
                target: "/real".to_string(),
            },
            mode: libc::S_IFLNK | 0o777,
            ..PayloadNode::regular(0o777)
        })
        .unwrap(),
        None,
        symlink_owner_id,
    )
    .insert(&conn)
    .unwrap();
    DirectoryClaim::new("/shared".to_string(), rpm_claimant_id, directory_node)
        .unwrap()
        .with_anchor_policy(DirectoryClaimAnchorPolicy::DirectoryOrSymlinkToDirectory)
        .with_materialization_target(Some("/real".to_string()))
        .insert(&conn)
        .unwrap();

    Trove::delete(&conn, target_owner_id).unwrap();
    assert_eq!(
        FileEntry::find_by_path(&conn, "/real")
            .unwrap()
            .unwrap()
            .trove_id,
        rpm_claimant_id
    );
    let before = snapshot_trove(
        &conn,
        &Trove::find_by_id(&conn, rpm_claimant_id).unwrap().unwrap(),
    )
    .unwrap();
    assert_eq!(before.files.len(), 1);
    assert_eq!(before.files[0].path, "/real");
    assert_eq!(
        before.directory_claims[0]
            .materialization_target_path
            .as_deref(),
        Some("/real")
    );

    Trove::delete(&conn, rpm_claimant_id).unwrap();
    assert!(FileEntry::find_by_path(&conn, "/real").unwrap().is_none());

    let mut rollback_changeset = Changeset::new("restore rpm target edge".to_string());
    let rollback_changeset_id = rollback_changeset.insert(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    restore_snapshot(&tx, rollback_changeset_id, &before).unwrap();
    tx.commit().unwrap();

    let restored = Trove::find_one_by_name(&conn, "rpm-claimant")
        .unwrap()
        .unwrap();
    let restored_claim = DirectoryClaim::find_by_path(&conn, "/shared")
        .unwrap()
        .into_iter()
        .find(|claim| claim.trove_id == restored.id.unwrap())
        .unwrap();
    assert_eq!(
        restored_claim.materialization_target_path.as_deref(),
        Some("/real")
    );
    assert_eq!(
        FileEntry::find_by_path(&conn, "/real")
            .unwrap()
            .unwrap()
            .trove_id,
        restored.id.unwrap()
    );
    assert_eq!(snapshot_trove(&conn, &restored).unwrap(), before);
}

#[test]
fn cyclic_cross_directory_claims_restore_through_global_anchor_phase() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let mut rollback_changeset = Changeset::new("restore cyclic directory claims".to_string());
    let rollback_changeset_id = rollback_changeset.insert(&conn).unwrap();
    let directory_node = |mode| {
        ResolvedPayloadNode::from_numeric_source(PayloadNode {
            kind: PayloadNodeKind::Directory,
            mode: libc::S_IFDIR | mode,
            ..PayloadNode::regular(0o755)
        })
        .unwrap()
    };
    let claim = |path: &str, claim_mode: u32, anchor_mode: Option<u32>| DirectoryClaimSnapshot {
        path: path.to_string(),
        node: directory_node(claim_mode),
        component: None,
        anchor_policy: DirectoryClaimAnchorPolicy::Directory,
        materialization_target_path: None,
        claimed_at: INSTALLED_AT.to_string(),
        anchor: anchor_mode.map(|mode| DirectoryAnchorSnapshot {
            materialized_node: directory_node(mode),
            installed_at: INSTALLED_AT.to_string(),
            component: None,
        }),
    };
    let mut package_a = TroveSnapshot::test_package("cycle-a", "1", Vec::new());
    package_a.directory_claims = vec![
        claim("/opt", 0o700, None),
        claim("/usr", 0o755, Some(0o711)),
    ];
    let mut package_b = TroveSnapshot::test_package("cycle-b", "1", Vec::new());
    package_b.directory_claims = vec![
        claim("/opt", 0o750, Some(0o775)),
        claim("/usr", 0o700, None),
    ];
    let snapshots = vec![package_a.clone(), package_b.clone()];

    let tx = conn.transaction().unwrap();
    restore_snapshots(&tx, rollback_changeset_id, &snapshots).unwrap();
    tx.commit().unwrap();

    let restored_a = Trove::find_one_by_name(&conn, "cycle-a").unwrap().unwrap();
    let restored_b = Trove::find_one_by_name(&conn, "cycle-b").unwrap().unwrap();
    assert_eq!(
        FileEntry::find_by_path(&conn, "/usr")
            .unwrap()
            .unwrap()
            .trove_id,
        restored_a.id.unwrap()
    );
    assert_eq!(
        FileEntry::find_by_path(&conn, "/opt")
            .unwrap()
            .unwrap()
            .trove_id,
        restored_b.id.unwrap()
    );
    for path in ["/opt", "/usr"] {
        let claims = DirectoryClaim::find_by_path(&conn, path).unwrap();
        assert_eq!(claims.len(), 2, "{path}");
        assert!(
            claims
                .iter()
                .all(|claim| claim.claimed_at.as_deref() == Some(INSTALLED_AT))
        );
    }
    assert_eq!(snapshot_trove(&conn, &restored_a).unwrap(), package_a);
    assert_eq!(snapshot_trove(&conn, &restored_b).unwrap(), package_b);
}

#[test]
fn exact_installed_authority_round_trips_and_rejects_broken_relations() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let mut install_changeset = Changeset::new("install exact authority fixture".to_string());
    let install_changeset_id = install_changeset.insert(&conn).unwrap();

    let mut repository = Repository::new(
        "authority-repository".to_string(),
        "https://packages.example.invalid/authority".to_string(),
    );
    let repository_id = repository.insert(&conn).unwrap();
    let mut label = LabelEntry::new(
        "authority".to_string(),
        "testing".to_string(),
        "current".to_string(),
    );
    let label_id = label.insert(&conn).unwrap();

    let mut trove = Trove::new_with_source(
        "rollback-rpm-fixture".to_string(),
        "1.0-1".to_string(),
        TroveType::Package,
        InstallSource::Taken,
        VersionScheme::Rpm,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.description = Some("exact installed authority".to_string());
    trove.installed_by_changeset_id = Some(install_changeset_id);
    trove.install_reason = InstallReason::Dependency;
    trove.flavor_spec = Some("[ssl, !debug]".to_string());
    trove.pinned = true;
    trove.selection_reason = Some("required by authority-suite".to_string());
    trove.label_id = Some(label_id);
    trove.source_profile = Some("fedora-44".to_string());
    trove.native_package_identity = Some(
        InstalledPackageIdentity::rpm(
            "rollback-rpm-fixture-1.0-1.x86_64",
            "rollback-rpm-fixture",
            None,
            "1.0",
            "1",
            "x86_64",
        )
        .unwrap(),
    );
    trove.installed_from_repository_id = Some(repository_id);
    let trove_id = trove.insert(&conn).unwrap();
    conn.execute(
        "UPDATE troves SET installed_at = ?1, orphan_since = ?2 WHERE id = ?3",
        params![INSTALLED_AT, "2025-02-04T05:06:07Z", trove_id],
    )
    .unwrap();

    Flavor::new(trove_id, "ssl".to_string(), "enabled".to_string())
        .insert(&conn)
        .unwrap();
    let mut component = Component::new(trove_id, "runtime".to_string());
    component.description = Some("runtime payload".to_string());
    component.is_installed = false;
    let component_id = component.insert(&conn).unwrap();
    conn.execute(
        "UPDATE components SET installed_at = ?1 WHERE id = ?2",
        params![INSTALLED_AT, component_id],
    )
    .unwrap();
    let mut dependency = ComponentDependency::new_same_package(
        component_id,
        "config".to_string(),
        ComponentDepType::Runtime,
    );
    dependency.version_constraint = Some("= 1.0-1".to_string());
    dependency.insert(&conn).unwrap();
    let mut component_provide =
        ComponentProvide::new(component_id, "authority-runtime".to_string());
    component_provide.version = Some("1.0-1".to_string());
    component_provide.insert(&conn).unwrap();

    let hash = "a".repeat(64);
    conn.execute(
        "INSERT INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, 9)",
        params![&hash, "objects/aa/authority"],
    )
    .unwrap();
    let mut file = FileEntry::new_with_component(
        "/usr/bin/authority".to_string(),
        ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o755)).unwrap(),
        Some(PayloadContentAuthority {
            sha256: hash.clone(),
            size: 9,
        }),
        trove_id,
        component_id,
    );
    let file_id = file.insert(&conn).unwrap();
    conn.execute(
        "UPDATE files SET installed_at = ?1 WHERE id = ?2",
        params![INSTALLED_AT, file_id],
    )
    .unwrap();

    let requirement = RepositoryRequirementGroup::simple(
        RepositoryRequirementKind::Depends,
        RepositoryRequirementClause::versioned("libauthority".to_string(), ">= 2-1".to_string()),
    );
    InstalledRequirementGroup::from_group(trove_id, VersionScheme::Rpm, requirement)
        .unwrap()
        .insert(&conn)
        .unwrap();
    ProvideEntry::new_typed(
        trove_id,
        conary_core::repository::dependency_model::RepositoryCapabilityKind::Binary,
        "authority".to_string(),
        Some("1.0-1".to_string()),
        VersionScheme::Rpm,
        Default::default(),
    )
    .insert(&conn)
    .unwrap();

    let mut config = ConfigFile::new("/usr/bin/authority".to_string(), trove_id, hash.clone());
    config.file_id = Some(file_id);
    config.original_md5 = Some("0123456789abcdef0123456789abcdef".to_string());
    config.current_hash = Some("b".repeat(64));
    config.noreplace = true;
    config.remove_on_upgrade = false;
    config.status = ConfigStatus::Modified;
    config.modified_at = Some("2025-02-05T06:07:08Z".to_string());
    config.source = ConfigSource::Rpm;
    let config_id = config.insert(&conn).unwrap();
    let mut backup = ConfigBackup::new(config_id, "e".repeat(64), "exact-rollback".to_string());
    backup.changeset_id = Some(install_changeset_id);
    let backup_id = backup.insert(&conn).unwrap();
    conn.execute(
        "UPDATE config_backups SET created_at = ?1 WHERE id = ?2",
        params![INSTALLED_AT, backup_id],
    )
    .unwrap();

    let mut capabilities = CapabilityDeclaration::new();
    capabilities.network.none = true;
    capabilities.rationale = Some("round-trip authority".to_string());
    store_capabilities(&conn, trove_id, &capabilities).unwrap();
    conn.execute(
        "UPDATE capabilities SET declared_at = ?1 WHERE trove_id = ?2",
        params![INSTALLED_AT, trove_id],
    )
    .unwrap();
    InstalledFileCapability::replace_for_trove(
        &conn,
        trove_id,
        &[FileCapability {
            path: "/usr/bin/authority".to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        }],
    )
    .unwrap();
    conn.execute(
        "UPDATE installed_file_capabilities SET created_at = ?1 WHERE trove_id = ?2",
        params![INSTALLED_AT, trove_id],
    )
    .unwrap();

    let bundle = native_bundle();
    let mut lifecycle =
        InstalledNativeLifecycleBundle::new(trove_id, Some(install_changeset_id), &bundle).unwrap();
    lifecycle.lifecycle_state = DebPackageState::TriggersPending;
    lifecycle.pending_triggers = vec!["ldconfig".to_string()];
    lifecycle.insert_or_replace(&conn).unwrap();
    conn.execute(
        "UPDATE installed_native_lifecycle_bundles SET installed_at = ?1 WHERE trove_id = ?2",
        params![INSTALLED_AT, trove_id],
    )
    .unwrap();
    InstalledCcsRemoveHook::new(trove_id, "echo exact\n".to_string(), Some(true))
        .insert_or_replace(&conn)
        .unwrap();

    conn.execute(
        "INSERT INTO provenance (
            trove_id, source_url, upstream_url, rekor_log_index, dna_hash
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            trove_id,
            "https://src.example.invalid/authority",
            "https://upstream.example.invalid/authority",
            42_i64,
            "dna-authority"
        ],
    )
    .unwrap();
    let mut conversion =
        ConvertedPackage::new_installed(trove_id, "rpm".to_string(), "c".repeat(64));
    conversion.insert(&conn).unwrap();
    conn.execute(
        "UPDATE converted_packages
         SET converted_at = ?1, enhancement_version = 7,
             enhancement_status = 'complete', enhancement_error = 'retained-evidence',
             enhancement_attempted_at = ?2, enhancement_priority = 9
         WHERE trove_id = ?3",
        params![INSTALLED_AT, "2025-02-06T07:08:09Z", trove_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO derived_packages (
            name, parent_trove_id, parent_name, status, built_trove_id
         ) VALUES ('authority-derived', ?1, 'rollback-rpm-fixture', 'built', ?1)",
        [trove_id],
    )
    .unwrap();

    let mut collection = Trove::new(
        "authority-collection".to_string(),
        "1".to_string(),
        TroveType::Collection,
        VersionScheme::Conary,
    );
    collection.installed_by_changeset_id = Some(install_changeset_id);
    let collection_id = collection.insert(&conn).unwrap();
    CollectionMember::new(collection_id, "rollback-rpm-fixture".to_string())
        .with_version("1.0-1".to_string())
        .optional()
        .insert(&conn)
        .unwrap();

    let mut peer = Trove::new(
        "shared-directory-peer".to_string(),
        "1".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    peer.installed_by_changeset_id = Some(install_changeset_id);
    let peer_id = peer.insert(&conn).unwrap();
    let mut directory_node = PayloadNode::regular(0o755);
    directory_node.kind = PayloadNodeKind::Directory;
    directory_node.mode = libc::S_IFDIR | 0o755;
    let directory_node = ResolvedPayloadNode::from_numeric_source(directory_node).unwrap();
    let mut directory = FileEntry::new("/usr".to_string(), directory_node.clone(), None, trove_id);
    directory.component_id = Some(component_id);
    let directory_id = directory.insert(&conn).unwrap();
    conn.execute(
        "UPDATE files SET installed_at = ?1 WHERE id = ?2",
        params![INSTALLED_AT, directory_id],
    )
    .unwrap();
    let mut claim_component = Component::new(trove_id, "directory-claim".to_string());
    let claim_component_id = claim_component.insert(&conn).unwrap();
    DirectoryClaim::new("/usr".to_string(), trove_id, directory_node.clone())
        .unwrap()
        .with_component(Some(claim_component_id))
        .insert(&conn)
        .unwrap();
    DirectoryClaim::new("/usr".to_string(), peer_id, directory_node)
        .unwrap()
        .insert(&conn)
        .unwrap();
    conn.execute(
        "UPDATE directory_claims SET claimed_at = ?1 WHERE path = '/usr'",
        [INSTALLED_AT],
    )
    .unwrap();
    let directory_node_json_before: String = conn
        .query_row(
            "SELECT payload_node_json FROM files WHERE path = '/usr'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let before =
        snapshot_trove(&conn, &Trove::find_by_id(&conn, trove_id).unwrap().unwrap()).unwrap();
    let collection_before = snapshot_trove(
        &conn,
        &Trove::find_by_id(&conn, collection_id).unwrap().unwrap(),
    )
    .unwrap();
    ConfigFile::delete(&conn, config_id).unwrap();
    Trove::delete(&conn, trove_id).unwrap();
    Trove::delete(&conn, collection_id).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT parent_trove_id IS NULL AND built_trove_id IS NULL
             FROM derived_packages WHERE name = 'authority-derived'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );

    let mut residual = NativeLifecycleResidualState::new(
        &bundle,
        DebPackageState::ConfigFiles,
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    residual.upsert(&conn).unwrap();

    let mut broken = before.clone();
    broken.files[0].component = Some("missing-component".to_string());
    let failed = conn.transaction().unwrap();
    let error = restore_snapshot(&failed, install_changeset_id, &broken).unwrap_err();
    assert!(error.to_string().contains("missing component"));
    failed.rollback().unwrap();
    assert!(
        Trove::find_by_name(&conn, "rollback-rpm-fixture")
            .unwrap()
            .is_empty()
    );

    let mut rollback_changeset = Changeset::new("rollback exact authority fixture".to_string());
    let rollback_changeset_id = rollback_changeset.insert(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    restore_snapshot(&tx, rollback_changeset_id, &before).unwrap();
    restore_snapshot(&tx, rollback_changeset_id, &collection_before).unwrap();
    tx.commit().unwrap();

    let restored = Trove::find_one_by_name(&conn, "rollback-rpm-fixture")
        .unwrap()
        .unwrap();
    let after = snapshot_trove(&conn, &restored).unwrap();
    assert_eq!(after, before);
    assert_eq!(
        FileEntry::find_by_path(&conn, "/usr")
            .unwrap()
            .unwrap()
            .trove_id,
        restored.id.unwrap(),
        "rollback must restore the exact pre-removal canonical directory anchor"
    );
    let directory_node_json_after: String = conn
        .query_row(
            "SELECT payload_node_json FROM files WHERE path = '/usr'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        directory_node_json_after, directory_node_json_before,
        "rollback must preserve canonical payload-node JSON bytes"
    );
    assert_eq!(
        restored.installed_by_changeset_id,
        Some(rollback_changeset_id),
        "rollback is new causation rather than copied historical identity"
    );
    let restored_collection = Trove::find_one_by_name(&conn, "authority-collection")
        .unwrap()
        .unwrap();
    assert_eq!(
        snapshot_trove(&conn, &restored_collection).unwrap(),
        collection_before
    );
    assert!(
        NativeLifecycleResidualState::find_exact(
            &conn,
            bundle.source_format.as_str(),
            &conary_core::ccs::native_transaction::NativePackageIdentity::new(
                &bundle.source_package,
                &bundle.source_version,
                bundle.source_arch.as_deref(),
            ),
        )
        .unwrap()
        .is_none()
    );
}

fn native_bundle() -> NativeLifecycleBundle {
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora".to_string(),
        source_profile: Some("fedora-44".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: "rollback-rpm-fixture".to_string(),
        source_version: "1.0-1".to_string(),
        source_checksum: Some(format!("sha256:{}", "d".repeat(64))),
        version_scheme: conary_core::ccs::native_lifecycle::VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.7.1".to_string(),
        conversion_policy: "exact-authority-test".to_string(),
        evidence_digest: None,
        scriptlet_fidelity: ScriptletFidelity::NativeFree,
        entries: Vec::new(),
    }
}
