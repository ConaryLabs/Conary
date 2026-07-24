// conary-core/src/ccs/legacy_scriptlets/tests.rs

use super::*;
use std::collections::BTreeMap;

fn sha256_prefixed(body: &str) -> String {
    crate::hash::sha256_prefixed(body.as_bytes())
}

fn sample_effect() -> ScriptletEffect {
    ScriptletEffect {
        kind: "ldconfig".to_string(),
        source: EffectSource::ShellAst,
        confidence: EffectConfidence::Declared,
        replacement: EffectReplacement::Complete,
        adapter_id: Some("ldconfig/v1".to_string()),
        adapter_digest: Some(
            "sha256:1111111111111111111111111111111111111111111111111111111111111111".to_string(),
        ),
        command: Some("ldconfig".to_string()),
        args: vec!["-X".to_string()],
        path: Some("/usr/lib64".to_string()),
        reason_code: Some("ldconfig-cache-refresh".to_string()),
        extra: BTreeMap::new(),
    }
}

fn sample_entry(id: &str, decision: ScriptletDecision, body: &str) -> LegacyScriptletEntry {
    LegacyScriptletEntry {
        id: id.to_string(),
        native_slot: "%post".to_string(),
        phase: LifecyclePath::PostInstall,
        lifecycle_paths: vec!["install:first".to_string()],
        interpreter: "/bin/sh".to_string(),
        interpreter_args: vec!["-e".to_string()],
        body_sha256: sha256_prefixed(body),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation {
            args: vec!["1".to_string()],
            environment: vec!["RPM_INSTALL_PREFIX=/".to_string()],
            stdin: Some("none".to_string()),
            chroot: Some("install-root".to_string()),
            extra: BTreeMap::new(),
        },
        transaction_order: TransactionOrder {
            position: "after-payload".to_string(),
            before: vec![],
            after: vec!["payload".to_string()],
            extra: BTreeMap::new(),
        },
        timeout_ms: 30_000,
        sandbox: Some(ScriptletSandboxRequirements {
            network: false,
            namespaces: vec!["mount".to_string(), "pid".to_string()],
            seccomp_profile: Some("legacy-scriptlet/default".to_string()),
            extra: BTreeMap::new(),
        }),
        capabilities: vec!["ldconfig".to_string()],
        decision,
        reason_code: "test-fixture".to_string(),
        human_reason: Some("fixture entry".to_string()),
        evidence_digest: Some(
            "sha256:2222222222222222222222222222222222222222222222222222222222222222".to_string(),
        ),
        source_evidence_refs: vec!["capture:rpm:%post".to_string()],
        effects: vec![sample_effect()],
        unknown_command_evidence: vec![],
        blocked_classes: vec![],
        boot_security_intents: Vec::new(),
        security_policy_intents: Vec::new(),
        rpm_trigger: None,
        deb_maintainer: None,
        arch_install: None,
        residual_replay: None,
        extra: BTreeMap::new(),
    }
}

fn sample_bundle() -> LegacyScriptletBundle {
    LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 2,
        source_format: SourceFormat::Rpm,
        source_family: "fedora-rhel".to_string(),
        source_distro: Some("fedora".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: "nginx".to_string(),
        source_version: "1.28.0-1.fc44".to_string(),
        source_checksum: Some(
            "sha256:3333333333333333333333333333333333333333333333333333333333333333".to_string(),
        ),
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "safe-or-legacy".to_string(),
        adapter_registry_digest: Some(
            "sha256:4444444444444444444444444444444444444444444444444444444444444444".to_string(),
        ),
        target_policy_digest: Some(
            "sha256:5555555555555555555555555555555555555555555555555555555555555555".to_string(),
        ),
        evidence_digest: Some(
            "sha256:6666666666666666666666666666666666666666666666666666666666666666".to_string(),
        ),
        target_compatibility: TargetCompatibility::SourceNative,
        allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy: PublicationPolicy::PublicIfNoBlocked,
        publication_status: PublicationStatus::PrivateReview,
        scriptlet_fidelity: ScriptletFidelity::Mixed,
        decision_counts: DecisionCounts {
            replaced: 1,
            legacy: 1,
            blocked: 0,
            review: 0,
            extra: BTreeMap::new(),
        },
        unsupported_class_counts: BTreeMap::new(),
        security_policy_intents: Vec::new(),
        entries: vec![
            sample_entry("rpm:%preun", ScriptletDecision::Replaced, "ldconfig\n"),
            sample_entry(
                "rpm:%post",
                ScriptletDecision::Legacy,
                "systemctl daemon-reload\n",
            ),
        ],
        extra: BTreeMap::new(),
    }
}

#[test]
fn legacy_scriptlet_bundle_round_trips_core_fields() {
    let bundle = sample_bundle();

    let encoded = toml::to_string_pretty(&bundle).expect("serialize bundle");
    let decoded: LegacyScriptletBundle = toml::from_str(&encoded).expect("parse bundle");

    assert_eq!(decoded.schema, LEGACY_SCRIPTLET_SCHEMA_V1);
    assert_eq!(decoded.source_format, SourceFormat::Rpm);
    assert_eq!(
        decoded.target_compatibility,
        TargetCompatibility::SourceNative
    );
    assert_eq!(decoded.foreign_replay_policy, ForeignReplayPolicy::Deny);
    assert_eq!(decoded.entries.len(), 2);
    assert_eq!(decoded.entries[0].decision, ScriptletDecision::Replaced);
    assert_eq!(decoded.entries[1].decision, ScriptletDecision::Legacy);
    assert_eq!(
        decoded.entries[0].effects[0].replacement,
        EffectReplacement::Complete
    );
}

#[test]
fn legacy_scriptlet_bundle_round_trips_reserved_metadata() {
    let mut bundle = sample_bundle();
    let entry = bundle.entries.first_mut().expect("fixture entry");
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
    entry.deb_maintainer = Some(DebMaintainerMetadata {
        invocation_mode: Some("configure".to_string()),
        old_version: Some("1.27".to_string()),
        new_version: Some("1.28".to_string()),
        triggers_content: Some("interest-noawait nginx-reload".to_string()),
        trigger_names: vec!["nginx-reload".to_string()],
        purge: true,
        abort: true,
        noninteractive: true,
        extra: BTreeMap::new(),
    });
    entry.arch_install = Some(ArchInstallMetadata {
        install_digest: Some(
            "sha256:7777777777777777777777777777777777777777777777777777777777777777".to_string(),
        ),
        called_function: Some("post_install".to_string()),
        old_version: Some("1.27-1".to_string()),
        new_version: Some("1.28-1".to_string()),
        wrapper_source_digest: Some(
            "sha256:8888888888888888888888888888888888888888888888888888888888888888".to_string(),
        ),
        extra: BTreeMap::new(),
    });
    entry.residual_replay = Some(ResidualReplayMetadata {
        superseded_effect_kinds: vec!["ldconfig".to_string()],
        wrapper_strategy: Some("source-and-suppress".to_string()),
        suppression_markers: vec!["CONARY_SUPPRESS_LDCONFIG=1".to_string()],
        residual_body_digest: Some(
            "sha256:9999999999999999999999999999999999999999999999999999999999999999".to_string(),
        ),
        extra: BTreeMap::new(),
    });

    let encoded = toml::to_string_pretty(&bundle).expect("serialize bundle");
    let decoded: LegacyScriptletBundle = toml::from_str(&encoded).expect("parse bundle");
    let decoded_entry = decoded.entries.first().expect("round-tripped entry");

    assert_eq!(
        decoded_entry
            .rpm_trigger
            .as_ref()
            .expect("rpm trigger")
            .file_globs,
        vec!["/usr/lib/systemd/system/*.service"]
    );
    assert!(decoded_entry.deb_maintainer.as_ref().expect("deb").purge);
    assert_eq!(
        decoded_entry
            .arch_install
            .as_ref()
            .expect("arch")
            .called_function
            .as_deref(),
        Some("post_install")
    );
    assert_eq!(
        decoded_entry
            .residual_replay
            .as_ref()
            .expect("residual")
            .superseded_effect_kinds,
        vec!["ldconfig"]
    );
}

#[test]
fn legacy_scriptlet_bundle_preserves_unknown_optional_fields() {
    let mut bundle = sample_bundle();
    bundle.extra.insert(
        "future_top_level".to_string(),
        toml::Value::String("kept".to_string()),
    );
    bundle.entries[0].extra.insert(
        "future_entry_field".to_string(),
        toml::Value::String("also-kept".to_string()),
    );
    bundle.entries[0].effects[0]
        .extra
        .insert("future_effect_field".to_string(), toml::Value::Integer(7));

    let encoded = toml::to_string_pretty(&bundle).expect("serialize bundle");
    let decoded: LegacyScriptletBundle = toml::from_str(&encoded).expect("parse bundle");

    assert_eq!(
        decoded
            .extra
            .get("future_top_level")
            .and_then(toml::Value::as_str),
        Some("kept")
    );
    assert_eq!(
        decoded.entries[0]
            .extra
            .get("future_entry_field")
            .and_then(toml::Value::as_str),
        Some("also-kept")
    );
    assert_eq!(
        decoded.entries[0].effects[0]
            .extra
            .get("future_effect_field")
            .and_then(toml::Value::as_integer),
        Some(7)
    );
}

#[test]
fn legacy_scriptlet_bundle_retains_unknown_typed_enum_values() {
    let toml = r#"
schema = "conary.legacy-scriptlets.v1"
schema_revision = 2
source_format = "apk"
source_family = "alpine"
source_package = "busybox"
source_version = "1.37.0"
version_scheme = "apk"
conversion_tool = "remi"
conversion_tool_version = "0.8.0"
conversion_policy = "passive-test"
target_compatibility = "future-compatible"
foreign_replay_policy = "operator-review"
publication_policy = "curated-lane"
publication_status = "staged"
scriptlet_fidelity = "machine-reviewed"

[decision_counts]
review = 0
"#;

    let decoded: LegacyScriptletBundle = toml::from_str(toml).expect("parse bundle");

    assert_eq!(
        decoded.source_format,
        SourceFormat::Unknown("apk".to_string())
    );
    assert_eq!(
        decoded.version_scheme,
        VersionScheme::Unknown("apk".to_string())
    );
    assert_eq!(
        decoded.target_compatibility,
        TargetCompatibility::Unknown("future-compatible".to_string())
    );
    assert_eq!(
        decoded.foreign_replay_policy,
        ForeignReplayPolicy::Unknown("operator-review".to_string())
    );
    assert_eq!(
        decoded.publication_policy,
        PublicationPolicy::Unknown("curated-lane".to_string())
    );
    assert_eq!(
        decoded.publication_status,
        PublicationStatus::Unknown("staged".to_string())
    );
    assert_eq!(
        decoded.scriptlet_fidelity,
        ScriptletFidelity::Unknown("machine-reviewed".to_string())
    );
}

#[test]
fn legacy_scriptlet_bundle_accepts_zero_entry_native_free_package() {
    let mut bundle = sample_bundle();
    bundle.entries.clear();
    bundle.decision_counts = DecisionCounts::default();
    bundle.scriptlet_fidelity = ScriptletFidelity::NativeFree;

    let encoded = toml::to_string_pretty(&bundle).expect("serialize bundle");
    let decoded: LegacyScriptletBundle = toml::from_str(&encoded).expect("parse bundle");

    assert!(decoded.entries.is_empty());
    assert_eq!(decoded.scriptlet_fidelity, ScriptletFidelity::NativeFree);
}

#[test]
fn legacy_scriptlet_bundle_rejects_duplicate_entry_ids() {
    let mut bundle = sample_bundle();
    bundle.entries[1].id = bundle.entries[0].id.clone();

    let error = bundle.validate().expect_err("duplicate IDs must fail");

    assert!(error.to_string().contains("duplicate entry id"));
}

#[test]
fn legacy_scriptlet_bundle_rejects_mismatched_decision_counts() {
    let mut bundle = sample_bundle();
    bundle.decision_counts.legacy = 0;

    let error = bundle.validate().expect_err("mismatched counts must fail");

    assert!(error.to_string().contains("decision counts"));
}

#[test]
fn legacy_scriptlet_bundle_rejects_zero_timeout() {
    let mut bundle = sample_bundle();
    bundle.entries[0].timeout_ms = 0;

    let error = bundle.validate().expect_err("zero timeout must fail");

    assert!(error.to_string().contains("timeout_ms"));
}

#[test]
fn legacy_scriptlet_bundle_rejects_malformed_sha256_digest() {
    let mut bundle = sample_bundle();
    bundle.entries[0].body_sha256 = "sha256:not-hex".to_string();

    let error = bundle.validate().expect_err("malformed digest must fail");

    assert!(error.to_string().contains("body_sha256"));
}

#[test]
fn legacy_scriptlet_bundle_rejects_tampered_body_hash() {
    let mut bundle = sample_bundle();
    bundle.entries[0].body.push_str("echo tampered\n");

    let error = bundle.validate().expect_err("tampered body must fail");

    assert!(error.to_string().contains("body_sha256 mismatch"));
}

#[test]
fn legacy_scriptlet_bundle_validates_base64_body_hash() {
    let mut bundle = sample_bundle();
    let body_bytes = b"\xff\x00native bytes\n";
    use base64::Engine as _;
    bundle.entries[0].body = base64::engine::general_purpose::STANDARD.encode(body_bytes);
    bundle.entries[0].body_encoding = Some("base64".to_string());
    bundle.entries[0].body_sha256 = crate::hash::sha256_prefixed(body_bytes);

    bundle.validate().expect("base64 body hash validates");
}

#[test]
fn legacy_scriptlet_entry_body_bytes_returns_utf8_body() {
    let entry = sample_entry("rpm:%post", ScriptletDecision::Legacy, "echo hello\n");

    assert_eq!(
        entry.body_bytes().expect("body bytes"),
        b"echo hello\n".to_vec()
    );
}

#[test]
fn legacy_scriptlet_entry_body_bytes_decodes_base64_body() {
    let mut entry = sample_entry("rpm:%post", ScriptletDecision::Legacy, "");
    let body_bytes = b"\xff\x00native bytes\n";
    use base64::Engine as _;
    entry.body = base64::engine::general_purpose::STANDARD.encode(body_bytes);
    entry.body_encoding = Some("base64".to_string());
    entry.body_sha256 = crate::hash::sha256_prefixed(body_bytes);

    assert_eq!(entry.body_bytes().expect("body bytes"), body_bytes);
}

#[test]
fn legacy_scriptlet_entry_body_bytes_rejects_hash_mismatch() {
    let mut entry = sample_entry("rpm:%post", ScriptletDecision::Legacy, "echo hello\n");
    entry.body_sha256 =
        "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();

    let error = entry.body_bytes().expect_err("hash mismatch must fail");

    assert!(error.to_string().contains("body_sha256 mismatch"));
}

#[test]
fn legacy_scriptlet_entry_body_bytes_rejects_unknown_encoding() {
    let mut entry = sample_entry("rpm:%post", ScriptletDecision::Legacy, "echo hello\n");
    entry.body_encoding = Some("rot13".to_string());

    let error = entry.body_bytes().expect_err("unknown encoding must fail");

    assert!(error.to_string().contains("body_encoding"));
}

#[test]
fn legacy_scriptlet_bundle_rejects_malformed_allowed_target() {
    let mut bundle = sample_bundle();
    bundle.allowed_targets = vec!["rpm/fedora/44".to_string()];

    let error = bundle.validate().expect_err("malformed target must fail");

    assert!(error.to_string().contains("allowed target"));
}

#[test]
fn legacy_scriptlet_bundle_rejects_misaligned_command_provenance() {
    let mut bundle = sample_bundle();
    bundle.entries[0]
        .unknown_command_evidence
        .push(UnknownCommandEvidence {
            command: "custom-helper".to_string(),
            argv: vec!["--do-it".to_string()],
            argument_provenance: Vec::new(),
            source: CommandEvidenceSource::ShellAst,
            ..UnknownCommandEvidence::default()
        });

    let error = bundle
        .validate()
        .expect_err("argv without matching provenance must fail");

    assert!(
        error
            .to_string()
            .contains("argv/provenance length mismatch")
    );
}
