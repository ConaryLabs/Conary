// apps/conary/src/commands/install/native_events/tests/rpm_warning.rs

//! RPM per-scriptlet-class lifecycle failure posture coverage.

use super::*;
use conary_core::ccs::native_transaction::{NativeTransactionGraph, NativeTransactionStep};
use conary_core::db::models::{Changeset, ChangesetStatus, LifecycleEvent};
use conary_core::packages::native_abi::RpmScriptletSlot;

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

fn failing_rpm_bundle_for_slot(
    entry_id: &str,
    phase: LifecyclePath,
    header_critical: bool,
) -> NativeLifecycleBundle {
    let mut bundle = rpm_bundle_for_phase("rpm-warning-fixture", "1", entry_id, phase, "<lua>");
    let entry = &mut bundle.entries[0];
    entry.body = "error('typed fixture failure')\n".to_string();
    entry.body_sha256 = conary_core::hash::sha256_prefixed(entry.body.as_bytes());
    // The stamp is derived exactly as conversion derives it: from the typed
    // class and the header flag word. The runtime itself derives posture from
    // the class alone, so the stamp is evidence, not the decision input.
    // Trigger-slot entries cannot derive a class until the caller attaches
    // rpm_trigger metadata; those callers re-stamp afterwards.
    let class = entry.rpm_class();
    let runtime = entry.rpm_runtime.as_mut().unwrap();
    runtime.program = RpmProgram::EmbeddedLua;
    if header_critical {
        runtime.raw_flags |= conary_core::ccs::native_lifecycle::RPM_SCRIPTLET_FLAG_CRITICAL;
    }
    if let Some(class) = class {
        runtime.criticality = class.effective_criticality(header_critical);
    }
    bundle
}

fn pending_changeset(conn: &rusqlite::Connection) -> i64 {
    Changeset::new("Install lifecycle warning fixture".to_string())
        .insert(conn)
        .unwrap()
}

fn execute_single_event(bundle: NativeLifecycleBundle) -> anyhow::Result<()> {
    prepare_single_event(bundle).1
}

/// Run one lifecycle event and hand back both the transaction and its result,
/// so a continued failure can be inspected as the typed evidence it is.
fn prepare_single_event(
    bundle: NativeLifecycleBundle,
) -> (PreparedNativeTransaction, anyhow::Result<()>) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let root = temp.path().join("selected-root");
    std::fs::create_dir(&root).unwrap();
    let event = rpm_entry_event(&bundle);
    let prepared =
        prepared_with_projected_event(bundle, event, NativeEventPathProjection::CurrentRoot);

    let result = prepared.execute_graph_event(&conn, 0, &root, &ExecutionMode::Install);
    (prepared, result)
}

/// The runtime derives posture from the typed class and the header flag word
/// alone, so every persisted criticality value lands on the posture the class
/// declares -- including header promotion and the file-trigger refusal.
#[test]
fn rpm_runtime_derives_posture_from_typed_class_and_header_flag() {
    // (bundle, expected fatal): one class/flag pair per persisted criticality
    // value.
    let mut cases: Vec<(NativeLifecycleBundle, bool)> = vec![
        // WarningOnly: a promotable class without the header flag warns.
        (
            failing_rpm_bundle_for_slot("rpm:%post", LifecyclePath::PostInstall, false),
            false,
        ),
        // Header: the same class with the header CRITICAL bit promotes to
        // fatal.
        (
            failing_rpm_bundle_for_slot("rpm:%post", LifecyclePath::PostInstall, true),
            true,
        ),
        // SlotDefault: a class whose default aborts, without the header flag.
        (
            failing_rpm_bundle_for_slot("rpm:%pre", LifecyclePath::PreInstall, false),
            true,
        ),
    ];
    // ForcedWarningOnly: a file trigger refuses promotion even with the
    // header bit set.
    let mut file_trigger =
        failing_rpm_bundle_for_slot("rpm:%filetriggerin:0", LifecyclePath::Trigger, true);
    let entry = &mut file_trigger.entries[0];
    entry.rpm_trigger = Some(conary_core::ccs::native_lifecycle::RpmTriggerMetadata {
        kind: conary_core::ccs::native_lifecycle::RpmTriggerKind::File,
        target_constraints: vec![
            conary_core::ccs::native_lifecycle::RpmTriggerTargetConstraint {
                package: "/usr/share/cache".to_string(),
                action: conary_core::ccs::native_lifecycle::RpmTriggerAction::Install,
                operator: None,
                version: None,
                raw_flags: 0,
            },
        ],
        priority: None,
        path_prefixes: vec!["/usr/share/cache".to_string()],
    });
    let class = entry.rpm_class().expect("typed trigger class");
    let runtime = entry.rpm_runtime.as_mut().unwrap();
    runtime.criticality = class.effective_criticality(true);
    cases.push((file_trigger, false));

    for (bundle, expected_fatal) in cases {
        let result = execute_single_event(bundle);
        assert_eq!(
            result.is_err(),
            expected_fatal,
            "unexpected graph result: {result:#?}"
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
    let mut bundle = failing_rpm_bundle_for_slot("rpm:%post", LifecyclePath::PostInstall, false);
    bundle.entries[0].interpreter = "/bin/sh".to_string();

    let error = execute_single_event(bundle)
        .expect_err("warning-only must not suppress a malformed embedded-Lua contract");

    assert!(
        format!("{error:#}").contains("declares embedded Lua with interpreter '/bin/sh'"),
        "unexpected contract failure: {error:#}"
    );
}

/// Every RPM scriptlet class the pinned table declares must reach the runtime
/// with that posture. This is the contract between the declared table in
/// `docs/specs/foreign-package-lifecycle-contracts.md` and the runtime that
/// consumes it, not one pinned scriptlet.
#[test]
fn every_declared_rpm_class_gets_its_declared_runtime_posture() {
    use conary_core::scriptlet::FailurePosture::{AbortsTransaction, WarnAndContinue};
    // The expected posture column is the spec table, written out rather than
    // read back from the policy module, so a change to the table changes this
    // test's result instead of moving with it.
    for (entry_id, phase, slot, expected) in [
        (
            "rpm:%pretrans",
            LifecyclePath::PreTransaction,
            RpmScriptletSlot::PreTrans,
            AbortsTransaction,
        ),
        (
            "rpm:%pre",
            LifecyclePath::PreInstall,
            RpmScriptletSlot::Pre,
            AbortsTransaction,
        ),
        (
            "rpm:%post",
            LifecyclePath::PostInstall,
            RpmScriptletSlot::Post,
            WarnAndContinue,
        ),
        (
            "rpm:%posttrans",
            LifecyclePath::PostTransaction,
            RpmScriptletSlot::PostTrans,
            WarnAndContinue,
        ),
        (
            "rpm:%preun",
            LifecyclePath::PreRemove,
            RpmScriptletSlot::PreUn,
            AbortsTransaction,
        ),
        (
            "rpm:%postun",
            LifecyclePath::PostRemove,
            RpmScriptletSlot::PostUn,
            WarnAndContinue,
        ),
    ] {
        // The runtime is told only the typed class; the persisted stamp is
        // derived from that class exactly as conversion derives it.
        let bundle = failing_rpm_bundle_for_slot(entry_id, phase, false);
        assert_eq!(
            bundle.entries[0].native_slot,
            Some(slot),
            "{entry_id} must carry the declared typed slot"
        );
        let (prepared, result) = prepare_single_event(bundle);
        let continued = prepared.take_continued_lifecycle_failures();
        match expected {
            AbortsTransaction => {
                let error = result
                    .err()
                    .unwrap_or_else(|| panic!("{entry_id} must fail the transaction"));
                let error = format!("{error:#}");
                assert!(
                    error.contains("native transaction event failed for rpm-warning-fixture 1"),
                    "{entry_id} aborted without the typed transaction failure: {error}"
                );
                assert!(
                    continued.is_empty(),
                    "{entry_id} aborted, so nothing was continued past"
                );
            }
            WarnAndContinue => {
                result.unwrap_or_else(|error| {
                    panic!("{entry_id} must not fail its transaction: {error:#}")
                });
                assert_eq!(
                    continued.len(),
                    1,
                    "{entry_id} continued but recorded no typed evidence"
                );
                assert_eq!(continued[0].entry, entry_id);
                assert_eq!(
                    continued[0].failure.failure_kind,
                    conary_core::scriptlet::ScriptletFailureKind::ScriptExited
                );
            }
        }
    }
}

/// A `%pre` failure aborts before its package's payload boundary, so nothing
/// the package ships can land. The graph, not the error text, proves it.
#[test]
fn a_pre_install_class_failure_aborts_before_the_payload_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let root = temp.path().join("selected-root");
    std::fs::create_dir(&root).unwrap();

    let bundle = failing_rpm_bundle_for_slot("rpm:%pre", LifecyclePath::PreInstall, false);
    let event = rpm_entry_event(&bundle);
    let mut prepared =
        prepared_with_projected_event(bundle, event, NativeEventPathProjection::CurrentRoot);
    prepared.plan.graph = NativeTransactionGraph {
        steps: vec![
            NativeTransactionStep::RunEvent { event_index: 0 },
            NativeTransactionStep::ApplyPayload { change_index: 0 },
        ],
    };

    let mut boundaries = Vec::new();
    let changeset_id = pending_changeset(&conn);
    let error = crate::commands::install::native_graph::drive_native_graph_with(
        &conn,
        changeset_id,
        &prepared,
        &root,
        &ExecutionMode::Install,
        |_, boundary| {
            boundaries.push(boundary);
            Ok(())
        },
    )
    .expect_err("a %pre class failure must fail the transaction");

    assert!(
        format!("{error:#}").contains("native transaction event failed for rpm-warning-fixture 1"),
        "unexpected %pre failure: {error:#}"
    );
    assert!(
        boundaries.is_empty(),
        "the payload boundary must not be crossed after a %pre failure: {boundaries:?}"
    );
}

/// Evidence the transaction ran past must still be reported on the abort exit.
///
/// Production callers write the event inside the same transaction as the
/// changeset and roll that transaction back on a graph error. A `%post` that
/// failed is most diagnostic precisely when a later step brings the transaction
/// down, so the accumulator has to be drained and reported on the error exit;
/// the persisted row is durable only on a committed changeset.
#[test]
fn a_continued_failure_is_still_reported_when_a_later_graph_step_aborts() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let root = temp.path().join("selected-root");
    std::fs::create_dir(&root).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let changeset_id = pending_changeset(&tx);

    // Entry 0 fails in a warn-and-continue class; entry 1 fails in an aborting
    // class later in the same graph.
    let mut bundle = failing_rpm_bundle_for_slot("rpm:%post", LifecyclePath::PostInstall, false);
    let aborting = failing_rpm_bundle_for_slot("rpm:%pre", LifecyclePath::PreInstall, false)
        .entries
        .remove(0);
    bundle.entries.push(aborting);

    let continuing_event = rpm_entry_event(&bundle);
    let mut aborting_event = continuing_event.clone();
    aborting_event.program = NativeEventProgram::BundleEntry {
        entry_id: "rpm:%pre".to_string(),
    };
    aborting_event.order_key = format!("{}:rpm:%pre", bundle.source_package);

    let mut prepared = prepared_with_projected_event(
        bundle,
        continuing_event,
        NativeEventPathProjection::CurrentRoot,
    );
    prepared.plan.events.push(aborting_event);
    prepared.path_projection = preflight::NativePathProjection::from_events(vec![
        NativeEventPathProjection::CurrentRoot,
        NativeEventPathProjection::CurrentRoot,
    ]);
    prepared.plan.graph = NativeTransactionGraph {
        steps: vec![
            NativeTransactionStep::RunEvent { event_index: 0 },
            NativeTransactionStep::ApplyPayload { change_index: 0 },
            NativeTransactionStep::RunEvent { event_index: 1 },
        ],
    };

    // The payload boundary sits between the two events, so it observes the
    // accumulator mid-graph without draining it.
    let mut pending_mid_graph = None;
    let error = crate::commands::install::native_graph::drive_native_graph_with(
        &tx,
        changeset_id,
        &prepared,
        &root,
        &ExecutionMode::Install,
        |_, _| {
            pending_mid_graph = Some(prepared.continued_lifecycle_failures.borrow().len());
            Ok(())
        },
    )
    .expect_err("the later aborting class must fail the transaction");

    assert_eq!(
        pending_mid_graph,
        Some(1),
        "the warn-and-continue failure must be recorded before the later step runs"
    );
    assert!(
        format!("{error:#}").contains("native transaction event failed for rpm-warning-fixture 1"),
        "the abort error must be unchanged: {error:#}"
    );
    assert!(
        prepared.continued_lifecycle_failures.borrow().is_empty(),
        "the continued failure must be drained for the ui::warn abort-path \
         evidence surface"
    );
    tx.rollback().unwrap();
    let events = LifecycleEvent::list_for_changeset(&conn, changeset_id).unwrap();
    assert_eq!(
        events.len(),
        0,
        "abort rollback must discard lifecycle evidence with the changeset"
    );
}

#[test]
fn a_lifecycle_evidence_persist_error_does_not_change_a_successful_graph_result() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let root = temp.path().join("selected-root");
    std::fs::create_dir(&root).unwrap();
    let changeset_id = pending_changeset(&conn);
    let mut changeset = Changeset::find_by_id(&conn, changeset_id).unwrap().unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();

    let bundle = failing_rpm_bundle_for_slot("rpm:%post", LifecyclePath::PostInstall, false);
    let event = rpm_entry_event(&bundle);
    let mut prepared =
        prepared_with_projected_event(bundle, event, NativeEventPathProjection::CurrentRoot);
    prepared.plan.graph = NativeTransactionGraph {
        steps: vec![NativeTransactionStep::RunEvent { event_index: 0 }],
    };

    crate::commands::install::native_graph::drive_native_graph_with(
        &conn,
        changeset_id,
        &prepared,
        &root,
        &ExecutionMode::Install,
        |_, _| Ok(()),
    )
    .expect("evidence persistence failure must not escalate a warn-and-continue graph");
    assert!(
        prepared.continued_lifecycle_failures.borrow().is_empty(),
        "the persist-error path must still drain the ui::warn evidence"
    );
    assert!(
        LifecycleEvent::list_for_changeset(&conn, changeset_id)
            .unwrap()
            .is_empty(),
        "the rejected Applied changeset must not receive lifecycle evidence"
    );
}

#[test]
fn a_continued_failure_persists_once_on_the_success_exit() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let root = temp.path().join("selected-root");
    std::fs::create_dir(&root).unwrap();
    let changeset_id = pending_changeset(&conn);

    let bundle = failing_rpm_bundle_for_slot("rpm:%post", LifecyclePath::PostInstall, false);
    let event = rpm_entry_event(&bundle);
    let mut prepared =
        prepared_with_projected_event(bundle, event, NativeEventPathProjection::CurrentRoot);
    prepared.plan.graph = NativeTransactionGraph {
        steps: vec![NativeTransactionStep::RunEvent { event_index: 0 }],
    };

    crate::commands::install::native_graph::drive_native_graph_with(
        &conn,
        changeset_id,
        &prepared,
        &root,
        &ExecutionMode::Install,
        |_, _| Ok(()),
    )
    .unwrap();

    let events = LifecycleEvent::list_for_changeset(&conn, changeset_id).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].source_entry, "rpm:%post");
    assert_eq!(events[0].requested_sandbox_mode.as_str(), "always");
    assert_eq!(events[0].effective_sandbox.as_str(), "target-root");
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
