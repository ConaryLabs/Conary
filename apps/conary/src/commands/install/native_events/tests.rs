// apps/conary/src/commands/install/native_events/tests.rs

use super::*;
use conary_core::ccs::native_lifecycle::{
    ArchHookActionMetadata, ArchHookMetadata, ArchHookOperation, ArchHookTriggerMetadata,
    ArchHookTriggerType, ArchHookWhen, DebControlMember, DebMaintainerArgumentMetadata,
    DebMaintainerArgumentValue, DebMaintainerInvocationMetadata, DebMaintainerMetadata,
    DebMaintainerMode, DebTriggerAwaitMode, DebTriggerDirective, DebTriggerMetadata, LifecyclePath,
    NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation,
    NativeLifecycleEntry, NativeLifecycleEntryKind, RpmCriticality, RpmProgram, RpmRuntimeMetadata,
    ScriptletFidelity, SourceFormat, TransactionOrder, VersionScheme as LifecycleVersionScheme,
};
use conary_core::ccs::native_transaction::{
    NativeEventPathProjection, NativeEventPlacement, NativeEventProgram, NativeEventStage,
    NativeTransactionEvent, NativeTransactionGraph,
};
use conary_core::db::models::{FileEntry, InstalledNativeLifecycleBundle, Trove, TroveType};
use conary_core::repository::dependency_model::{
    PackageRelationRemovalMode, RepositoryRequirementKind,
};
use conary_core::scriptlet::ExecutionMode;
use conary_core::transaction::{PackageRelationIncomingIdentity, PackageRelationRemoval};
use std::collections::{BTreeSet, HashMap};
use std::os::unix::fs::PermissionsExt;

#[path = "tests/arch.rs"]
mod arch;
#[path = "tests/deconfiguration.rs"]
mod deconfiguration;

fn pre_remove_bundle(package_name: &str, version: &str) -> NativeLifecycleBundle {
    let body = "exit 0\n".to_string();
    let entry = NativeLifecycleEntry {
        id: "rpm:%preun".to_string(),
        native_slot: "%preun".to_string(),
        kind: NativeLifecycleEntryKind::Executable,
        phase: LifecyclePath::PreRemove,
        lifecycle_paths: vec![LifecyclePath::PreRemove.as_str().to_string()],
        interpreter: "/bin/sh".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body,
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "pre-remove".to_string(),
            ..TransactionOrder::default()
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        rpm_trigger: None,
        rpm_runtime: Some(RpmRuntimeMetadata {
            program: RpmProgram::External,
            body_transforms: Vec::new(),
            critical: true,
            criticality: RpmCriticality::SlotDefault,
            raw_flags: 0,
            unknown_flags: 0,
            install_prefixes: Vec::new(),
            macro_context: Default::default(),
            header_context: Default::default(),
            package_rpm_version: None,
        }),
        rpm_sysusers: None,
        deb_maintainer: None,
        arch_install: None,
        arch_hook: None,
        residual_lifecycle: None,
    };
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora".to_string(),
        source_distro: Some("fedora-44".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: package_name.to_string(),
        source_version: version.to_string(),
        source_checksum: None,
        version_scheme: LifecycleVersionScheme::Rpm,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "1".to_string(),
        conversion_policy: "typed-relation-test".to_string(),
        evidence_digest: None,
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![entry],
    }
}

fn rpm_bundle_for_phase(
    package_name: &str,
    version: &str,
    entry_id: &str,
    phase: LifecyclePath,
    interpreter: &str,
) -> NativeLifecycleBundle {
    let mut bundle = pre_remove_bundle(package_name, version);
    let entry = &mut bundle.entries[0];
    entry.id = entry_id.to_string();
    entry.native_slot = entry_id.trim_start_matches("rpm:").to_string();
    entry.phase = phase.clone();
    entry.lifecycle_paths = vec![phase.as_str().to_string()];
    entry.interpreter = interpreter.to_string();
    entry.transaction_order.position = phase.as_str().to_string();
    bundle
}

fn prepared_with_projected_event(
    bundle: NativeLifecycleBundle,
    event: NativeTransactionEvent,
    projection: NativeEventPathProjection,
) -> PreparedNativeTransaction {
    let path_projection = preflight::NativePathProjection::from_events(vec![projection]);
    PreparedNativeTransaction {
        owners: vec![NativeBundleOwner {
            package_name: bundle.source_package.clone(),
            package_version: bundle.source_version.clone(),
            instances_after: 1,
            role: NativeBundleRole::Installing,
            initial_package_state: DebPackageState::Installed,
            initial_pending_triggers: Vec::new(),
            initial_awaited_packages: Vec::new(),
            bundle,
        }],
        plan: NativeTransactionPlan {
            events: vec![event],
            graph: NativeTransactionGraph::default(),
            deb: Default::default(),
        },
        path_projection,
        ..PreparedNativeTransaction::default()
    }
}

fn empty_extraction() -> ExtractionResult {
    ExtractionResult {
        extracted_files: Vec::new(),
        classified: HashMap::new(),
        component_names_by_path: None,
        installed_component_names: None,
        ccs_remove_hook: None,
        installed_component_types: Vec::new(),
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    }
}

fn deb_entry(
    id: &str,
    phase: LifecyclePath,
    control_member: DebControlMember,
    mode: DebMaintainerMode,
    interpreter: &str,
) -> NativeLifecycleEntry {
    let mut entry = pre_remove_bundle("fixture", "1").entries.remove(0);
    entry.id = id.to_string();
    entry.native_slot = match control_member {
        DebControlMember::Config => "config",
        DebControlMember::Preinst => "preinst",
        DebControlMember::Postinst => "postinst",
        DebControlMember::Prerm => "prerm",
        DebControlMember::Postrm => "postrm",
        DebControlMember::Triggers => "triggers",
    }
    .to_string();
    entry.phase = phase.clone();
    entry.lifecycle_paths = vec![phase.as_str().to_string()];
    entry.interpreter = interpreter.to_string();
    entry.rpm_runtime = None;
    entry.deb_maintainer = Some(DebMaintainerMetadata {
        control_member,
        invocations: vec![DebMaintainerInvocationMetadata {
            mode,
            arguments: vec![DebMaintainerArgumentMetadata {
                index: 1,
                name: "action".to_string(),
                value: DebMaintainerArgumentValue::Action,
                literal: None,
                required: true,
            }],
            lifecycle_paths: vec![phase],
        }],
        ..DebMaintainerMetadata::default()
    });
    entry
}

fn deb_triggers_entry(declarations: Vec<DebTriggerMetadata>) -> NativeLifecycleEntry {
    let mut entry = pre_remove_bundle("fixture", "1").entries.remove(0);
    entry.id = "deb:triggers".to_string();
    entry.native_slot = "triggers".to_string();
    entry.kind = NativeLifecycleEntryKind::ControlArtifact;
    entry.phase = LifecyclePath::Trigger;
    entry.lifecycle_paths = vec![LifecyclePath::Trigger.as_str().to_string()];
    entry.interpreter = "package-manager-control-artifact".to_string();
    entry.rpm_runtime = None;
    entry.body = declarations
        .iter()
        .map(|declaration| declaration.raw_line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    entry.body.push('\n');
    entry.body_sha256 = conary_core::hash::sha256_prefixed(entry.body.as_bytes());
    entry.deb_maintainer = Some(DebMaintainerMetadata {
        control_member: DebControlMember::Triggers,
        trigger_declarations: declarations,
        ..DebMaintainerMetadata::default()
    });
    entry
}

fn deb_remove_with_recovery_bundle(package_name: &str, version: &str) -> NativeLifecycleBundle {
    let mut bundle = pre_remove_bundle(package_name, version);
    bundle.source_format = SourceFormat::Deb;
    bundle.source_family = "debian".to_string();
    bundle.version_scheme = LifecycleVersionScheme::Deb;
    bundle.entries = vec![
        deb_entry(
            "deb:prerm",
            LifecyclePath::PreRemove,
            DebControlMember::Prerm,
            DebMaintainerMode::Remove,
            "/bin/sh",
        ),
        deb_entry(
            "deb:postinst",
            LifecyclePath::PostInstall,
            DebControlMember::Postinst,
            DebMaintainerMode::AbortRemove,
            "/definitely/missing-recovery-interpreter",
        ),
    ];
    bundle
}

fn deb_postinst_environment_bundle(source_arch: Option<&str>) -> NativeLifecycleBundle {
    let mut bundle = pre_remove_bundle("deb-environment", "1.0-1");
    bundle.source_format = SourceFormat::Deb;
    bundle.source_family = "debian".to_string();
    bundle.source_arch = source_arch.map(str::to_string);
    bundle.version_scheme = LifecycleVersionScheme::Deb;
    let mut entry = deb_entry(
        "deb:postinst",
        LifecyclePath::PostInstall,
        DebControlMember::Postinst,
        DebMaintainerMode::Configure,
        "/bin/sh",
    );
    entry.body = r#"test "$#" -eq 1
test "$1" = configure
test -z "$DPKG_ROOT"
test "$DPKG_MAINTSCRIPT_PACKAGE" = deb-environment
test "$DPKG_MAINTSCRIPT_PACKAGE_REFCOUNT" = 1
test "$DPKG_MAINTSCRIPT_ARCH" = amd64
test "$DPKG_MAINTSCRIPT_NAME" = postinst
test "$DPKG_MAINTSCRIPT_DEBUG" = 0
test "$DPKG_RUNNING_VERSION" = 1.23.7
test "$DPKG_ADMINDIR" = /var/lib/conary/dpkg-compat
"#
    .to_string();
    entry.body_sha256 = conary_core::hash::sha256_prefixed(entry.body.as_bytes());
    bundle.entries = vec![entry];
    bundle
}

fn deb_trigger_runtime_bundle(
    package_name: &str,
    interest_trigger: Option<&str>,
) -> NativeLifecycleBundle {
    let mut bundle = pre_remove_bundle(package_name, "1");
    bundle.source_format = SourceFormat::Deb;
    bundle.source_family = "debian".to_string();
    bundle.source_distro = Some("debian".to_string());
    bundle.source_release = Some("13".to_string());
    bundle.source_arch = Some("amd64".to_string());
    bundle.version_scheme = LifecycleVersionScheme::Deb;
    let mut postinst = deb_entry(
        "deb:postinst",
        LifecyclePath::Trigger,
        DebControlMember::Postinst,
        DebMaintainerMode::Triggered,
        "/bin/sh",
    );
    let metadata = postinst.deb_maintainer.as_mut().unwrap();
    metadata.invocations[0]
        .arguments
        .push(DebMaintainerArgumentMetadata {
            index: 2,
            name: "trigger-names".to_string(),
            value: DebMaintainerArgumentValue::TriggerNames,
            literal: None,
            required: true,
        });
    let mut entries = vec![postinst];
    if let Some(trigger_name) = interest_trigger {
        entries.push(deb_triggers_entry(vec![DebTriggerMetadata {
            directive: DebTriggerDirective::Interest,
            trigger_name: trigger_name.to_string(),
            await_mode: DebTriggerAwaitMode::Await,
            raw_line: format!("interest-await {trigger_name}"),
        }]));
    }
    bundle.entries = entries;
    bundle
}

fn deb_postinst_event() -> NativeTransactionEvent {
    NativeTransactionEvent {
        owner_package: "deb-environment".to_string(),
        owner_version: "1.0-1".to_string(),
        owner_arch: Some("amd64".to_string()),
        source_format: "deb".to_string(),
        stage: NativeEventStage::DebPostInstall,
        program: NativeEventProgram::BundleEntry {
            entry_id: "deb:postinst".to_string(),
        },
        args: vec!["configure".to_string()],
        stdin: Vec::new(),
        matched_targets: Vec::new(),
        rpm_trigger_owner: None,
        deb_package_refcount: Some(1),
        order_key: "deb-environment".to_string(),
        placement: NativeEventPlacement::TransactionElement {
            transaction_index: 0,
        },
    }
}

#[test]
fn relation_removal_is_a_first_class_native_remove_event() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut old = Trove::new(
        "oldpkg".to_string(),
        "1".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    let old_id = old.insert(&conn).unwrap();
    FileEntry::new(
        "/usr/bin/oldpkg".to_string(),
        conary_core::payload::ResolvedPayloadNode::from_numeric_source(
            conary_core::payload::PayloadNode::regular(0o755),
        )
        .unwrap(),
        Some(conary_core::payload::PayloadContentAuthority {
            sha256: conary_core::hash::sha256(b"oldpkg"),
            size: b"oldpkg".len() as u64,
        }),
        old_id,
    )
    .insert(&conn)
    .unwrap();
    let bundle = pre_remove_bundle("oldpkg", "1");
    InstalledNativeLifecycleBundle::new(old_id, None, &bundle)
        .unwrap()
        .insert_or_replace(&conn)
        .unwrap();
    let removals = vec![PackageRelationRemoval {
        trove_id: old_id,
        package_name: "oldpkg".to_string(),
        package_version: "1".to_string(),
        package_architecture: None,
        triggering_incoming: PackageRelationIncomingIdentity {
            transaction_index: 0,
            package_name: "newpkg".to_string(),
            package_version: "2".to_string(),
            package_architecture: None,
        },
        incoming_packages: vec!["newpkg".to_string()],
        ownership_transfer_packages: vec!["newpkg".to_string()],
        kind: RepositoryRequirementKind::Obsolete,
        mode: PackageRelationRemovalMode::OwnershipTransfer,
        native_text: Some("oldpkg < 2".to_string()),
    }];

    let prepared = PreparedNativeTransaction::prepare_install(
        &conn,
        "newpkg",
        "2",
        None,
        conary_core::repository::versioning::VersionScheme::Rpm,
        &[],
        None,
        None,
        &removals,
        &[],
        &empty_extraction(),
    )
    .unwrap();

    assert!(prepared.requires_upgrade_payload_boundary);
    let event = prepared
        .plan
        .events_at(NativeEventStage::PackagePreRemove)
        .find(|event| event.owner_package == "oldpkg")
        .expect("relation removal must schedule the installed owner's pre-remove lifecycle");
    assert_eq!(event.owner_version, "1");
}

#[test]
fn debian_disappearance_requires_one_exact_overwriter_for_every_non_conffile_path() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut old = Trove::new(
        "oldpkg".to_string(),
        "1".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Debian,
    );
    let old_id = old.insert(&conn).unwrap();
    let mut conffile = ConfigFile::new(
        "/etc/oldpkg.conf".to_string(),
        old_id,
        conary_core::hash::sha256(b"old"),
    );
    conffile.source = conary_core::db::models::ConfigSource::Deb;
    conffile.insert(&conn).unwrap();
    let relation = PackageRelationRemoval {
        trove_id: old_id,
        package_name: "oldpkg".to_string(),
        package_version: "1".to_string(),
        package_architecture: None,
        triggering_incoming: PackageRelationIncomingIdentity {
            transaction_index: 0,
            package_name: "new-a".to_string(),
            package_version: "2".to_string(),
            package_architecture: None,
        },
        incoming_packages: vec!["new-a".to_string(), "new-b".to_string()],
        ownership_transfer_packages: vec!["new-a".to_string(), "new-b".to_string()],
        kind: RepositoryRequirementKind::Obsolete,
        mode: PackageRelationRemovalMode::OwnershipTransfer,
        native_text: Some("oldpkg < 2".to_string()),
    };
    let old_paths = BTreeSet::from([
        "/etc/oldpkg.conf".to_string(),
        "/usr/bin/tool".to_string(),
        "/usr/share/tool.data".to_string(),
    ]);
    let new_a_paths = vec![
        "/usr/bin/tool".to_string(),
        "/usr/share/tool.data".to_string(),
    ];
    let new_b_paths = vec!["/usr/share/other".to_string()];
    let inputs = [
        NativeInstallInput {
            package_name: "new-a",
            package_version: "2",
            package_arch: None,
            version_scheme: conary_core::repository::versioning::VersionScheme::Debian,
            provides: &[],
            new_bundle: None,
            old_trove: None,
            relation_removals: &[],
            relation_deconfigurations: &[],
            paths: new_a_paths,
        },
        NativeInstallInput {
            package_name: "new-b",
            package_version: "2",
            package_arch: None,
            version_scheme: conary_core::repository::versioning::VersionScheme::Debian,
            provides: &[],
            new_bundle: None,
            old_trove: None,
            relation_removals: &[],
            relation_deconfigurations: &[],
            paths: new_b_paths,
        },
    ];

    assert_eq!(
        debian_relation_removal_operation(&conn, &relation, old_id, &inputs, &old_paths).unwrap(),
        NativeTransactionOperation::Disappear {
            overwriter_transaction_index: 0,
        }
    );

    let split_inputs = [
        NativeInstallInput {
            package_name: "new-a",
            package_version: "2",
            package_arch: None,
            version_scheme: conary_core::repository::versioning::VersionScheme::Debian,
            provides: &[],
            new_bundle: None,
            old_trove: None,
            relation_removals: &[],
            relation_deconfigurations: &[],
            paths: vec!["/usr/bin/tool".to_string()],
        },
        NativeInstallInput {
            package_name: "new-b",
            package_version: "2",
            package_arch: None,
            version_scheme: conary_core::repository::versioning::VersionScheme::Debian,
            provides: &[],
            new_bundle: None,
            old_trove: None,
            relation_removals: &[],
            relation_deconfigurations: &[],
            paths: vec!["/usr/share/tool.data".to_string()],
        },
    ];
    assert_eq!(
        debian_relation_removal_operation(&conn, &relation, old_id, &split_inputs, &old_paths)
            .unwrap(),
        NativeTransactionOperation::RemoveInFavour {
            replacement_transaction_index: 0,
        }
    );
}

#[test]
fn transaction_preflight_walks_debian_recovery_branches() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    let selected_root = temp.path().join("selected-root");
    std::fs::create_dir_all(selected_root.join("bin")).unwrap();
    std::fs::write(selected_root.join("bin/sh"), b"test interpreter").unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = Trove::new(
        "deb-fixture".to_string(),
        "1".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Debian,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let bundle = deb_remove_with_recovery_bundle("deb-fixture", "1");
    InstalledNativeLifecycleBundle::new(trove_id, None, &bundle)
        .unwrap()
        .insert_or_replace(&conn)
        .unwrap();

    let prepared = PreparedNativeTransaction::prepare_remove(
        &conn,
        trove_id,
        "deb-fixture",
        "1",
        Vec::new(),
        false,
    )
    .unwrap();
    let error = prepared
        .preflight(&selected_root, &ExecutionMode::Remove)
        .expect_err("missing recovery interpreter must fail before removal mutation");

    assert!(
        error
            .to_string()
            .contains("Debian recovery preflight failed"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn transaction_preflight_walks_exact_command_events() {
    let owner_bundle = pre_remove_bundle("command-owner", "1");
    let prepared = PreparedNativeTransaction {
        owners: vec![NativeBundleOwner {
            package_name: "command-owner".to_string(),
            package_version: "1".to_string(),
            instances_after: 1,
            role: NativeBundleRole::Installing,
            initial_package_state: DebPackageState::NotInstalled,
            initial_pending_triggers: Vec::new(),
            initial_awaited_packages: Vec::new(),
            bundle: owner_bundle,
        }],
        plan: NativeTransactionPlan {
            events: vec![NativeTransactionEvent {
                owner_package: "command-owner".to_string(),
                owner_version: "1".to_string(),
                owner_arch: Some("x86_64".to_string()),
                source_format: "arch".to_string(),
                stage: NativeEventStage::ArchPreTransaction,
                program: NativeEventProgram::Command {
                    argv: vec!["/definitely/missing-native-command".to_string()],
                },
                args: Vec::new(),
                stdin: Vec::new(),
                matched_targets: Vec::new(),
                rpm_trigger_owner: None,
                deb_package_refcount: None,
                order_key: "fixture".to_string(),
                placement: NativeEventPlacement::TransactionBefore,
            }],
            graph: NativeTransactionGraph::default(),
            deb: Default::default(),
        },
        ..PreparedNativeTransaction::default()
    };

    let error = prepared
        .preflight(Path::new("/"), &ExecutionMode::Install)
        .expect_err("missing exact command must fail before transaction mutation");
    assert!(
        error.to_string().contains("native transaction preflight"),
        "unexpected error: {error:#}"
    );
}

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
        },
    );
    let target_root = tempfile::tempdir().unwrap();

    prepared
        .preflight(target_root.path(), &ExecutionMode::Install)
        .expect("post-payload preflight must use the incoming path projection");
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

#[test]
fn post_payload_preflight_rejects_removed_interpreter() {
    let interpreter = "/opt/removed-runtime/bin/sh";
    let bundle = rpm_bundle_for_phase(
        "removed-runtime-user",
        "1",
        "rpm:%postun",
        LifecyclePath::PostRemove,
        interpreter,
    );
    let event = NativeTransactionEvent {
        owner_package: bundle.source_package.clone(),
        owner_version: bundle.source_version.clone(),
        owner_arch: bundle.source_arch.clone(),
        source_format: "rpm".to_string(),
        stage: NativeEventStage::PackagePostRemove,
        program: NativeEventProgram::BundleEntry {
            entry_id: "rpm:%postun".to_string(),
        },
        args: vec!["0".to_string()],
        stdin: Vec::new(),
        matched_targets: Vec::new(),
        rpm_trigger_owner: None,
        deb_package_refcount: None,
        order_key: "removed-runtime-user".to_string(),
        placement: NativeEventPlacement::TransactionElement {
            transaction_index: 0,
        },
    };
    let prepared = prepared_with_projected_event(
        bundle,
        event,
        NativeEventPathProjection::Projected {
            introduced_paths: BTreeSet::new(),
            explicitly_removed_paths: BTreeSet::from([interpreter
                .trim_start_matches('/')
                .to_string()]),
        },
    );
    let target_root = tempfile::tempdir().unwrap();
    let current_interpreter = target_root.path().join(interpreter.trim_start_matches('/'));
    std::fs::create_dir_all(current_interpreter.parent().unwrap()).unwrap();
    std::fs::write(&current_interpreter, "#!/bin/sh\n").unwrap();

    let error = prepared
        .preflight(target_root.path(), &ExecutionMode::Remove)
        .expect_err("post-payload preflight must reject an explicitly removed interpreter");
    let error_chain = format!("{error:#}");
    assert!(
        error_chain.contains("Interpreter not found"),
        "unexpected error: {error_chain}"
    );
}

#[test]
fn debian_entry_runtime_receives_exact_dpkg_environment() {
    let event = deb_postinst_event();
    let prepared = PreparedNativeTransaction {
        owners: vec![NativeBundleOwner {
            package_name: "deb-environment".to_string(),
            package_version: "1.0-1".to_string(),
            instances_after: 1,
            role: NativeBundleRole::Installing,
            initial_package_state: DebPackageState::NotInstalled,
            initial_pending_triggers: Vec::new(),
            initial_awaited_packages: Vec::new(),
            bundle: deb_postinst_environment_bundle(Some("amd64")),
        }],
        ..PreparedNativeTransaction::default()
    };

    let owner = &prepared.owners[0];
    let entry = &owner.bundle.entries[0];
    let context = debian_runtime::for_entry(owner, entry, event.deb_package_refcount)
        .unwrap()
        .expect("Debian entry must have a dpkg runtime context");

    assert_eq!(event.args, ["configure"]);
    assert_eq!(context.package_refcount, 1);
    assert_eq!(
        context.environment,
        [
            "DPKG_ROOT=",
            "DPKG_ADMINDIR=/var/lib/conary/dpkg-compat",
            "DPKG_FORCE=",
            "DPKG_MAINTSCRIPT_PACKAGE=deb-environment",
            "DPKG_MAINTSCRIPT_PACKAGE_REFCOUNT=1",
            "DPKG_MAINTSCRIPT_ARCH=amd64",
            "DPKG_MAINTSCRIPT_NAME=postinst",
            "DPKG_MAINTSCRIPT_DEBUG=0",
            "DPKG_RUNNING_VERSION=1.23.7",
        ]
    );
}

#[test]
fn transaction_preflight_rejects_missing_debian_source_architecture() {
    let mut event = deb_postinst_event();
    event.owner_arch = None;
    let prepared = PreparedNativeTransaction {
        owners: vec![NativeBundleOwner {
            package_name: "deb-environment".to_string(),
            package_version: "1.0-1".to_string(),
            instances_after: 1,
            role: NativeBundleRole::Installing,
            initial_package_state: DebPackageState::NotInstalled,
            initial_pending_triggers: Vec::new(),
            initial_awaited_packages: Vec::new(),
            bundle: deb_postinst_environment_bundle(None),
        }],
        plan: NativeTransactionPlan {
            events: vec![event],
            graph: NativeTransactionGraph::default(),
            deb: Default::default(),
        },
        path_projection: preflight::NativePathProjection::from_events(vec![
            NativeEventPathProjection::CurrentRoot,
        ]),
        ..PreparedNativeTransaction::default()
    };

    let error = prepared
        .preflight(Path::new("/"), &ExecutionMode::Install)
        .expect_err("missing source architecture must fail before mutation");
    let error_chain = format!("{error:#}");
    assert!(
        error_chain.contains("no source architecture"),
        "unexpected error: {error_chain}"
    );
}

#[test]
fn persisted_debian_trigger_batch_success_clears_reverse_await_state() {
    let (_temp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();

    let mut interested_trove = Trove::new(
        "cache-owner".to_string(),
        "1".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Debian,
    );
    interested_trove.architecture = Some("amd64".to_string());
    let interested_trove_id = interested_trove.insert(&conn).unwrap();
    let mut interested = InstalledNativeLifecycleBundle::new(
        interested_trove_id,
        None,
        &deb_trigger_runtime_bundle("cache-owner", Some("refresh-cache")),
    )
    .unwrap();
    interested.set_lifecycle_state(DebPackageState::TriggersPending);
    interested.pending_triggers = vec!["refresh-cache".to_string()];
    interested.insert_or_replace(&conn).unwrap();

    let mut awaiter_trove = Trove::new(
        "activator".to_string(),
        "1".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Debian,
    );
    awaiter_trove.architecture = Some("amd64".to_string());
    let awaiter_trove_id = awaiter_trove.insert(&conn).unwrap();
    let mut awaiter = InstalledNativeLifecycleBundle::new(
        awaiter_trove_id,
        None,
        &deb_trigger_runtime_bundle("activator", None),
    )
    .unwrap();
    awaiter.set_lifecycle_state(DebPackageState::TriggersAwaited);
    awaiter.awaited_packages = vec![NativePackageIdentity {
        package_name: "cache-owner".to_string(),
        package_version: "1".to_string(),
        package_arch: Some("amd64".to_string()),
    }];
    awaiter.insert_or_replace(&conn).unwrap();

    let prepared = PreparedNativeTransaction::prepare_batch(&conn, &[]).unwrap();
    let event_index = prepared
        .plan
        .events
        .iter()
        .position(|event| event.stage == NativeEventStage::DebAwaitedTriggerProcessing)
        .expect("persisted pending trigger must resume");
    let event = &prepared.plan.events[event_index];
    assert_eq!(event.owner_package, "cache-owner");
    assert_eq!(event.args, vec!["triggered", "refresh-cache"]);

    prepared.persist_trigger_pending(&conn, event).unwrap();
    prepared.persist_event_success(&conn, event).unwrap();

    let interested = InstalledNativeLifecycleBundle::find_by_trove(&conn, interested_trove_id)
        .unwrap()
        .unwrap();
    assert_eq!(interested.lifecycle_state, DebPackageState::Installed);
    assert!(interested.pending_triggers.is_empty());
    let awaiter = InstalledNativeLifecycleBundle::find_by_trove(&conn, awaiter_trove_id)
        .unwrap()
        .unwrap();
    assert_eq!(awaiter.lifecycle_state, DebPackageState::Installed);
    assert!(awaiter.awaited_packages.is_empty());
}
