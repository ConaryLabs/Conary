// apps/conary/tests/common/legacy_scriptlet_fixtures.rs

use anyhow::Result;
use conary_core::ccs::builder::{CcsBuilder, write_ccs_package};
use conary_core::ccs::legacy_scriptlets::{
    DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
    LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy, PublicationStatus,
    RpmTriggerMetadata, RpmTriggerTargetConstraint, ScriptletDecision, ScriptletFidelity,
    SourceFormat, TargetCompatibility, TransactionOrder, VersionScheme,
};
use conary_core::ccs::manifest::CcsManifest;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::TempDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyBundleFixture {
    NoBundle,
    NativeFree,
    ReplacedOnly,
    ReviewEntry,
    BlockedEntry,
    SameSourceLegacyPostInstall,
    FutureLegacyPostRemove,
    RawTriggerLegacy,
    UnsupportedNativeInvocation,
}

impl LegacyBundleFixture {
    pub fn package_name(self) -> &'static str {
        match self {
            Self::NoBundle => "legacy-fixture-none",
            Self::NativeFree => "legacy-fixture-native-free",
            Self::ReplacedOnly => "legacy-fixture-replaced",
            Self::ReviewEntry => "legacy-fixture-review",
            Self::BlockedEntry => "legacy-fixture-blocked",
            Self::SameSourceLegacyPostInstall => "legacy-fixture-post",
            Self::FutureLegacyPostRemove => "legacy-fixture-remove",
            Self::RawTriggerLegacy => "legacy-fixture-trigger",
            Self::UnsupportedNativeInvocation => "legacy-fixture-unsupported-native",
        }
    }
}

pub fn synthetic_legacy_bundle(case: LegacyBundleFixture) -> Option<LegacyScriptletBundle> {
    match case {
        LegacyBundleFixture::NoBundle => None,
        LegacyBundleFixture::NativeFree => {
            Some(bundle_fixture(case, ScriptletFidelity::NativeFree, vec![]))
        }
        LegacyBundleFixture::ReplacedOnly => Some(bundle_fixture(
            case,
            ScriptletFidelity::FullyReplaced,
            vec![entry_fixture(
                "rpm:%post",
                LifecyclePath::PostInstall,
                ScriptletDecision::Replaced,
                "ldconfig\n",
            )],
        )),
        LegacyBundleFixture::ReviewEntry => Some(bundle_fixture(
            case,
            ScriptletFidelity::ReviewRequired,
            vec![entry_fixture(
                "rpm:%post",
                LifecyclePath::PostInstall,
                ScriptletDecision::Review,
                "systemctl daemon-reload\n",
            )],
        )),
        LegacyBundleFixture::BlockedEntry => Some(bundle_fixture(
            case,
            ScriptletFidelity::Blocked,
            vec![entry_fixture(
                "rpm:%pre",
                LifecyclePath::PreInstall,
                ScriptletDecision::Blocked,
                "useradd unsafe-fixture\n",
            )],
        )),
        LegacyBundleFixture::SameSourceLegacyPostInstall => Some(bundle_fixture(
            case,
            ScriptletFidelity::LegacyReplay,
            vec![entry_fixture(
                "rpm:%post",
                LifecyclePath::PostInstall,
                ScriptletDecision::Legacy,
                "echo replay-post-install\n",
            )],
        )),
        LegacyBundleFixture::FutureLegacyPostRemove => Some(bundle_fixture(
            case,
            ScriptletFidelity::LegacyReplay,
            vec![entry_fixture(
                "rpm:%postun",
                LifecyclePath::PostRemove,
                ScriptletDecision::Legacy,
                "echo replay-post-remove\n",
            )],
        )),
        LegacyBundleFixture::RawTriggerLegacy => {
            let mut entry = entry_fixture(
                "rpm:%triggerin",
                LifecyclePath::Trigger,
                ScriptletDecision::Legacy,
                "echo replay-trigger\n",
            );
            entry.rpm_trigger = Some(RpmTriggerMetadata {
                kind: "file-trigger".to_string(),
                condition: Some("in".to_string()),
                target_constraints: vec![RpmTriggerTargetConstraint {
                    package: "systemd".to_string(),
                    operator: Some(">=".to_string()),
                    version: Some("255".to_string()),
                    extra: BTreeMap::new(),
                }],
                priority: Some(100),
                file_globs: vec!["/usr/lib/systemd/system/*.service".to_string()],
                stdin_contract: Some("paths".to_string()),
                transaction_order: Some("post-transaction".to_string()),
                extra: BTreeMap::new(),
            });
            Some(bundle_fixture(
                case,
                ScriptletFidelity::LegacyReplay,
                vec![entry],
            ))
        }
        LegacyBundleFixture::UnsupportedNativeInvocation => {
            let mut entry = entry_fixture(
                "rpm:%post",
                LifecyclePath::PostInstall,
                ScriptletDecision::Legacy,
                "cat >/var/lib/fixture/state\n",
            );
            entry.native_invocation.stdin = Some("paths".to_string());
            Some(bundle_fixture(
                case,
                ScriptletFidelity::LegacyReplay,
                vec![entry],
            ))
        }
    }
}

pub fn build_ccs_package_fixture(
    name: &str,
    version: &str,
    bundle: Option<LegacyScriptletBundle>,
) -> Result<(TempDir, PathBuf)> {
    let temp = tempfile::tempdir()?;
    let source_dir = temp.path().join("src");
    std::fs::create_dir_all(source_dir.join("usr/bin"))?;
    std::fs::write(source_dir.join("usr/bin/fixture"), b"fixture\n")?;

    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.legacy_scriptlets = bundle;

    let result = CcsBuilder::new(manifest, &source_dir).build()?;
    let package_path = temp.path().join(format!("{name}.ccs"));
    write_ccs_package(&result, &package_path)?;
    Ok((temp, package_path))
}

fn bundle_fixture(
    case: LegacyBundleFixture,
    scriptlet_fidelity: ScriptletFidelity,
    entries: Vec<LegacyScriptletEntry>,
) -> LegacyScriptletBundle {
    let publication_status = match case {
        LegacyBundleFixture::ReviewEntry => PublicationStatus::PrivateReview,
        LegacyBundleFixture::BlockedEntry => PublicationStatus::Blocked,
        _ => PublicationStatus::Public,
    };
    let target_compatibility = match case {
        LegacyBundleFixture::ReviewEntry => TargetCompatibility::ReviewRequired,
        LegacyBundleFixture::BlockedEntry => TargetCompatibility::Blocked,
        _ => TargetCompatibility::SourceNative,
    };

    LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 1,
        source_format: SourceFormat::Rpm,
        source_family: "fedora-rhel".to_string(),
        source_distro: Some("fedora".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: case.package_name().to_string(),
        source_version: "1.0.0-1.fc44".to_string(),
        source_checksum: Some(conary_core::hash::sha256_prefixed(
            format!("{}-source", case.package_name()).as_bytes(),
        )),
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi-test".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "goal6-test-fixture".to_string(),
        adapter_registry_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{}-adapter-registry", case.package_name()).as_bytes(),
        )),
        target_policy_digest: None,
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{}-evidence", case.package_name()).as_bytes(),
        )),
        target_compatibility,
        allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy: PublicationPolicy::PublicIfNoBlocked,
        publication_status,
        scriptlet_fidelity,
        decision_counts: decision_counts(&entries),
        unsupported_class_counts: BTreeMap::new(),
        entries,
        extra: BTreeMap::new(),
    }
}

fn entry_fixture(
    id: &str,
    phase: LifecyclePath,
    decision: ScriptletDecision,
    body: &str,
) -> LegacyScriptletEntry {
    let lifecycle_paths = lifecycle_paths_for_phase(&phase);
    let transaction_position = transaction_position_for_phase(&phase).to_string();
    let reason_code = reason_code_for_decision(&decision).to_string();

    LegacyScriptletEntry {
        id: id.to_string(),
        native_slot: id.split(':').nth(1).unwrap_or("%post").to_string(),
        phase,
        lifecycle_paths,
        interpreter: "/bin/sh".to_string(),
        interpreter_args: vec!["-e".to_string()],
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation {
            args: vec!["1".to_string()],
            environment: vec!["RPM_INSTALL_PREFIX=/".to_string()],
            stdin: None,
            chroot: None,
            extra: BTreeMap::new(),
        },
        transaction_order: TransactionOrder {
            position: transaction_position,
            before: vec![],
            after: vec![],
            extra: BTreeMap::new(),
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: vec![],
        decision,
        reason_code,
        human_reason: Some("goal 6 fixture".to_string()),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{id}:{body}").as_bytes(),
        )),
        source_evidence_refs: vec![format!("capture:{id}")],
        effects: vec![],
        unknown_commands: vec![],
        blocked_classes: vec![],
        rpm_trigger: None,
        deb_maintainer: None,
        arch_install: None,
        residual_replay: None,
        extra: BTreeMap::new(),
    }
}

fn lifecycle_paths_for_phase(phase: &LifecyclePath) -> Vec<String> {
    let path = match phase {
        LifecyclePath::PreInstall => "install:pre",
        LifecyclePath::PostInstall => "install:post",
        LifecyclePath::PreUpgrade => "upgrade:new-pre",
        LifecyclePath::PostUpgrade => "upgrade:new-post",
        LifecyclePath::PreRemove => "remove:pre",
        LifecyclePath::PostRemove => "remove:post",
        LifecyclePath::PreTransaction => "transaction:pre",
        LifecyclePath::PostTransaction => "transaction:post",
        LifecyclePath::Trigger => "trigger",
        LifecyclePath::FileTrigger => "file-trigger",
        LifecyclePath::Unknown(_) => "unknown",
    };
    vec![path.to_string()]
}

fn transaction_position_for_phase(phase: &LifecyclePath) -> &'static str {
    match phase {
        LifecyclePath::PreInstall
        | LifecyclePath::PreUpgrade
        | LifecyclePath::PreRemove
        | LifecyclePath::PreTransaction => "before-payload",
        LifecyclePath::Trigger | LifecyclePath::FileTrigger => "transaction",
        LifecyclePath::PostInstall
        | LifecyclePath::PostUpgrade
        | LifecyclePath::PostRemove
        | LifecyclePath::PostTransaction
        | LifecyclePath::Unknown(_) => "after-payload",
    }
}

fn reason_code_for_decision(decision: &ScriptletDecision) -> &'static str {
    match decision {
        ScriptletDecision::Replaced => "adapter-covered",
        ScriptletDecision::Legacy => "legacy-replay-required",
        ScriptletDecision::Blocked => "blocked-unsafe-operation",
        ScriptletDecision::Review => "operator-review-required",
        ScriptletDecision::Unknown(_) => "unknown-decision",
    }
}

fn decision_counts(entries: &[LegacyScriptletEntry]) -> DecisionCounts {
    let mut counts = DecisionCounts::default();
    for entry in entries {
        match &entry.decision {
            ScriptletDecision::Replaced => counts.replaced += 1,
            ScriptletDecision::Legacy => counts.legacy += 1,
            ScriptletDecision::Blocked => counts.blocked += 1,
            ScriptletDecision::Review => counts.review += 1,
            ScriptletDecision::Unknown(value) => {
                *counts.extra.entry(value.clone()).or_insert(0) += 1;
            }
        }
    }
    counts
}
