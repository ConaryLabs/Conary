// apps/conary/src/commands/install/native_events/tests/provider_projection.rs

use super::*;

#[test]
fn post_payload_preflight_accepts_incoming_interpreter() {
    let interpreter = "/opt/incoming-runtime/bin/sh";
    let bundle = rpm_bundle_for_phase(
        "incoming-runtime-user",
        "1",
        "rpm:%post",
        LifecyclePath::PostInstall,
        interpreter,
    );
    let event = NativeTransactionEvent {
        owner_package: bundle.source_package.clone(),
        owner_version: bundle.source_version.clone(),
        owner_arch: bundle.source_arch.clone(),
        source_format: "rpm".to_string(),
        stage: NativeEventStage::PackagePostInstall,
        program: NativeEventProgram::BundleEntry {
            entry_id: "rpm:%post".to_string(),
        },
        args: vec!["1".to_string()],
        stdin: Vec::new(),
        matched_targets: Vec::new(),
        rpm_trigger_owner: None,
        deb_package_refcount: None,
        order_key: "incoming-runtime-user".to_string(),
        placement: NativeEventPlacement::TransactionElement {
            transaction_index: 0,
        },
    };
    let prepared = prepared_with_projected_event(
        bundle,
        event,
        NativeEventPathProjection::Projected {
            introduced_paths: BTreeSet::from([interpreter.trim_start_matches('/').to_string()]),
            explicitly_removed_paths: BTreeSet::new(),
            introduced_path_capabilities: BTreeSet::new(),
            explicitly_removed_path_capabilities: BTreeSet::new(),
        },
    );
    let target_root = tempfile::tempdir().unwrap();

    prepared
        .preflight(target_root.path(), &ExecutionMode::Install)
        .expect("post-payload preflight must use the incoming path projection");
}

#[test]
fn post_payload_preflight_accepts_an_exact_projected_path_provider() {
    let interpreter = "/bin/sh";
    let bundle = rpm_bundle_for_phase(
        "path-provider-user",
        "1",
        "rpm:%pre",
        LifecyclePath::PreInstall,
        interpreter,
    );
    let event = NativeTransactionEvent {
        owner_package: bundle.source_package.clone(),
        owner_version: bundle.source_version.clone(),
        owner_arch: bundle.source_arch.clone(),
        source_format: "rpm".to_string(),
        stage: NativeEventStage::PackagePreInstall,
        program: NativeEventProgram::BundleEntry {
            entry_id: "rpm:%pre".to_string(),
        },
        args: vec!["1".to_string()],
        stdin: Vec::new(),
        matched_targets: Vec::new(),
        rpm_trigger_owner: None,
        deb_package_refcount: None,
        order_key: "path-provider-user".to_string(),
        placement: NativeEventPlacement::TransactionElement {
            transaction_index: 1,
        },
    };
    let prepared = prepared_with_projected_event(
        bundle,
        event,
        NativeEventPathProjection::Projected {
            introduced_paths: BTreeSet::from(["usr/bin/sh".to_string()]),
            explicitly_removed_paths: BTreeSet::new(),
            introduced_path_capabilities: BTreeSet::from(["bin/sh".to_string()]),
            explicitly_removed_path_capabilities: BTreeSet::new(),
        },
    );
    let target_root = tempfile::tempdir().unwrap();

    prepared
        .preflight(target_root.path(), &ExecutionMode::Install)
        .expect("an already-applied exact path provider must satisfy interpreter preflight");
}

#[test]
fn prepared_batch_projects_an_earlier_exact_interpreter_provider() {
    let (_temp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let provider = conary_core::packages::traits::ProvidedCapability {
        kind: conary_core::repository::dependency_model::RepositoryCapabilityKind::Generic,
        name: "/bin/sh".to_string(),
        version: None,
        version_relation: None,
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier:
            conary_core::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
        provenance: conary_core::repository::dependency_model::CapabilityProvenance::AuthorDeclared,
    };
    let consumer_bundle = rpm_bundle_for_phase(
        "crypto-policies-fixture",
        "1",
        "rpm:%pre",
        LifecyclePath::PreInstall,
        "/bin/sh",
    );
    let prepared = PreparedNativeTransaction::prepare_batch(
        &conn,
        &[
            NativeInstallInput {
                package_name: "bash-fixture",
                package_version: "1",
                package_arch: Some("x86_64"),
                version_scheme: VersionScheme::Rpm,
                provides: &[provider],
                new_bundle: None,
                old_trove: None,
                relation_removals: &[],
                relation_deconfigurations: &[],
                paths: vec!["/usr/bin/sh".to_string()],
            },
            NativeInstallInput {
                package_name: "crypto-policies-fixture",
                package_version: "1",
                package_arch: Some("x86_64"),
                version_scheme: VersionScheme::Rpm,
                provides: &[],
                new_bundle: Some(&consumer_bundle),
                old_trove: None,
                relation_removals: &[],
                relation_deconfigurations: &[],
                paths: vec!["/etc/crypto-policies/config".to_string()],
            },
        ],
    )
    .unwrap();
    let target_root = tempfile::tempdir().unwrap();

    prepared
        .preflight(target_root.path(), &ExecutionMode::Install)
        .expect("the earlier dependency payload must project its exact /bin/sh provider");
}

#[test]
fn pre_payload_preflight_rejects_incoming_interpreter() {
    let interpreter = "/opt/incoming-runtime/bin/sh";
    let bundle = rpm_bundle_for_phase(
        "incoming-runtime-user",
        "1",
        "rpm:%pre",
        LifecyclePath::PreInstall,
        interpreter,
    );
    let event = NativeTransactionEvent {
        owner_package: bundle.source_package.clone(),
        owner_version: bundle.source_version.clone(),
        owner_arch: bundle.source_arch.clone(),
        source_format: "rpm".to_string(),
        stage: NativeEventStage::PackagePreInstall,
        program: NativeEventProgram::BundleEntry {
            entry_id: "rpm:%pre".to_string(),
        },
        args: vec!["1".to_string()],
        stdin: Vec::new(),
        matched_targets: Vec::new(),
        rpm_trigger_owner: None,
        deb_package_refcount: None,
        order_key: "incoming-runtime-user".to_string(),
        placement: NativeEventPlacement::TransactionElement {
            transaction_index: 0,
        },
    };
    let prepared =
        prepared_with_projected_event(bundle, event, NativeEventPathProjection::CurrentRoot);
    let target_root = tempfile::tempdir().unwrap();

    let error = prepared
        .preflight(target_root.path(), &ExecutionMode::Install)
        .expect_err("pre-payload preflight must validate the current root");
    let error_chain = format!("{error:#}");
    assert!(
        error_chain.contains("Interpreter not found"),
        "unexpected error: {error_chain}"
    );
}
