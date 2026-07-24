// conary-core/src/ccs/convert/adapters/tests/registry.rs

use super::*;

#[test]
fn adapter_registry_classifies_safe_helpers_with_complete_replacement() {
    let registry = AdapterRegistry::default();

    let classification = registry.classify_invocation(&invocation("ldconfig", &[]));

    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = classification
    else {
        panic!("ldconfig should be known");
    };
    assert_eq!(reason_code, "helper-complete-ldconfig");
    assert_eq!(effects[0].adapter_id.as_deref(), Some("ldconfig/v2"));
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
}

#[test]
fn blocked_boot_security_classes_carry_command_evidence() {
    let registry = AdapterRegistry::default();

    for (command, args, class_id, expected_argv) in [
        ("depmod", vec!["6.10.0"], "kernel-module", vec!["<kver>"]),
        (
            "kernel-install",
            vec!["add", "6.10.0", "/lib/modules/6.10.0/vmlinuz"],
            "kernel-module",
            vec!["add", "<kver>", "/lib/modules/<kver>/vmlinuz"],
        ),
        (
            "dracut",
            vec!["--force", "/boot/6.10.0/initramfs.img"],
            "initramfs",
            vec!["--force", "<boot>/<kver>/initramfs.img"],
        ),
        (
            "restorecon",
            vec!["-R", "/usr/lib/modules"],
            "selinux",
            vec!["-R", "<path>"],
        ),
    ] {
        let classification = registry.classify_invocation(&invocation(command, &args));
        match classification {
            ScriptletClassification::Blocked {
                class_id: actual_class,
                command: Some(evidence),
                ..
            } => {
                assert_eq!(actual_class, class_id);
                assert_eq!(evidence.command, command);
                assert_eq!(evidence.argv, expected_argv);
                assert_eq!(evidence.source.as_str(), "shell-ast");
                assert!(evidence.environment.is_empty());
            }
            other => panic!("expected blocked evidence for {command}, got {other:?}"),
        }
    }
}

#[test]
fn adapter_registry_lets_blocked_class_win_before_adapter_matching() {
    let registry = AdapterRegistry::default();

    let classification =
        registry.classify_invocation(&invocation("curl", &["https://example.invalid"]));

    assert!(matches!(
        classification,
        ScriptletClassification::Blocked {
            reason_code,
            class_id,
            ..
        }
            if reason_code == "blocked-class-network" && class_id == "network"
    ));
}

#[test]
fn adapter_registry_reports_typed_unknown_command_evidence() {
    let registry = AdapterRegistry::default();

    let classification = registry.classify_invocation(&invocation("custom-helper", &["--do-it"]));

    assert!(matches!(
        classification,
        ScriptletClassification::Unknown { reason_code, command }
            if reason_code == "unknown-command"
                && command.command == "custom-helper"
                && command.argv == ["--do-it"]
    ));
}

#[test]
fn adapter_registry_has_stable_builtin_order_and_unique_ids() {
    let registry = AdapterRegistry::default();
    let ids = registry.adapter_ids();

    assert_eq!(
        ids,
        vec![
            "native-free/v1",
            "ldconfig/v2",
            "systemd-daemon-reload/v2",
            "systemd-unit-state/v1",
            "deb-systemd-helper/v1",
            "dpkg-maintscript-helper/v1",
            "systemd-tmpfiles-create/v1",
            "systemd-sysusers/v1",
            "sysctl/v1",
            "setuid-mode/v1",
            "file-capability/v1",
            "selinux-policy/v1",
            "apparmor-policy/v1",
            "alternatives-registration/v1",
            "cache-refresh/v1",
        ]
    );

    let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len());

    let native_free = registry
        .adapters_for_testing()
        .into_iter()
        .find(|adapter| adapter.id() == "native-free/v1")
        .expect("native-free adapter present");
    let payload = PayloadHints::default();
    let command = invocation("true", &[]);
    assert!(!native_free.matches(AdapterInput {
        invocation: &command,
        payload: &payload,
    }));
}

#[test]
fn adapter_registry_golden_helpers_are_fully_replaced_with_adapter_evidence() {
    let registry = AdapterRegistry::default();
    let payload = golden_adapter_payload();
    let cases = [
        GoldenAdapterCase {
            fixture_id: "adapter-sysusers",
            command: "systemd-sysusers",
            argv: &["/usr/lib/sysusers.d/demo.conf"],
            adapter_id: "systemd-sysusers/v1",
            reason_code: "helper-complete-sysusers",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-sysctl",
            command: "sysctl",
            argv: &["-w", "kernel.example=1"],
            adapter_id: "sysctl/v1",
            reason_code: "helper-complete-sysctl",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-sysctl-target-profile-private-review",
            command: "sysctl",
            argv: &["-w", "net.ipv4.ip_forward=1"],
            adapter_id: "sysctl/v1",
            reason_code: "helper-complete-sysctl",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-setuid-mode",
            command: "chmod",
            argv: &["u+s", "/usr/bin/demo"],
            adapter_id: "setuid-mode/v1",
            reason_code: "helper-complete-setuid-mode",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-file-capability",
            command: "setcap",
            argv: &["cap_net_bind_service=+ep", "/usr/bin/demo"],
            adapter_id: "file-capability/v1",
            reason_code: "helper-complete-file-capability",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-file-capability-high-risk",
            command: "setcap",
            argv: &["cap_sys_admin=+ep", "/usr/bin/demo"],
            adapter_id: "file-capability/v1",
            reason_code: "helper-complete-file-capability",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-registry-systemd-daemon-reload",
            command: "systemctl",
            argv: &["daemon-reload"],
            adapter_id: "systemd-daemon-reload/v2",
            reason_code: "helper-complete-systemd-daemon-reload",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-registry-systemd-unit-state",
            command: "systemctl",
            argv: &["enable", "demo.service"],
            adapter_id: "systemd-unit-state/v1",
            reason_code: "helper-complete-systemd-unit-state",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-deb-systemd-helper-unit-state",
            command: "deb-systemd-helper",
            argv: &["enable", "demo.service"],
            adapter_id: "deb-systemd-helper/v1",
            reason_code: "helper-complete-deb-systemd-helper-unit-state",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-tmpfiles-create",
            command: "systemd-tmpfiles",
            argv: &["--create", "/usr/lib/tmpfiles.d/demo.conf"],
            adapter_id: "systemd-tmpfiles-create/v1",
            reason_code: "helper-complete-tmpfiles-create",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-cache-refresh",
            command: "update-mime-database",
            argv: &["/usr/share/mime"],
            adapter_id: "cache-refresh/v1",
            reason_code: "helper-complete-cache-refresh",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-selinux-policy",
            command: "restorecon",
            argv: &["-R", "/usr/bin/demo"],
            adapter_id: "selinux-policy/v1",
            reason_code: "helper-complete-selinux-policy",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-apparmor-policy",
            command: "apparmor_parser",
            argv: &["-r", "/etc/apparmor.d/usr.bin.demo"],
            adapter_id: "apparmor-policy/v1",
            reason_code: "helper-complete-apparmor-policy",
        },
        GoldenAdapterCase {
            fixture_id: "adapter-alternatives-registration",
            command: "update-alternatives",
            argv: &[
                "--install",
                "/usr/bin/editor",
                "editor",
                "/usr/bin/demo-editor",
                "50",
            ],
            adapter_id: "alternatives-registration/v1",
            reason_code: "helper-complete-alternatives-registration",
        },
    ];

    for case in cases {
        let invocation = invocation(case.command, case.argv);
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation,
            payload: &payload,
        });

        assert_complete_adapter_evidence(
            case.fixture_id,
            classification,
            case.adapter_id,
            case.reason_code,
        );
    }
}

struct GoldenAdapterCase {
    fixture_id: &'static str,
    command: &'static str,
    argv: &'static [&'static str],
    adapter_id: &'static str,
    reason_code: &'static str,
}

fn golden_adapter_payload() -> PayloadHints {
    let mut payload = PayloadHints::default();
    payload.payload_paths.insert("/usr/bin/demo".to_string());
    payload
        .file_modes
        .insert("/usr/bin/demo".to_string(), 0o755);
    payload.executable_paths.insert("/usr/bin/demo".to_string());
    payload
        .payload_paths
        .insert("/usr/share/selinux/packages/demo.pp".to_string());
    payload
        .payload_paths
        .insert("/etc/apparmor.d/usr.bin.demo".to_string());
    payload.systemd_units.insert("demo.service".to_string());
    payload
        .tmpfiles_configs
        .insert("/usr/lib/tmpfiles.d/demo.conf".to_string());
    payload
        .sysusers_configs
        .insert("/usr/lib/sysusers.d/demo.conf".to_string());
    payload
        .cache_inputs
        .entry("mime-db".to_string())
        .or_default()
        .insert("/usr/share/mime/packages/demo.xml".to_string());
    payload
}

fn assert_complete_adapter_evidence(
    fixture_id: &str,
    classification: ScriptletClassification,
    adapter_id: &str,
    reason_code: &str,
) {
    let ScriptletClassification::Known {
        reason_code: actual_reason,
        effects,
    } = classification
    else {
        panic!("{fixture_id} should classify as known adapter evidence");
    };

    assert_eq!(actual_reason, reason_code, "{fixture_id} reason code");
    assert_eq!(
        effects[0].adapter_id.as_deref(),
        Some(adapter_id),
        "{fixture_id} adapter id"
    );
    assert_eq!(
        effects[0].replacement,
        EffectReplacement::Complete,
        "{fixture_id} replacement"
    );
}
