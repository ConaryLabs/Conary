// tests/conversion_integration/authority.rs

use super::*;

#[test]
fn golden_conversion_native_free_is_public_ready_without_entries() {
    let temp_dir = TempDir::new().unwrap();
    let converter = passive_converter(temp_dir.path());
    let metadata = create_test_metadata("adapter-registry-native-free");
    let files = create_test_files("adapter-registry-native-free");

    let result = converter
        .convert(
            &metadata,
            &files,
            "rpm",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("native-free conversion succeeds");
    let parsed = parse_converted_package(&result);
    let bundle = parsed
        .manifest()
        .legacy_scriptlets
        .as_ref()
        .expect("converted package should carry scriptlet bundle");

    assert!(bundle.entries.is_empty());
    assert_eq!(bundle.decision_counts.total(), 0);
    assert_eq!(bundle.scriptlet_fidelity, ScriptletFidelity::NativeFree);
    assert_eq!(
        bundle.target_compatibility,
        TargetCompatibility::ConaryPortable
    );
    assert_eq!(bundle.publication_status, PublicationStatus::Public);
    assert_eq!(result.scriptlet_metadata.scriptlet_fidelity, "native-free");
    assert_eq!(result.scriptlet_metadata.publication_status, "public");
}

#[test]
fn golden_conversion_adapter_backed_cases_are_fully_replaced() {
    let temp_dir = TempDir::new().unwrap();
    let converter = passive_converter(temp_dir.path());
    let mut metadata = create_test_metadata("adapter-registry-fully-replaced");
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
restorecon -R /usr/bin/adapter-registry-fully-replaced
semanage fcontext -a -t demo_exec_t /usr/bin/adapter-registry-fully-replaced
semodule -i /usr/share/selinux/packages/demo.pp
setsebool -P demo_can_network on
apparmor_parser -r /etc/apparmor.d/usr.bin.demo
update-alternatives --install /usr/bin/editor editor /usr/bin/demo-editor 50
"
        .to_string(),
        flags: None,
    }];
    let files = golden_payload_files("adapter-registry-fully-replaced");

    let result = converter
        .convert(
            &metadata,
            &files,
            "rpm",
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .expect("adapter-backed conversion succeeds");
    let parsed = parse_converted_package(&result);
    let bundle = parsed
        .manifest()
        .legacy_scriptlets
        .as_ref()
        .expect("converted package should carry scriptlet bundle");

    assert_eq!(bundle.scriptlet_fidelity, ScriptletFidelity::FullyReplaced);
    assert_eq!(
        bundle.target_compatibility,
        TargetCompatibility::ConaryPortable
    );
    assert_eq!(bundle.publication_status, PublicationStatus::Public);
    assert_eq!(bundle.decision_counts.replaced, 1);
    assert_eq!(bundle.decision_counts.legacy, 0);
    assert!(bundle.entries.iter().any(|entry| {
        entry.effects.iter().any(|effect| {
            effect.adapter_id.is_some()
                && effect.replacement
                    == conary_core::ccs::legacy_scriptlets::EffectReplacement::Complete
        })
    }));
    assert!(bundle.entries.iter().any(|entry| {
        entry.effects.iter().any(|effect| {
            effect.adapter_id.as_deref() == Some("selinux-policy/v1")
                && effect
                    .extra
                    .get("target_security_policy")
                    .and_then(toml::Value::as_str)
                    == Some("selinux-optional")
        })
    }));
    assert!(bundle.entries.iter().any(|entry| {
        entry.effects.iter().any(|effect| {
            effect.adapter_id.as_deref() == Some("apparmor-policy/v1")
                && effect
                    .extra
                    .get("target_security_policy")
                    .and_then(toml::Value::as_str)
                    == Some("apparmor-optional")
        })
    }));
    assert!(
        !bundle
            .entries
            .iter()
            .any(|entry| entry.decision == ScriptletDecision::Legacy)
    );
    assert_eq!(
        result.scriptlet_metadata.scriptlet_fidelity,
        "fully-replaced"
    );
    assert_eq!(result.scriptlet_metadata.publication_status, "public");
}

#[test]
fn golden_conversion_unknown_same_target_requires_non_public_legacy_replay() {
    let temp_dir = TempDir::new().unwrap();
    let converter = passive_converter(temp_dir.path());
    let mut metadata = create_test_metadata("legacy-replay-unknown-shell");
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "custom-helper --do-thing\n".to_string(),
        flags: None,
    }];
    let files = create_test_files("legacy-replay-unknown-shell");

    let result = converter
        .convert(
            &metadata,
            &files,
            "rpm",
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        )
        .expect("unknown shell conversion succeeds");
    let parsed = parse_converted_package(&result);
    let bundle = parsed
        .manifest()
        .legacy_scriptlets
        .as_ref()
        .expect("converted package should carry scriptlet bundle");

    assert_eq!(bundle.scriptlet_fidelity, ScriptletFidelity::LegacyReplay);
    assert_eq!(
        bundle.target_compatibility,
        TargetCompatibility::SourceNative
    );
    assert_ne!(bundle.publication_status, PublicationStatus::Public);
    assert_eq!(bundle.decision_counts.legacy, 1);
    assert!(bundle.entries.iter().any(|entry| {
        entry.decision == ScriptletDecision::Legacy
            && entry
                .unknown_command_evidence
                .iter()
                .any(|evidence| evidence.command == "custom-helper")
    }));
    assert_eq!(
        result.scriptlet_metadata.scriptlet_fidelity,
        "legacy-replay"
    );
    assert_eq!(result.scriptlet_metadata.publication_status, "local-only");
}

#[test]
fn golden_conversion_foreign_replay_is_refused_before_mutation() {
    let temp_dir = TempDir::new().unwrap();
    let converter = passive_converter(temp_dir.path());
    let mut metadata = create_test_metadata("legacy-replay-foreign-replay-rejected");
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "custom-helper --do-thing\n".to_string(),
        flags: None,
    }];
    let files = create_test_files("legacy-replay-foreign-replay-rejected");
    let result = converter
        .convert(
            &metadata,
            &files,
            "rpm",
            "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        )
        .expect("unknown shell conversion succeeds");
    let bundle = result
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");

    let input = LegacyReplayPolicyInput {
        replay_enabled: true,
        foreign_replay_override: true,
        no_scripts: false,
        requested_sandbox_mode: SandboxMode::Always,
        host_policy: HostForeignReplayPolicy::Permissive,
        target: ReplayTarget {
            format: "deb",
            distro: "ubuntu",
            release: "26.04",
            arch: "x86_64",
        },
        compatibility_matrix: TargetCompatibilityMatrix::production_default(),
        compatibility_environment: CompatibilityPreflightEnvironment::default(),
    };

    let preflight = plan_legacy_replay(
        Some(bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &input,
    )
    .expect("plan legacy replay");

    let LegacyReplayPreflight::Refused(refusal) = preflight else {
        panic!("foreign replay request should be refused before mutation");
    };
    assert_eq!(refusal.kind, LegacyReplayRefusalKind::TargetMismatch);
    assert_eq!(refusal.kind.reason_code(), "target-mismatch");
}
