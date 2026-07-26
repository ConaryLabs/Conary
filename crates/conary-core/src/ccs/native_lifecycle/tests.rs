// conary-core/src/ccs/native_lifecycle/tests.rs

use super::*;

fn sha256_prefixed(body: &str) -> String {
    crate::hash::sha256_prefixed(body.as_bytes())
}

fn sample_entry(id: &str, body: &str) -> NativeLifecycleEntry {
    NativeLifecycleEntry {
        id: id.to_string(),
        native_slot: "%post".to_string(),
        kind: NativeLifecycleEntryKind::Executable,
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
        },
        transaction_order: TransactionOrder {
            position: "after-payload".to_string(),
            before: vec![],
            after: vec!["payload".to_string()],
        },
        timeout_ms: 30_000,
        sandbox: Some(ScriptletSandboxRequirements {
            network: false,
            namespaces: vec!["mount".to_string(), "pid".to_string()],
            seccomp_profile: Some("native-lifecycle/default".to_string()),
        }),
        capabilities: vec!["ldconfig".to_string()],
        evidence_digest: Some(
            "sha256:2222222222222222222222222222222222222222222222222222222222222222".to_string(),
        ),
        source_evidence_refs: vec!["capture:rpm:%post".to_string()],
        rpm_trigger: None,
        rpm_runtime: Some(RpmRuntimeMetadata {
            program: RpmProgram::External,
            body_transforms: Vec::new(),
            critical: false,
            criticality: RpmCriticality::WarningOnly,
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
    }
}

fn sample_bundle() -> NativeLifecycleBundle {
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora-rhel".to_string(),
        source_profile: Some("fedora-44".to_string()),
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
        conversion_policy: "native-lifecycle-v2".to_string(),
        evidence_digest: Some(
            "sha256:6666666666666666666666666666666666666666666666666666666666666666".to_string(),
        ),
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![
            sample_entry("rpm:%preun", "ldconfig\n"),
            sample_entry("rpm:%post", "systemctl daemon-reload\n"),
        ],
    }
}

#[test]
fn alpm_hook_schema_requires_an_action_and_rejects_compatibility_fields() {
    let missing_action = r#"
hook_path = "/usr/share/libalpm/hooks/40-cache.hook"
triggers = []
"#;
    assert!(toml::from_str::<ArchHookMetadata>(missing_action).is_err());

    let compatibility_field = r#"
hook_path = "/usr/share/libalpm/hooks/40-cache.hook"
triggers = []
directory_priority = 1

[action]
when = "post-transaction"
exec = "/bin/true"
argv = ["/bin/true"]
"#;
    assert!(toml::from_str::<ArchHookMetadata>(compatibility_field).is_err());

    let mut hook = ArchHookMetadata {
        hook_path: "/usr/share/libalpm/hooks/40-cache.hook".to_string(),
        triggers: vec![ArchHookTriggerMetadata {
            operations: vec![ArchHookOperation::Install],
            trigger_type: ArchHookTriggerType::Path,
            targets: vec!["usr/share/cache/[[:digit:]]*".to_string()],
        }],
        action: ArchHookActionMetadata {
            description: None,
            when: ArchHookWhen::PostTransaction,
            exec: r#"/usr/bin/cache "two words""#.to_string(),
            argv: vec!["/usr/bin/cache".to_string(), "two words".to_string()],
            depends: Vec::new(),
            abort_on_fail: false,
            needs_targets: false,
        },
    };
    hook.validate("arch:hook").unwrap();
    hook.action.argv.push("drift".to_string());
    assert!(
        hook.validate("arch:hook")
            .unwrap_err()
            .to_string()
            .contains("wordsplit projection")
    );
}

#[test]
fn native_lifecycle_bundle_round_trips_core_fields() {
    let bundle = sample_bundle();

    let encoded = toml::to_string_pretty(&bundle).expect("serialize bundle");
    let decoded: NativeLifecycleBundle = toml::from_str(&encoded).expect("parse bundle");

    assert_eq!(decoded.schema, NATIVE_LIFECYCLE_SCHEMA_V1);
    assert_eq!(decoded.source_format, SourceFormat::Rpm);
    assert_eq!(decoded.entries.len(), 2);
    assert_eq!(decoded.entries[0].body, "ldconfig\n");
}

#[test]
fn native_lifecycle_bundle_round_trips_reserved_metadata() {
    let mut bundle = sample_bundle();
    let entry = bundle.entries.first_mut().expect("fixture entry");
    entry.rpm_trigger = Some(RpmTriggerMetadata {
        kind: RpmTriggerKind::File,
        target_constraints: vec![RpmTriggerTargetConstraint {
            package: "systemd".to_string(),
            action: RpmTriggerAction::Install,
            operator: Some(">=".to_string()),
            version: Some("255".to_string()),
            raw_flags: 0,
        }],
        priority: Some(100),
        path_prefixes: vec!["/usr/lib/systemd/system".to_string()],
    });
    entry.residual_lifecycle = Some(ResidualLifecycleMetadata {
        superseded_effect_kinds: vec!["ldconfig".to_string()],
        wrapper_strategy: Some("source-and-suppress".to_string()),
        suppression_markers: vec!["CONARY_SUPPRESS_LDCONFIG=1".to_string()],
        residual_body_digest: Some(
            "sha256:9999999999999999999999999999999999999999999999999999999999999999".to_string(),
        ),
    });

    let encoded = toml::to_string_pretty(&bundle).expect("serialize bundle");
    let decoded: NativeLifecycleBundle = toml::from_str(&encoded).expect("parse bundle");
    let decoded_entry = decoded.entries.first().expect("round-tripped entry");

    assert_eq!(
        decoded_entry
            .rpm_trigger
            .as_ref()
            .expect("rpm trigger")
            .path_prefixes,
        vec!["/usr/lib/systemd/system"]
    );
    assert_eq!(
        decoded_entry
            .residual_lifecycle
            .as_ref()
            .expect("residual")
            .superseded_effect_kinds,
        vec!["ldconfig"]
    );
}

#[test]
fn native_lifecycle_bundle_rejects_unknown_persisted_fields() {
    let bundle = sample_bundle();

    let mut top_level = toml::Value::try_from(&bundle).expect("serialize bundle");
    top_level
        .as_table_mut()
        .unwrap()
        .insert("future_top_level".to_string(), toml::Value::Boolean(true));
    assert!(top_level.try_into::<NativeLifecycleBundle>().is_err());

    let mut entry = toml::Value::try_from(&bundle).expect("serialize bundle");
    entry["entries"][0]
        .as_table_mut()
        .unwrap()
        .insert("future_entry_field".to_string(), toml::Value::Boolean(true));
    assert!(entry.try_into::<NativeLifecycleBundle>().is_err());
}

#[test]
fn deb_trigger_contract_rejects_stale_projections_and_raw_line_drift() {
    let body = b"interest-noawait icon-cache\n";
    let metadata = DebMaintainerMetadata {
        control_member: DebControlMember::Triggers,
        invocations: Vec::new(),
        trigger_declarations: vec![DebTriggerMetadata {
            directive: DebTriggerDirective::Interest,
            trigger_name: "icon-cache".to_string(),
            await_mode: DebTriggerAwaitMode::NoAwait,
            raw_line: "interest-noawait icon-cache".to_string(),
        }],
        noninteractive: false,
    };
    metadata
        .validate("deb:triggers", body)
        .expect("exact typed declaration matches its preserved control artifact");

    for stale_field in ["triggers_content", "trigger_names"] {
        let mut encoded = serde_json::to_value(&metadata).expect("serialize Debian metadata");
        encoded.as_object_mut().unwrap().insert(
            stale_field.to_string(),
            serde_json::Value::String("stale duplicate".to_string()),
        );
        assert!(
            serde_json::from_value::<DebMaintainerMetadata>(encoded).is_err(),
            "{stale_field} must not remain accepted"
        );
    }

    let mut drifted = metadata;
    drifted.trigger_declarations[0].raw_line = "activate-noawait icon-cache".to_string();
    let error = drifted
        .validate("deb:triggers", body)
        .expect_err("raw trigger evidence must match the exact parsed artifact");
    assert!(
        error
            .to_string()
            .contains("exact parsed control-artifact line"),
        "{error}"
    );
}

#[test]
fn deb_maintainer_optional_arguments_must_form_a_positional_suffix() {
    let metadata = DebMaintainerMetadata {
        control_member: DebControlMember::Preinst,
        invocations: vec![DebMaintainerInvocationMetadata {
            mode: DebMaintainerMode::Upgrade,
            arguments: vec![
                DebMaintainerArgumentMetadata {
                    index: 1,
                    name: "action".to_string(),
                    value: DebMaintainerArgumentValue::Action,
                    literal: None,
                    required: true,
                },
                DebMaintainerArgumentMetadata {
                    index: 2,
                    name: "old-version".to_string(),
                    value: DebMaintainerArgumentValue::OldVersion,
                    literal: None,
                    required: false,
                },
                DebMaintainerArgumentMetadata {
                    index: 3,
                    name: "new-version".to_string(),
                    value: DebMaintainerArgumentValue::NewVersion,
                    literal: None,
                    required: true,
                },
            ],
            lifecycle_paths: vec![LifecyclePath::PreUpgrade],
        }],
        ..DebMaintainerMetadata::default()
    };

    let error = metadata
        .validate("deb:preinst", b"exit 0\n")
        .expect_err("required arguments after optional positions are ambiguous");
    assert!(
        error
            .to_string()
            .contains("optional arguments must form a positional suffix"),
        "{error}"
    );
}

#[test]
fn native_lifecycle_bundle_accepts_only_the_current_revision() {
    let current = sample_bundle();
    current.validate().expect("current revision validates");

    for revision in [NATIVE_LIFECYCLE_SCHEMA_REVISION - 1, u16::MAX] {
        let mut stale = current.clone();
        stale.schema_revision = revision;
        let error = stale
            .validate()
            .expect_err("non-current revision must fail");
        assert!(
            error.to_string().contains(&format!(
                "schema_revision must be {NATIVE_LIFECYCLE_SCHEMA_REVISION}"
            )),
            "{error}"
        );
    }
}

#[test]
fn native_lifecycle_closed_enums_reject_unknown_values() {
    let encoded = toml::to_string_pretty(&sample_bundle()).expect("serialize current bundle");
    let unknown_source = encoded.replacen("source_format = \"rpm\"", "source_format = \"apk\"", 1);
    let error = toml::from_str::<NativeLifecycleBundle>(&unknown_source)
        .expect_err("unknown source format must fail during deserialization");
    assert!(error.to_string().contains("unknown variant"), "{error}");

    let unknown_entry_kind =
        encoded.replacen("kind = \"executable\"", "kind = \"future-entry-kind\"", 1);
    let error = toml::from_str::<NativeLifecycleBundle>(&unknown_entry_kind)
        .expect_err("unknown entry kind must fail during deserialization");
    assert!(error.to_string().contains("unknown variant"), "{error}");

    let unknown_phase = encoded.replacen("phase = \"post-install\"", "phase = \"future-phase\"", 1);
    let error = toml::from_str::<NativeLifecycleBundle>(&unknown_phase)
        .expect_err("unknown lifecycle phase must fail during deserialization");
    assert!(error.to_string().contains("unknown variant"), "{error}");
}

#[test]
fn native_lifecycle_bundle_accepts_zero_entry_native_free_package() {
    let mut bundle = sample_bundle();
    bundle.entries.clear();
    bundle.scriptlet_fidelity = ScriptletFidelity::NativeFree;

    let encoded = toml::to_string_pretty(&bundle).expect("serialize bundle");
    let decoded: NativeLifecycleBundle = toml::from_str(&encoded).expect("parse bundle");

    assert!(decoded.entries.is_empty());
    assert_eq!(decoded.scriptlet_fidelity, ScriptletFidelity::NativeFree);
}

#[test]
fn native_lifecycle_bundle_rejects_duplicate_entry_ids() {
    let mut bundle = sample_bundle();
    bundle.entries[1].id = bundle.entries[0].id.clone();

    let error = bundle.validate().expect_err("duplicate IDs must fail");

    assert!(error.to_string().contains("duplicate entry id"));
}

#[test]
fn native_lifecycle_bundle_rejects_mismatched_version_scheme() {
    let mut bundle = sample_bundle();
    bundle.version_scheme = VersionScheme::Deb;

    let error = bundle
        .validate()
        .expect_err("source format and version scheme must agree");

    assert!(
        error.to_string().contains("requires version_scheme 'rpm'"),
        "unexpected error: {error}"
    );
}

#[test]
fn native_lifecycle_bundle_rejects_foreign_format_metadata() {
    let mut bundle = sample_bundle();
    let entry = bundle.entries.first_mut().expect("fixture entry");
    entry.arch_install = Some(ArchInstallMetadata {
        install_digest: entry.body_sha256.clone(),
        called_function: "post_install".to_string(),
        selection: ArchInstallSelection::LibalpmGrepV1,
    });

    let error = bundle
        .validate()
        .expect_err("one source ABI cannot carry another format's contract");

    assert!(
        error.to_string().contains("foreign format metadata"),
        "unexpected error: {error}"
    );
}

#[test]
fn native_lifecycle_bundle_rejects_missing_format_runtime() {
    let mut bundle = sample_bundle();
    bundle.entries[0].rpm_runtime = None;

    let error = bundle
        .validate()
        .expect_err("RPM lifecycle entries require typed RPM runtime metadata");

    assert!(
        error.to_string().contains("no typed RPM runtime contract"),
        "unexpected error: {error}"
    );
}

#[test]
fn native_lifecycle_bundle_rejects_zero_timeout() {
    let mut bundle = sample_bundle();
    bundle.entries[0].timeout_ms = 0;

    let error = bundle.validate().expect_err("zero timeout must fail");

    assert!(error.to_string().contains("timeout_ms"));
}

#[test]
fn native_lifecycle_bundle_rejects_malformed_sha256_digest() {
    let mut bundle = sample_bundle();
    bundle.entries[0].body_sha256 = "sha256:not-hex".to_string();

    let error = bundle.validate().expect_err("malformed digest must fail");

    assert!(error.to_string().contains("body_sha256"));
}

#[test]
fn native_lifecycle_bundle_rejects_tampered_body_hash() {
    let mut bundle = sample_bundle();
    bundle.entries[0].body.push_str("echo tampered\n");

    let error = bundle.validate().expect_err("tampered body must fail");

    assert!(error.to_string().contains("body_sha256 mismatch"));
}

#[test]
fn native_lifecycle_bundle_validates_base64_body_hash() {
    let mut bundle = sample_bundle();
    let body_bytes = b"\xff\x00native bytes\n";
    use base64::Engine as _;
    bundle.entries[0].body = base64::engine::general_purpose::STANDARD.encode(body_bytes);
    bundle.entries[0].body_encoding = Some("base64".to_string());
    bundle.entries[0].body_sha256 = crate::hash::sha256_prefixed(body_bytes);

    bundle.validate().expect("base64 body hash validates");
}

#[test]
fn native_lifecycle_entry_body_bytes_returns_utf8_body() {
    let entry = sample_entry("rpm:%post", "echo hello\n");

    assert_eq!(
        entry.body_bytes().expect("body bytes"),
        b"echo hello\n".to_vec()
    );
}

#[test]
fn native_lifecycle_entry_body_bytes_decodes_base64_body() {
    let mut entry = sample_entry("rpm:%post", "");
    let body_bytes = b"\xff\x00native bytes\n";
    use base64::Engine as _;
    entry.body = base64::engine::general_purpose::STANDARD.encode(body_bytes);
    entry.body_encoding = Some("base64".to_string());
    entry.body_sha256 = crate::hash::sha256_prefixed(body_bytes);

    assert_eq!(entry.body_bytes().expect("body bytes"), body_bytes);
}

#[test]
fn native_lifecycle_entry_body_bytes_rejects_hash_mismatch() {
    let mut entry = sample_entry("rpm:%post", "echo hello\n");
    entry.body_sha256 =
        "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();

    let error = entry.body_bytes().expect_err("hash mismatch must fail");

    assert!(error.to_string().contains("body_sha256 mismatch"));
}

#[test]
fn native_lifecycle_entry_body_bytes_rejects_unknown_encoding() {
    let mut entry = sample_entry("rpm:%post", "echo hello\n");
    entry.body_encoding = Some("rot13".to_string());

    let error = entry.body_bytes().expect_err("unknown encoding must fail");

    assert!(error.to_string().contains("body_encoding"));
}
