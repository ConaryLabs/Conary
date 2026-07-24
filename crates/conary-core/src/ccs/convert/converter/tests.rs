// conary-core/src/ccs/convert/converter/tests.rs

use super::*;
use crate::ccs::legacy_scriptlets::EffectReplacement;
use crate::packages::native_abi::*;
use crate::packages::traits::{Dependency, PackageFile, PackageFormat, Scriptlet, ScriptletPhase};

#[test]
fn scriptlet_bundle_types_are_publicly_exported() {
    let summary = crate::ccs::convert::ScriptletBundleSummary::default();
    assert_eq!(summary.publication_status, "public");
    let nested_summary = crate::ccs::convert::scriptlet_bundle::ScriptletBundleSummary::default();
    assert_eq!(nested_summary.publication_status, "public");

    let counts = crate::ccs::convert::ScriptletDecisionCountsSummary::default();
    let nested_counts =
        crate::ccs::convert::scriptlet_bundle::ScriptletDecisionCountsSummary::default();
    assert_eq!(counts, nested_counts);

    assert!(
        std::any::type_name::<crate::ccs::convert::ScriptletBundleInput<'static>>()
            .contains("ScriptletBundleInput")
    );
    assert!(
            std::any::type_name::<
                crate::ccs::convert::scriptlet_bundle::ScriptletBundleInput<'static>,
            >()
            .contains("ScriptletBundleInput")
        );
    assert!(
        std::any::type_name::<crate::ccs::convert::ScriptletBundleBuild>()
            .contains("ScriptletBundleBuild")
    );
    assert!(
        std::any::type_name::<crate::ccs::convert::scriptlet_bundle::ScriptletBundleBuild>()
            .contains("ScriptletBundleBuild")
    );

    let _root_builder: for<'a> fn(
        crate::ccs::convert::ScriptletBundleInput<'a>,
    )
        -> anyhow::Result<crate::ccs::convert::ScriptletBundleBuild> =
        crate::ccs::convert::build_legacy_scriptlet_bundle;
    let _module_builder: for<'a> fn(
        crate::ccs::convert::scriptlet_bundle::ScriptletBundleInput<'a>,
    ) -> anyhow::Result<
        crate::ccs::convert::scriptlet_bundle::ScriptletBundleBuild,
    > = crate::ccs::convert::scriptlet_bundle::build_legacy_scriptlet_bundle;
}

fn make_test_metadata() -> PackageMetadata {
    PackageMetadata {
        package_path: PathBuf::from("/tmp/test-1.0.0.rpm"),
        name: "test-package".to_string(),
        version: "1.0.0".to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some("Test package".to_string()),
        files: vec![PackageFile {
            path: "/usr/bin/test".to_string(),
            size: 100,
            mode: 0o755,
            sha256: Some("abc123".to_string()),
            symlink_target: None,
        }],
        dependencies: vec![Dependency {
            name: "libc".to_string(),
            version: Some(">= 2.17".to_string()),
            dep_type: DependencyType::Runtime,
            description: None,
        }],
        provides: vec![],
        scriptlets: vec![Scriptlet {
            phase: ScriptletPhase::PreInstall,
            interpreter: "/bin/sh".to_string(),
            content: "getent passwd testuser || useradd -r testuser".to_string(),
            flags: None,
        }],
        native_scriptlet_abi: Vec::new(),
        config_files: vec![],
    }
}

fn make_test_files() -> Vec<ExtractedFile> {
    vec![ExtractedFile {
        path: "/usr/bin/test".to_string(),
        content: b"#!/bin/sh\necho test".to_vec(),
        size: 20,
        mode: 0o755,
        sha256: Some("abc123".to_string()),
        symlink_target: None,
    }]
}

fn make_test_files_with_demo_unit() -> Vec<ExtractedFile> {
    let mut files = make_test_files();
    files.push(ExtractedFile {
        path: "/usr/lib/systemd/system/demo.service".to_string(),
        content: b"[Service]\nExecStart=/usr/bin/demo\n".to_vec(),
        size: 32,
        mode: 0o644,
        sha256: None,
        symlink_target: None,
    });
    files
}

fn effect_extra_str<'a>(
    effect: &'a crate::ccs::legacy_scriptlets::ScriptletEffect,
    key: &str,
) -> Option<&'a str> {
    effect.extra.get(key).and_then(toml::Value::as_str)
}

fn effect_extra_bool(
    effect: &crate::ccs::legacy_scriptlets::ScriptletEffect,
    key: &str,
) -> Option<bool> {
    effect.extra.get(key).and_then(toml::Value::as_bool)
}

fn classification_extra_str<'a>(
    effect: &'a crate::ccs::convert::effects::ScriptletEffectEvidence,
    key: &str,
) -> Option<&'a str> {
    effect.extra.get(key).and_then(toml::Value::as_str)
}

fn classification_extra_bool(
    effect: &crate::ccs::convert::effects::ScriptletEffectEvidence,
    key: &str,
) -> Option<bool> {
    effect.extra.get(key).and_then(toml::Value::as_bool)
}

fn rpm_native_entry(
    id: &str,
    slot_name: &str,
    body: &str,
    slot: RpmScriptletSlot,
    lifecycle: NativeLifecyclePath,
    position: NativeTransactionPosition,
    support: NativeScriptletSupport,
) -> NativeScriptletEntry {
    NativeScriptletEntry {
        id: id.to_string(),
        format: NativeScriptletFormat::Rpm,
        kind: NativeScriptletKind::Executable,
        native_slot: slot_name.to_string(),
        primary_lifecycle: lifecycle,
        compatibility_phase: None,
        lifecycle_paths: vec![lifecycle],
        interpreter: Some("/bin/sh".to_string()),
        interpreter_args: vec![],
        body: NativeScriptletBody::from_bytes(body.as_bytes().to_vec()),
        invocation: NativeInvocationContract::none(),
        order: NativeTransactionOrder::new(position),
        support,
        metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
            slot,
            scriptlet_flags: None,
            trigger: None,
        }),
    }
}

fn arch_alpm_entry(id: &str, support: NativeScriptletSupport) -> NativeScriptletEntry {
    NativeScriptletEntry {
            id: id.to_string(),
            format: NativeScriptletFormat::Arch,
            kind: NativeScriptletKind::ControlArtifact,
            native_slot: "alpm-hook".to_string(),
            primary_lifecycle: NativeLifecyclePath::Trigger,
            compatibility_phase: None,
            lifecycle_paths: vec![NativeLifecyclePath::Trigger],
            interpreter: None,
            interpreter_args: vec![],
            body: NativeScriptletBody::from_bytes(
                b"[Trigger]\nType = Package\nTarget = demo\n[Action]\nWhen = PostTransaction\nExec = /bin/true\n"
                    .to_vec(),
            ),
            invocation: NativeInvocationContract::none(),
            order: NativeTransactionOrder::new(NativeTransactionPosition::ControlArtifact),
            support,
            metadata: NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(
                ArchAlpmHookMetadata {
                    hook_path: "/usr/share/libalpm/hooks/demo.hook".to_string(),
                    triggers: vec![],
                    action: None,
                },
            )),
        }
}

fn arch_install_function_entry(
    function_name: &str,
    install_source: &str,
    function_body: &str,
) -> NativeScriptletEntry {
    NativeScriptletEntry {
        id: format!("arch:{function_name}"),
        format: NativeScriptletFormat::Arch,
        kind: NativeScriptletKind::Executable,
        native_slot: function_name.to_string(),
        primary_lifecycle: NativeLifecyclePath::PostInstall,
        compatibility_phase: Some(ScriptletPhase::PostInstall),
        lifecycle_paths: vec![NativeLifecyclePath::PostInstall],
        interpreter: Some("/bin/sh".to_string()),
        interpreter_args: vec![],
        body: NativeScriptletBody::from_bytes(install_source.as_bytes().to_vec()),
        invocation: NativeInvocationContract::none(),
        order: NativeTransactionOrder::new(NativeTransactionPosition::AfterPayload),
        support: NativeScriptletSupport::Parsed,
        metadata: NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(
            ArchInstallScriptletMetadata {
                install_source_sha256: crate::hash::sha256_prefixed(install_source.as_bytes()),
                function_name: function_name.to_string(),
                function_body: Some(function_body.to_string()),
                function_body_sha256: Some(crate::hash::sha256_prefixed(function_body.as_bytes())),
                extraction_status: ArchFunctionExtractionStatus::Parsed,
            },
        )),
    }
}

fn passive_test_converter(output_dir: &Path) -> LegacyConverter {
    LegacyConverter::new(ConversionOptions {
        output_dir: output_dir.to_path_buf(),
        ..ConversionOptions::default()
    })
}

#[test]
fn conversion_result_carries_scriptlet_classification_report() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "/sbin/ldconfig\n".to_string(),
        flags: None,
    }];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert!(result.scriptlet_classification.known_count >= 1);
    assert!(
        result
            .scriptlet_classification
            .entries
            .iter()
            .any(|entry| entry.entry_id.contains("scriptlet"))
    );
    result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle")
        .validate()
        .unwrap();
}

#[test]
fn conversion_result_embeds_legacy_scriptlet_bundle() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "/sbin/ldconfig\n".to_string(),
        flags: None,
    }];

    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .unwrap();

    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .unwrap();
    assert_eq!(bundle.source_package, metadata.name);
    assert_eq!(
        result
            .legacy_scriptlets
            .as_ref()
            .unwrap()
            .evidence_digest
            .as_deref(),
        bundle.evidence_digest.as_deref()
    );
    assert_eq!(
        result.scriptlet_metadata.evidence_digest.as_deref(),
        bundle.evidence_digest.as_deref()
    );
    bundle.validate().unwrap();
}

#[test]
fn conversion_result_attaches_foreign_boundary_provenance() {
    let temp_dir = tempfile::tempdir().unwrap();
    let metadata = make_test_metadata();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        )
        .unwrap();

    let provenance = result
        .build_result
        .manifest
        .provenance
        .as_ref()
        .expect("conversion provenance");
    assert_eq!(
        provenance.origin_class.as_deref(),
        Some("foreign-converted")
    );
    assert_eq!(provenance.hardening_level.as_deref(), Some("hermetic"));
    let boundary = provenance
        .foreign_conversion_boundary
        .as_ref()
        .expect("foreign conversion boundary");
    assert_eq!(boundary.source_format, "rpm");
    assert_eq!(
        boundary.source_checksum,
        "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
    );
    let build_risk_report = boundary
        .build_risk_report
        .as_ref()
        .expect("build risk report");
    assert_eq!(
        build_risk_report.status,
        crate::security::command_risk::CommandRiskStatus::Clean
    );
    assert_eq!(
        boundary.build_risk_report_hash.as_deref(),
        Some(canonical_json_hash(build_risk_report).unwrap().as_str())
    );
    let scriptlet_risk_report = boundary
        .scriptlet_risk_report
        .as_ref()
        .expect("scriptlet risk report");
    assert_eq!(
        boundary.scriptlet_risk_report_hash.as_deref(),
        Some(canonical_json_hash(scriptlet_risk_report).unwrap().as_str())
    );

    let package =
        crate::ccs::CcsPackage::parse(result.package_path.as_ref().unwrap().to_str().unwrap())
            .unwrap();
    assert!(
        package
            .manifest()
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.foreign_conversion_boundary.as_ref())
            .is_some()
    );
}

#[test]
fn conversion_boundary_records_foreign_scriptlet_command_risk() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "npm install synthetic-atomic-lockfile\n".to_string(),
        flags: None,
    }];
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(
            &metadata,
            &make_test_files(),
            "arch",
            "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        )
        .unwrap();

    let boundary = result
        .build_result
        .manifest
        .provenance
        .as_ref()
        .and_then(|provenance| provenance.foreign_conversion_boundary.as_ref())
        .expect("foreign conversion boundary");
    let report = boundary
        .scriptlet_risk_report
        .as_ref()
        .expect("scriptlet risk report");
    assert_eq!(
        report.status,
        crate::security::command_risk::CommandRiskStatus::Blocked
    );
    assert!(report.entries.iter().any(|entry| {
        entry.reason_code == crate::security::command_risk::PACKAGE_MANAGER_FETCH
    }));
    assert_eq!(
        boundary.scriptlet_risk_report_hash.as_deref(),
        Some(canonical_json_hash(report).unwrap().as_str())
    );
}

#[test]
fn converted_ccs_archive_round_trip_preserves_legacy_scriptlet_bundle() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "/sbin/ldconfig\n".to_string(),
        flags: None,
    }];
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        )
        .unwrap();
    let package_path = result.package_path.as_ref().unwrap();

    let file = std::fs::File::open(package_path).unwrap();
    let archive = crate::ccs::archive_reader::read_ccs_archive(file).unwrap();
    let bundle = archive.manifest.legacy_scriptlets.as_ref().unwrap();

    assert_eq!(bundle.source_package, metadata.name);
    bundle.validate().unwrap();
}

#[test]
fn remi_converter_context_flows_into_bundle_metadata() {
    let temp_dir = tempfile::tempdir().unwrap();
    let metadata = make_test_metadata();
    let converter = passive_test_converter(temp_dir.path())
        .with_source_distro("fedora")
        .with_source_release("44")
        .with_conversion_tool("remi");

    let result = converter
        .convert(
            &metadata,
            &make_test_files(),
            "rpm",
            "not-a-real-prefixed-sha",
        )
        .unwrap();

    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .unwrap();
    assert_eq!(bundle.source_distro.as_deref(), Some("fedora"));
    assert_eq!(bundle.source_release.as_deref(), Some("44"));
    assert_eq!(bundle.conversion_tool, "remi");
    assert_eq!(bundle.source_checksum, None);
}

#[test]
fn adapter_registry_classifies_formal_systemctl_invocation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "systemctl enable demo.service\n".to_string(),
        flags: None,
    }];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert!(result.scriptlet_classification.entries.iter().any(|entry| {
        matches!(
            &entry.classification,
            crate::ccs::convert::effects::ScriptletClassification::Known { .. }
        )
    }));
}

#[test]
fn shell_text_without_adapter_evidence_does_not_create_manifest_hooks() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PreInstall,
        interpreter: "/bin/sh".to_string(),
        content: "getent passwd demo || useradd -r demo\n".to_string(),
        flags: None,
    }];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert_ne!(
        result.scriptlet_metadata.scriptlet_fidelity,
        "fully-replaced"
    );
    assert_ne!(result.scriptlet_metadata.publication_status, "public");

    let manifest_hooks = &result.build_result.manifest.hooks;
    assert!(manifest_hooks.users.is_empty());
    assert!(manifest_hooks.groups.is_empty());
    assert!(manifest_hooks.services.is_empty());
    assert!(manifest_hooks.systemd.is_empty());
    assert!(
        !result.scriptlet_classification.entries.iter().any(|entry| {
            matches!(
                &entry.classification,
                ScriptletClassification::Known { effects, .. }
                    if effects.iter().any(|effect| effect.adapter_id.is_some())
            )
        })
    );
}

#[test]
fn conversion_integration_reports_complete_payload_backed_helpers() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "\
/sbin/ldconfig
systemctl daemon-reload
systemctl enable demo.service
systemd-tmpfiles --create /usr/lib/tmpfiles.d/demo.conf
systemd-sysusers /usr/lib/sysusers.d/demo.conf
update-mime-database /usr/share/mime
"
        .to_string(),
        flags: None,
    }];
    let mut files = make_test_files();
    files.extend([
        ExtractedFile {
            path: "/usr/lib/systemd/system/demo.service".to_string(),
            content: b"[Service]\nExecStart=/usr/bin/demo\n".to_vec(),
            size: 32,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/lib/tmpfiles.d/demo.conf".to_string(),
            content: b"d /run/demo 0755 root root -\n".to_vec(),
            size: 28,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/lib/sysusers.d/demo.conf".to_string(),
            content: b"u demo - \"Demo User\" /run/demo -\n".to_vec(),
            size: 32,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/share/mime/packages/demo.xml".to_string(),
            content: b"<mime-info/>".to_vec(),
            size: 12,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
    ]);
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    let complete_effects = result
        .scriptlet_classification
        .entries
        .iter()
        .filter_map(|entry| match &entry.classification {
            ScriptletClassification::Known { effects, .. } => Some(effects),
            _ => None,
        })
        .flatten()
        .filter(|effect| effect.replacement == EffectReplacement::Complete)
        .count();

    assert_eq!(
        complete_effects, 6,
        "all 6 known helper invocations should be complete"
    );
    result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle")
        .validate()
        .unwrap();
    assert!(result.build_result.manifest.hooks.systemd.is_empty());
}

#[test]
fn conversion_integration_projects_safe_sysctl_write_into_manifest_hook() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path()).with_target_profile_id("fedora-44");

    let result = convert_scriptlet_body(&converter, "sysctl -w kernel.example=1\n");

    let sysctl_hooks = &result.build_result.manifest.hooks.sysctl;
    assert_eq!(sysctl_hooks.len(), 1);
    assert_eq!(sysctl_hooks[0].key, "kernel.example");
    assert_eq!(sysctl_hooks[0].value, "1");
    assert!(!sysctl_hooks[0].only_if_lower);

    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.entries.len(), 1);
    assert_eq!(bundle.entries[0].decision.as_str(), "replaced");
    assert_eq!(bundle.entries[0].reason_code, "helper-complete-sysctl");
    assert_eq!(bundle.entries[0].effects.len(), 1);
    assert_eq!(
        bundle.entries[0].effects[0].adapter_id.as_deref(),
        Some("sysctl/v1")
    );
    assert_eq!(bundle.publication_status.as_str(), "public");
    assert_eq!(result.scriptlet_metadata.publication_status, "public");
}

fn convert_scriptlet_body(converter: &LegacyConverter, content: &str) -> ConversionResult {
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: content.to_string(),
        flags: None,
    }];
    converter
        .convert(&metadata, &make_test_files(), "rpm", "sha256:test")
        .expect("conversion succeeds")
}

#[test]
fn conversion_public_ready_for_profile_allowed_sysctl_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path()).with_target_profile_id("fedora-44");

    let result = convert_scriptlet_body(&converter, "sysctl -w kernel.example=1\n");

    let sysctl_hooks = &result.build_result.manifest.hooks.sysctl;
    assert_eq!(sysctl_hooks.len(), 1);
    assert_eq!(sysctl_hooks[0].key, "kernel.example");
    let bundle = result.legacy_scriptlets.as_ref().expect("scriptlet bundle");
    assert_eq!(bundle.publication_status.as_str(), "public");
    assert_eq!(result.scriptlet_metadata.publication_status, "public");
}

#[test]
fn adapter_sysctl_target_profile_private_review_reports_public_policy_reason() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path()).with_target_profile_id("fedora-44");

    let result = convert_scriptlet_body(&converter, "sysctl -w net.ipv4.ip_forward=1\n");

    let sysctl_hooks = &result.build_result.manifest.hooks.sysctl;
    assert_eq!(sysctl_hooks.len(), 1);
    assert_eq!(sysctl_hooks[0].key, "net.ipv4.ip_forward");
    let bundle = result.legacy_scriptlets.as_ref().expect("scriptlet bundle");
    assert_eq!(bundle.decision_counts.replaced, 1);
    assert_eq!(bundle.publication_status.as_str(), "private-review");
    assert_eq!(
        result.scriptlet_metadata.review_reason_codes,
        vec!["public-policy-sysctl-target-profile-unsupported".to_string()]
    );
    let bundle_summary =
        ScriptletBundleSummary::from_bundle(bundle, bundle.evidence_digest.clone());
    assert_eq!(bundle_summary.publication_status, "private-review");
    assert_eq!(
        bundle_summary.review_reason_codes,
        vec!["public-policy-sysctl-target-profile-unsupported".to_string()]
    );
}

#[test]
fn conversion_integration_projects_payload_backed_setuid_into_file_mode_authority() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "chmod u+s /usr/bin/test\n".to_string(),
        flags: None,
    }];
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &make_test_files(), "rpm", "sha256:test")
        .expect("conversion succeeds");

    let setuid_file = result
        .build_result
        .files
        .iter()
        .find(|file| file.path == "/usr/bin/test")
        .expect("payload file should remain present");
    assert_eq!(setuid_file.mode & 0o4000, 0o4000);
    assert_eq!(
        result.build_result.manifest.policy.allow_setuid_paths,
        vec!["/usr/bin/test".to_string()]
    );

    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.entries.len(), 1);
    assert_eq!(bundle.entries[0].decision.as_str(), "replaced");
    assert_eq!(bundle.entries[0].reason_code, "helper-complete-setuid-mode");
    assert_eq!(bundle.entries[0].effects.len(), 1);
    assert_eq!(
        bundle.entries[0].effects[0].adapter_id.as_deref(),
        Some("setuid-mode/v1")
    );
    assert_eq!(result.scriptlet_metadata.publication_status, "public");
}

#[test]
fn conversion_integration_projects_payload_backed_setcap_into_file_capability_authority() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "setcap cap_net_bind_service=+ep /usr/bin/test\n".to_string(),
        flags: None,
    }];
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &make_test_files(), "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert_eq!(
        result.scriptlet_metadata.scriptlet_fidelity,
        "fully-replaced"
    );
    assert_eq!(
        result.scriptlet_metadata.target_compatibility,
        "conary-portable"
    );
    assert_eq!(result.scriptlet_metadata.publication_status, "public");
    assert!(result.scriptlet_metadata.review_reason_codes.is_empty());
    assert_eq!(result.scriptlet_metadata.decision_counts.replaced, 1);
    assert_eq!(result.scriptlet_metadata.decision_counts.review, 0);

    let file_capabilities = &result.build_result.manifest.file_capabilities;
    assert_eq!(file_capabilities.len(), 1);
    assert_eq!(file_capabilities[0].path, "/usr/bin/test");
    assert_eq!(
        file_capabilities[0].capabilities,
        vec!["cap_net_bind_service".to_string()]
    );
    assert!(file_capabilities[0].permitted);
    assert!(file_capabilities[0].effective);
    assert!(!file_capabilities[0].inheritable);

    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.entries.len(), 1);
    assert_eq!(bundle.entries[0].decision.as_str(), "replaced");
    assert_eq!(
        bundle.entries[0].reason_code,
        "helper-complete-file-capability"
    );
    assert_eq!(bundle.entries[0].effects.len(), 1);
    assert_eq!(
        bundle.entries[0].effects[0].adapter_id.as_deref(),
        Some("file-capability/v1")
    );
    assert_eq!(result.scriptlet_metadata.publication_status, "public");
}

#[test]
fn conversion_integration_keeps_high_risk_setcap_replaced_but_private_review() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "setcap cap_sys_admin=+ep /usr/bin/test\n".to_string(),
        flags: None,
    }];
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &make_test_files(), "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert_eq!(
        result.scriptlet_metadata.scriptlet_fidelity,
        "fully-replaced"
    );
    assert_eq!(
        result.scriptlet_metadata.target_compatibility,
        "conary-portable"
    );
    assert_eq!(
        result.scriptlet_metadata.publication_status,
        "private-review"
    );
    assert_eq!(result.scriptlet_metadata.decision_counts.replaced, 1);
    assert_eq!(result.scriptlet_metadata.decision_counts.review, 0);
    assert_eq!(
        result.scriptlet_metadata.review_reason_codes,
        vec!["public-policy-file-capability-private-review".to_string()]
    );

    let file_capabilities = &result.build_result.manifest.file_capabilities;
    assert_eq!(file_capabilities.len(), 1);
    assert_eq!(file_capabilities[0].path, "/usr/bin/test");
    assert_eq!(
        file_capabilities[0].capabilities,
        vec!["cap_sys_admin".to_string()]
    );

    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.entries.len(), 1);
    assert_eq!(bundle.entries[0].decision.as_str(), "replaced");
    assert_eq!(
        bundle.entries[0].reason_code,
        "helper-complete-file-capability"
    );
    assert_eq!(
        bundle.entries[0].effects[0].adapter_id.as_deref(),
        Some("file-capability/v1")
    );
}

#[path = "tests/policy.rs"]
mod policy;

#[path = "tests/manifest.rs"]
mod manifest;
