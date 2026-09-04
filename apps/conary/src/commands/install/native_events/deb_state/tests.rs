// apps/conary/src/commands/install/native_events/deb_state/tests.rs

use super::*;
use conary_core::ccs::native_lifecycle::{
    NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, ScriptletFidelity, VersionScheme,
};
use conary_core::ccs::native_transaction::{
    DebTriggerActivation, DebTriggerActivationBoundary, DebTriggerUnconfiguration,
};
use conary_core::db::models::{Trove, TroveType};
use conary_core::repository::dependency_model::DebianMultiArch;
use conary_core::repository::versioning::VersionScheme as RepositoryVersionScheme;

fn deb_bundle() -> conary_core::ccs::native_lifecycle::NativeLifecycleBundle {
    conary_core::ccs::native_lifecycle::NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Deb,
        source_family: "debian".to_string(),
        source_profile: Some("ubuntu-26.04".to_string()),
        source_release: Some("13".to_string()),
        source_arch: Some("amd64".to_string()),
        source_package: "fixture".to_string(),
        source_version: "1.0-1".to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Deb,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "1".to_string(),
        conversion_policy: "test".to_string(),
        evidence_digest: None,
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: Vec::new(),
    }
}

#[test]
fn successful_debian_install_without_postinst_reaches_terminal_state() {
    let (_tmp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut owners = Vec::new();

    for (package, state, pending, awaited) in [
        (
            "native-free",
            DebPackageState::Unpacked,
            Vec::new(),
            Vec::new(),
        ),
        (
            "pending-trigger",
            DebPackageState::TriggersPending,
            vec!["refresh-cache".to_string()],
            Vec::new(),
        ),
        (
            "awaiting-trigger",
            DebPackageState::Unpacked,
            Vec::new(),
            vec![NativePackageIdentity::new(
                "trigger-owner",
                "1.0-1",
                Some("amd64"),
            )],
        ),
    ] {
        let mut trove = Trove::new(
            package.to_string(),
            "1.0-1".to_string(),
            TroveType::Package,
            RepositoryVersionScheme::Debian,
        );
        trove.debian_multi_arch = Some(DebianMultiArch::No);
        let trove_id = trove.insert(&conn).unwrap();
        let mut bundle = deb_bundle();
        bundle.source_package = package.to_string();
        let mut installed = InstalledNativeLifecycleBundle::new(trove_id, None, &bundle).unwrap();
        installed.lifecycle_state = state;
        installed.pending_triggers = pending.clone();
        installed.awaited_packages = awaited.clone();
        installed.insert_or_replace(&conn).unwrap();
        owners.push(NativeBundleOwner {
            package_name: package.to_string(),
            package_version: "1.0-1".to_string(),
            instances_after: 1,
            role: NativeBundleRole::Installing,
            initial_package_state: DebPackageState::NotInstalled,
            initial_pending_triggers: Vec::new(),
            initial_awaited_packages: Vec::new(),
            bundle,
        });
    }

    let transaction = PreparedNativeTransaction {
        owners,
        operations: vec![NativeTransactionOperation::Install; 3],
        ..PreparedNativeTransaction::default()
    };
    transaction
        .finalize_successful_debian_installs(&conn)
        .unwrap();

    let states = transaction
        .owners
        .iter()
        .map(|owner| transaction.runtime_state(&conn, owner).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(states[0].lifecycle_state, DebPackageState::Installed);
    assert_eq!(states[1].lifecycle_state, DebPackageState::TriggersPending);
    assert_eq!(states[1].pending_triggers, ["refresh-cache"]);
    assert_eq!(states[2].lifecycle_state, DebPackageState::TriggersAwaited);
    assert_eq!(states[2].awaited_packages.len(), 1);
}

#[test]
fn removed_debian_bundle_retains_exact_state_without_an_installed_trove() {
    let (_tmp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let transaction = PreparedNativeTransaction {
        owners: vec![NativeBundleOwner {
            package_name: "fixture".to_string(),
            package_version: "1.0-1".to_string(),
            instances_after: 0,
            role: NativeBundleRole::Removing,
            initial_package_state: DebPackageState::Installed,
            initial_pending_triggers: vec!["fixture-trigger".to_string()],
            initial_awaited_packages: Vec::new(),
            bundle: deb_bundle(),
        }],
        operations: vec![NativeTransactionOperation::Remove],
        ..PreparedNativeTransaction::default()
    };

    let identity = NativePackageIdentity::new("fixture", "1.0-1", Some("amd64"));
    transaction
        .mark_remove_payload_started_for(&conn, &identity)
        .unwrap();
    let state: String = conn
        .query_row(
            "SELECT lifecycle_state FROM native_lifecycle_residual_states",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, DebPackageState::HalfInstalled.as_str());

    transaction
        .mark_remove_payload_completed_for(&conn, &identity, 0)
        .unwrap();
    let (state, pending): (String, String) = conn
        .query_row(
            "SELECT lifecycle_state, pending_triggers_json
                 FROM native_lifecycle_residual_states",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, DebPackageState::ConfigFiles.as_str());
    assert_eq!(pending, r#"["fixture-trigger"]"#);
}

#[test]
fn disappear_completion_deletes_residual_state() {
    let (_tmp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let identity = NativePackageIdentity::new("fixture", "1.0-1", Some("amd64"));
    let owner = || NativeBundleOwner {
        package_name: "fixture".to_string(),
        package_version: "1.0-1".to_string(),
        instances_after: 0,
        role: NativeBundleRole::Removing,
        initial_package_state: DebPackageState::Installed,
        initial_pending_triggers: Vec::new(),
        initial_awaited_packages: Vec::new(),
        bundle: deb_bundle(),
    };

    let disappear = PreparedNativeTransaction {
        owners: vec![owner()],
        operations: vec![NativeTransactionOperation::Disappear {
            overwriter_transaction_index: 1,
        }],
        ..PreparedNativeTransaction::default()
    };
    disappear
        .mark_remove_payload_started_for(&conn, &identity)
        .unwrap();
    disappear
        .mark_remove_payload_completed_for(&conn, &identity, 0)
        .unwrap();
    assert!(
        NativeLifecycleResidualState::find_exact(&conn, "deb", &identity)
            .unwrap()
            .is_none()
    );
}

#[test]
fn boundary_activation_persists_pending_and_incoming_await_state_idempotently() {
    let (_tmp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let interested = NativePackageIdentity::new("cache-owner", "1.0-1", Some("amd64"));
    let triggering = NativePackageIdentity::new("activator", "2.0-1", Some("amd64"));
    let mut interested_bundle = deb_bundle();
    interested_bundle.source_package = interested.package_name.clone();
    interested_bundle.source_version = interested.package_version.clone();
    let mut triggering_bundle = deb_bundle();
    triggering_bundle.source_package = triggering.package_name.clone();
    triggering_bundle.source_version = triggering.package_version.clone();
    let mut plan = conary_core::ccs::native_transaction::NativeTransactionPlan::default();
    plan.deb.trigger_activations.push(DebTriggerActivation {
        transaction_index: 7,
        boundary: DebTriggerActivationBoundary::BeforePayloadMutation,
        trigger_name: "refresh-cache".to_string(),
        triggering: triggering.clone(),
        interested: interested.clone(),
        awaited: true,
    });
    let transaction = PreparedNativeTransaction {
        owners: vec![
            NativeBundleOwner {
                package_name: interested.package_name.clone(),
                package_version: interested.package_version.clone(),
                instances_after: 1,
                role: NativeBundleRole::Installed,
                initial_package_state: DebPackageState::Installed,
                initial_pending_triggers: Vec::new(),
                initial_awaited_packages: Vec::new(),
                bundle: interested_bundle,
            },
            NativeBundleOwner {
                package_name: triggering.package_name.clone(),
                package_version: triggering.package_version.clone(),
                instances_after: 1,
                role: NativeBundleRole::Installing,
                initial_package_state: DebPackageState::NotInstalled,
                initial_pending_triggers: Vec::new(),
                initial_awaited_packages: Vec::new(),
                bundle: triggering_bundle,
            },
        ],
        plan,
        ..PreparedNativeTransaction::default()
    };

    for _ in 0..2 {
        transaction
            .persist_trigger_activations_for(
                &conn,
                7,
                DebTriggerActivationBoundary::BeforePayloadMutation,
            )
            .unwrap();
    }

    let interested_state = NativeLifecycleResidualState::find_exact(&conn, "deb", &interested)
        .unwrap()
        .unwrap();
    assert_eq!(
        interested_state.lifecycle_state,
        DebPackageState::TriggersPending
    );
    assert_eq!(interested_state.pending_triggers, ["refresh-cache"]);

    let triggering_state = NativeLifecycleResidualState::find_exact(&conn, "deb", &triggering)
        .unwrap()
        .unwrap();
    assert_eq!(
        triggering_state.lifecycle_state,
        DebPackageState::TriggersAwaited
    );
    assert_eq!(triggering_state.awaited_packages, [interested]);

    transaction
        .mark_install_payload_applied_for(&conn, &triggering)
        .unwrap();
    let triggering_state = NativeLifecycleResidualState::find_exact(&conn, "deb", &triggering)
        .unwrap()
        .unwrap();
    assert_eq!(triggering_state.lifecycle_state, DebPackageState::Unpacked);
    assert_eq!(
        triggering_state.awaited_packages,
        [NativePackageIdentity::new(
            "cache-owner",
            "1.0-1",
            Some("amd64")
        )]
    );
}

#[test]
fn unconfiguration_clears_pending_state_and_releases_reverse_awaiters() {
    let (_tmp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let interested = NativePackageIdentity::new("cache-owner", "1.0-1", Some("amd64"));
    let awaiter = NativePackageIdentity::new("awaiter", "1.0-1", Some("amd64"));
    let mut interested_bundle = deb_bundle();
    interested_bundle.source_package = interested.package_name.clone();
    let mut awaiter_bundle = deb_bundle();
    awaiter_bundle.source_package = awaiter.package_name.clone();
    let mut plan = conary_core::ccs::native_transaction::NativeTransactionPlan::default();
    plan.deb
        .trigger_unconfigurations
        .push(DebTriggerUnconfiguration {
            transaction_index: 9,
            boundary: DebTriggerActivationBoundary::BeforePayloadMutation,
            owner: interested.clone(),
        });
    let transaction = PreparedNativeTransaction {
        owners: vec![
            NativeBundleOwner {
                package_name: interested.package_name.clone(),
                package_version: interested.package_version.clone(),
                instances_after: 0,
                role: NativeBundleRole::Removing,
                initial_package_state: DebPackageState::TriggersPending,
                initial_pending_triggers: vec!["refresh-cache".to_string()],
                initial_awaited_packages: Vec::new(),
                bundle: interested_bundle,
            },
            NativeBundleOwner {
                package_name: awaiter.package_name.clone(),
                package_version: awaiter.package_version.clone(),
                instances_after: 1,
                role: NativeBundleRole::Installed,
                initial_package_state: DebPackageState::TriggersAwaited,
                initial_pending_triggers: Vec::new(),
                initial_awaited_packages: vec![interested.clone()],
                bundle: awaiter_bundle,
            },
        ],
        plan,
        ..PreparedNativeTransaction::default()
    };

    transaction
        .persist_trigger_activations_for(
            &conn,
            9,
            DebTriggerActivationBoundary::BeforePayloadMutation,
        )
        .unwrap();

    let interested_state = NativeLifecycleResidualState::find_exact(&conn, "deb", &interested)
        .unwrap()
        .unwrap();
    assert_eq!(interested_state.lifecycle_state, DebPackageState::Unpacked);
    assert!(interested_state.pending_triggers.is_empty());
    assert!(interested_state.awaited_packages.is_empty());

    let awaiter_state = NativeLifecycleResidualState::find_exact(&conn, "deb", &awaiter)
        .unwrap()
        .unwrap();
    assert_eq!(awaiter_state.lifecycle_state, DebPackageState::Installed);
    assert!(awaiter_state.awaited_packages.is_empty());
}
