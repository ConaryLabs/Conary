// conary-core/src/ccs/convert/scriptlet_bundle/test_support.rs

use super::{ScriptletBundleBuild, ScriptletBundleInput, build_legacy_scriptlet_bundle};
use crate::ccs::convert::effects::{
    ScriptletClassification, ScriptletClassificationReport, ScriptletEffectEvidence,
};
use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
use crate::packages::common::PackageMetadata;
use crate::packages::native_abi::*;
use crate::packages::traits::{ExtractedFile, ScriptletPhase};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub(super) fn package_metadata(name: &str, version: &str) -> PackageMetadata {
    PackageMetadata {
        package_path: PathBuf::from(format!("/tmp/{name}-{version}.rpm")),
        name: name.to_string(),
        version: version.to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some("test package".to_string()),
        files: Vec::new(),
        dependencies: Vec::new(),
        provides: Vec::new(),
        scriptlets: Vec::new(),
        native_scriptlet_abi: Vec::new(),
        config_files: Vec::new(),
    }
}

pub(super) fn complete_effect(kind: &str, command: &str) -> ScriptletEffectEvidence {
    ScriptletEffectEvidence {
        kind: kind.to_string(),
        source: EffectSource::StaticSignal,
        confidence: EffectConfidence::Inferred,
        replacement: EffectReplacement::Complete,
        adapter_id: Some("test-adapter/v1".to_string()),
        adapter_digest: Some(crate::hash::sha256_prefixed(b"test-adapter/v1")),
        command: Some(command.to_string()),
        args: Vec::new(),
        path: None,
        reason_code: Some(format!("{kind}-complete")),
        extra: BTreeMap::new(),
    }
}

pub(super) fn known_report_with_effect(
    effect: ScriptletEffectEvidence,
) -> ScriptletClassificationReport {
    let mut report = ScriptletClassificationReport::default();
    report.push(
        "scriptlet:0:post-install",
        ScriptletClassification::Known {
            reason_code: effect
                .reason_code
                .clone()
                .unwrap_or_else(|| "known-complete".to_string()),
            effects: vec![effect],
        },
    );
    report
}

pub(super) fn bundle_for_metadata(
    metadata: &PackageMetadata,
    files: &[ExtractedFile],
    classification: &ScriptletClassificationReport,
) -> anyhow::Result<ScriptletBundleBuild> {
    build_legacy_scriptlet_bundle(ScriptletBundleInput {
        source_metadata: metadata,
        final_metadata: metadata,
        source_files: files,
        final_files: files,
        source_format: "rpm",
        source_distro: Some("fedora-44"),
        source_release: Some("44"),
        source_arch: Some("x86_64"),
        source_checksum: None,
        classification,
        conversion_tool: "remi",
        conversion_tool_version: "0.1.0",
    })
}

pub(super) fn native_entry_with_body(bytes: Vec<u8>) -> NativeScriptletEntry {
    NativeScriptletEntry {
        id: "rpm:%post".to_string(),
        format: NativeScriptletFormat::Rpm,
        kind: NativeScriptletKind::Executable,
        native_slot: "%post".to_string(),
        primary_lifecycle: NativeLifecyclePath::PostInstall,
        compatibility_phase: Some(ScriptletPhase::PostInstall),
        lifecycle_paths: vec![NativeLifecyclePath::PostInstall],
        interpreter: Some("/bin/sh".to_string()),
        interpreter_args: Vec::new(),
        body: NativeScriptletBody::from_bytes(bytes),
        invocation: NativeInvocationContract::none(),
        order: NativeTransactionOrder::new(NativeTransactionPosition::AfterPayload),
        support: NativeScriptletSupport::Parsed,
        metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
            slot: RpmScriptletSlot::Post,
            scriptlet_flags: None,
            trigger: None,
        }),
    }
}

pub(super) fn rpm_trigger_entry() -> NativeScriptletEntry {
    let mut entry = native_entry_with_body(b"echo trigger\n".to_vec());
    entry.id = "rpm:trigger".to_string();
    entry.native_slot = "%filetriggerin".to_string();
    entry.primary_lifecycle = NativeLifecyclePath::FileTrigger;
    entry.lifecycle_paths = vec![NativeLifecyclePath::FileTrigger];
    entry.invocation.stdin = NativeStdinContract::Paths;
    entry.metadata = NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
        slot: RpmScriptletSlot::Trigger,
        scriptlet_flags: Some(RpmScriptletFlagsMetadata {
            names: vec!["EXPAND".to_string()],
            raw_bits: 1,
        }),
        trigger: Some(RpmTriggerMetadata {
            family: RpmTriggerFamily::File,
            conditions: vec![RpmTriggerCondition {
                name: "hicolor-icon-theme".to_string(),
                action: RpmTriggerAction::Install,
                version: Some("1.0".to_string()),
                comparison: Some(">=".to_string()),
                raw_flags: 8,
            }],
            file_globs: vec!["/usr/share/icons/*".to_string()],
        }),
    });
    entry
}

pub(super) fn deb_triggers_entry() -> NativeScriptletEntry {
    let mut entry = native_entry_with_body(b"interest-noawait icon-cache\n".to_vec());
    entry.id = "deb:triggers".to_string();
    entry.format = NativeScriptletFormat::Deb;
    entry.native_slot = "triggers".to_string();
    entry.primary_lifecycle = NativeLifecyclePath::Trigger;
    entry.lifecycle_paths = vec![NativeLifecyclePath::Trigger];
    entry.invocation.stdin = NativeStdinContract::Debconf;
    entry.metadata = NativeScriptletMetadata::Deb(DebNativeScriptletMetadata {
        control_member: DebControlMember::Triggers,
        maintainer_modes: vec![DebMaintainerInvocation {
            mode: DebMaintainerMode::Triggered,
            args: Vec::new(),
            lifecycle_paths: vec![NativeLifecyclePath::Trigger],
        }],
        trigger_declarations: vec![DebTriggerDeclaration {
            directive: DebTriggerDirective::Interest,
            trigger_name: "icon-cache".to_string(),
            await_mode: DebTriggerAwaitMode::NoAwait,
            raw_line: "interest-noawait icon-cache".to_string(),
        }],
    });
    entry
}

pub(super) fn arch_install_entry() -> NativeScriptletEntry {
    let mut entry = native_entry_with_body(b"post_install() { echo ok; }\n".to_vec());
    entry.id = "arch:post_install".to_string();
    entry.format = NativeScriptletFormat::Arch;
    entry.native_slot = "post_install".to_string();
    entry.primary_lifecycle = NativeLifecyclePath::PostInstall;
    entry.lifecycle_paths = vec![NativeLifecyclePath::PostInstall];
    entry.metadata = NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(
        ArchInstallScriptletMetadata {
            install_source_sha256: crate::hash::sha256_prefixed(b"post_install() { echo ok; }\n"),
            function_name: "post_install".to_string(),
            function_body: Some("echo ok;".to_string()),
            function_body_sha256: Some(crate::hash::sha256_prefixed(b"echo ok;")),
            extraction_status: ArchFunctionExtractionStatus::Parsed,
        },
    ));
    entry
}

pub(super) fn arch_alpm_hook_entry() -> NativeScriptletEntry {
    NativeScriptletEntry {
        id: "arch:hook".to_string(),
        format: NativeScriptletFormat::Arch,
        kind: NativeScriptletKind::ControlArtifact,
        native_slot: "alpm-hook".to_string(),
        primary_lifecycle: NativeLifecyclePath::Trigger,
        compatibility_phase: None,
        lifecycle_paths: vec![NativeLifecyclePath::Trigger],
        interpreter: None,
        interpreter_args: Vec::new(),
        body: NativeScriptletBody::from_bytes(
            b"[Trigger]\nType = Package\nTarget = demo\n[Action]\nWhen = PostTransaction\nExec = /bin/true\n"
                .to_vec(),
        ),
        invocation: NativeInvocationContract::none(),
        order: NativeTransactionOrder::new(NativeTransactionPosition::ControlArtifact),
        support: NativeScriptletSupport::DeferredReview {
            reason_code: "arch-alpm-hook-semantics-deferred".to_string(),
        },
        metadata: NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(
            ArchAlpmHookMetadata {
                hook_path: "/usr/share/libalpm/hooks/demo.hook".to_string(),
                triggers: vec![ArchAlpmHookTrigger {
                    operations: vec![ArchAlpmHookOperation::Install],
                    trigger_type: ArchAlpmHookTriggerType::Package,
                    targets: vec!["demo".to_string()],
                }],
                action: Some(ArchAlpmHookAction {
                    description: Some("demo hook".to_string()),
                    when: NativeTransactionPosition::AfterTransaction,
                    exec: "/bin/true".to_string(),
                    depends: vec!["bash".to_string()],
                    abort_on_fail: false,
                    needs_targets: false,
                }),
            },
        )),
    }
}
