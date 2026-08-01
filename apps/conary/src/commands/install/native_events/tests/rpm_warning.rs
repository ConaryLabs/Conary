// apps/conary/src/commands/install/native_events/tests/rpm_warning.rs

//! RPM warning-only lifecycle execution coverage.

use super::*;

fn rpm_entry_event(bundle: &NativeLifecycleBundle) -> NativeTransactionEvent {
    let entry = &bundle.entries[0];
    NativeTransactionEvent {
        owner_package: bundle.source_package.clone(),
        owner_version: bundle.source_version.clone(),
        owner_arch: bundle.source_arch.clone(),
        source_format: bundle.source_format.as_str().to_string(),
        stage: NativeEventStage::PackagePostInstall,
        program: NativeEventProgram::BundleEntry {
            entry_id: entry.id.clone(),
        },
        args: vec!["1".to_string()],
        stdin: Vec::new(),
        matched_targets: Vec::new(),
        rpm_trigger_owner: None,
        deb_package_refcount: None,
        order_key: format!("{}:{}", bundle.source_package, entry.id),
        placement: NativeEventPlacement::TransactionElement {
            transaction_index: 0,
        },
    }
}

fn failing_rpm_bundle(criticality: RpmCriticality, critical: bool) -> NativeLifecycleBundle {
    let mut bundle = rpm_bundle_for_phase(
        "rpm-warning-fixture",
        "1",
        "rpm:%post",
        LifecyclePath::PostInstall,
        "<lua>",
    );
    let entry = &mut bundle.entries[0];
    entry.body = "error('typed fixture failure')\n".to_string();
    entry.body_sha256 = conary_core::hash::sha256_prefixed(entry.body.as_bytes());
    let runtime = entry.rpm_runtime.as_mut().unwrap();
    runtime.program = RpmProgram::EmbeddedLua;
    runtime.criticality = criticality;
    runtime.critical = critical;
    bundle
}

fn execute_single_event(bundle: NativeLifecycleBundle) -> anyhow::Result<()> {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let root = temp.path().join("selected-root");
    std::fs::create_dir(&root).unwrap();
    let event = rpm_entry_event(&bundle);
    let prepared =
        prepared_with_projected_event(bundle, event, NativeEventPathProjection::CurrentRoot);

    prepared.execute_graph_event(&conn, 0, &root, &ExecutionMode::Install)
}

#[test]
fn rpm_script_execution_failure_follows_every_persisted_criticality_value() {
    for (criticality, critical, expected_fatal) in [
        (RpmCriticality::WarningOnly, false, false),
        (RpmCriticality::ForcedWarningOnly, false, false),
        (RpmCriticality::Header, true, true),
        (RpmCriticality::SlotDefault, true, true),
    ] {
        let result = execute_single_event(failing_rpm_bundle(criticality, critical));
        assert_eq!(
            result.is_err(),
            expected_fatal,
            "unexpected graph result for {criticality:?}: {result:#?}"
        );
        if let Err(error) = result {
            assert!(
                format!("{error:#}")
                    .contains("native transaction event failed for rpm-warning-fixture 1"),
                "unexpected critical failure: {error:#}"
            );
        }
    }
}

#[test]
fn warning_only_rpm_contract_failure_remains_fatal() {
    let mut bundle = failing_rpm_bundle(RpmCriticality::WarningOnly, false);
    bundle.entries[0].interpreter = "/bin/sh".to_string();

    let error = execute_single_event(bundle)
        .expect_err("warning-only must not suppress a malformed embedded-Lua contract");

    assert!(
        format!("{error:#}").contains("declares embedded Lua with interpreter '/bin/sh'"),
        "unexpected contract failure: {error:#}"
    );
}

#[test]
fn non_rpm_lifecycle_failure_remains_fatal() {
    let mut bundle = rpm_bundle_for_phase(
        "arch-failure-fixture",
        "1",
        "arch:post_install",
        LifecyclePath::PostInstall,
        "/definitely/missing-arch-interpreter",
    );
    bundle.source_format = SourceFormat::Arch;
    bundle.source_family = "arch".to_string();
    bundle.source_profile = Some("arch".to_string());
    bundle.source_release = None;
    bundle.version_scheme = LifecycleVersionScheme::Arch;
    let entry = &mut bundle.entries[0];
    entry.rpm_runtime = None;

    let error =
        execute_single_event(bundle).expect_err("non-RPM lifecycle failures must remain fatal");

    assert!(
        format!("{error:#}")
            .contains("Interpreter not found: /definitely/missing-arch-interpreter"),
        "unexpected non-RPM failure: {error:#}"
    );
}
