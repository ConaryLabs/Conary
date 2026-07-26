// apps/conary/src/commands/install/native_events/tests/deconfiguration.rs

//! Exact Debian relation-deconfiguration preparation and state coverage.

use super::*;
use conary_core::ccs::native_transaction::NativeTransactionStep;
use conary_core::payload::{PayloadContentAuthority, PayloadNode, ResolvedPayloadNode};
use conary_core::transaction::{
    PackageRelationDeconfiguration, PackageRelationDeconfigurationCause,
    PackageRelationInstalledIdentity,
};

fn deconfiguration_arguments() -> Vec<DebMaintainerArgumentMetadata> {
    vec![
        DebMaintainerArgumentMetadata {
            index: 1,
            name: "action".to_string(),
            value: DebMaintainerArgumentValue::Action,
            literal: None,
            required: true,
        },
        DebMaintainerArgumentMetadata {
            index: 2,
            name: "in-favour".to_string(),
            value: DebMaintainerArgumentValue::Literal,
            literal: Some("in-favour".to_string()),
            required: true,
        },
        DebMaintainerArgumentMetadata {
            index: 3,
            name: "installing-package".to_string(),
            value: DebMaintainerArgumentValue::InstallingPackageName,
            literal: None,
            required: true,
        },
        DebMaintainerArgumentMetadata {
            index: 4,
            name: "installing-version".to_string(),
            value: DebMaintainerArgumentValue::InstallingPackageVersion,
            literal: None,
            required: true,
        },
        DebMaintainerArgumentMetadata {
            index: 5,
            name: "removing".to_string(),
            value: DebMaintainerArgumentValue::ConflictingPackageMarker,
            literal: None,
            required: false,
        },
        DebMaintainerArgumentMetadata {
            index: 6,
            name: "conflicting-package".to_string(),
            value: DebMaintainerArgumentValue::ConflictingPackageName,
            literal: None,
            required: false,
        },
        DebMaintainerArgumentMetadata {
            index: 7,
            name: "conflicting-version".to_string(),
            value: DebMaintainerArgumentValue::ConflictingPackageVersion,
            literal: None,
            required: false,
        },
    ]
}

fn deconfigured_bundle() -> NativeLifecycleBundle {
    let mut bundle = pre_remove_bundle("dependent", "1");
    bundle.source_format = SourceFormat::Deb;
    bundle.source_family = "debian".to_string();
    bundle.source_profile = Some("ubuntu-26.04".to_string());
    bundle.source_release = Some("13".to_string());
    bundle.source_arch = Some("amd64".to_string());
    bundle.version_scheme = LifecycleVersionScheme::Deb;
    let mut prerm = deb_entry(
        "deb:prerm",
        LifecyclePath::PreRemove,
        DebControlMember::Prerm,
        DebMaintainerMode::Deconfigure,
        "/bin/sh",
    );
    prerm.deb_maintainer.as_mut().unwrap().invocations[0].arguments = deconfiguration_arguments();
    let mut postinst = deb_entry(
        "deb:postinst",
        LifecyclePath::PostInstall,
        DebControlMember::Postinst,
        DebMaintainerMode::AbortDeconfigure,
        "/bin/sh",
    );
    postinst.deb_maintainer.as_mut().unwrap().invocations[0].arguments =
        deconfiguration_arguments();
    bundle.entries = vec![prerm, postinst];
    bundle
}

#[test]
fn relation_deconfiguration_prepares_exact_debian_operation_event_and_state() {
    let (_temp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut dependent = Trove::new(
        "dependent".to_string(),
        "1".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Debian,
    );
    dependent.architecture = Some("amd64".to_string());
    dependent.debian_multi_arch =
        Some(conary_core::repository::dependency_model::DebianMultiArch::No);
    let dependent_id = dependent.insert(&conn).unwrap();
    FileEntry::new(
        "/usr/bin/dependent".to_string(),
        ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o755)).unwrap(),
        Some(PayloadContentAuthority {
            sha256: conary_core::hash::sha256(b"dependent"),
            size: b"dependent".len() as u64,
        }),
        dependent_id,
    )
    .insert(&conn)
    .unwrap();
    InstalledNativeLifecycleBundle::new(dependent_id, None, &deconfigured_bundle())
        .unwrap()
        .insert_or_replace(&conn)
        .unwrap();

    let deconfigurations = vec![PackageRelationDeconfiguration {
        package: PackageRelationInstalledIdentity {
            trove_id: dependent_id,
            package_name: "dependent".to_string(),
            package_version: "1".to_string(),
            package_architecture: Some("amd64".to_string()),
        },
        triggering_incoming: PackageRelationIncomingIdentity {
            transaction_index: 0,
            package_name: "incoming".to_string(),
            package_version: "2".to_string(),
            package_architecture: Some("amd64".to_string()),
        },
        cause: PackageRelationDeconfigurationCause::Breaks {
            native_text: Some("dependent (<< 2)".to_string()),
        },
        dependency_depth: 0,
    }];
    let prepared = PreparedNativeTransaction::prepare_install(
        &conn,
        NativeInstallInput {
            package_name: "incoming",
            package_version: "2",
            package_arch: Some("amd64"),
            version_scheme: VersionScheme::Debian,
            provides: &[],
            new_bundle: None,
            old_trove: None,
            relation_removals: &[],
            relation_deconfigurations: &deconfigurations,
            paths: Vec::new(),
        },
    )
    .unwrap();

    assert!(prepared.requires_upgrade_payload_boundary);
    assert!(matches!(
        prepared.operations.as_slice(),
        [
            NativeTransactionOperation::Install,
            NativeTransactionOperation::DeconfigureInFavour {
                installing_transaction_index: 0,
                removing_conflict_transaction_index: None,
            }
        ]
    ));
    let owner = prepared
        .owners
        .iter()
        .find(|owner| owner.package_name == "dependent")
        .expect("installed deconfiguration owner");
    assert_eq!(owner.role, NativeBundleRole::Deconfiguring);
    assert_eq!(owner.instances_after, 1);

    let event = prepared
        .plan
        .events_at(NativeEventStage::DebPreDeconfigure)
        .next()
        .expect("deconfiguration event");
    assert_eq!(event.owner_package, "dependent");
    assert_eq!(
        event.args,
        vec!["deconfigure", "in-favour", "incoming", "2"]
    );
    assert_eq!(
        event.placement,
        NativeEventPlacement::TransactionElement {
            transaction_index: 0,
        }
    );
    assert_eq!(
        prepared
            .plan
            .graph
            .steps
            .iter()
            .filter(|step| matches!(step, NativeTransactionStep::ApplyPayload { .. }))
            .count(),
        1,
        "deconfiguration must not invent a payload mutation"
    );

    prepared.persist_event_success(&conn, event).unwrap();
    let stored = InstalledNativeLifecycleBundle::find_by_trove(&conn, dependent_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.lifecycle_state, DebPackageState::Unpacked);
    prepared
        .persist_event_failure(&conn, event, DebPackageState::HalfConfigured)
        .unwrap();
    let stored = InstalledNativeLifecycleBundle::find_by_trove(&conn, dependent_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.lifecycle_state, DebPackageState::HalfConfigured);
}
