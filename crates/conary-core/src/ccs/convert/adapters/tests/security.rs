// conary-core/src/ccs/convert/adapters/tests/security.rs

use super::*;

#[test]
fn selinux_adapter_models_payload_backed_policy_and_label_intent_as_portable_effects() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::from_files(&[
        file("/usr/bin/demo"),
        file("/usr/share/selinux/packages/demo.pp"),
    ]);

    for (command, argv, kind, path, operation) in [
        (
            "restorecon",
            vec!["-R", "/usr/bin/demo"],
            "selinux-label-refresh",
            "/usr/bin/demo",
            "label-refresh",
        ),
        (
            "semanage",
            vec!["fcontext", "-a", "-t", "demo_exec_t", "/usr/bin/demo"],
            "selinux-file-context",
            "/usr/bin/demo",
            "file-context-add",
        ),
        (
            "setsebool",
            vec!["-P", "demo_can_network", "on"],
            "selinux-boolean",
            "demo_can_network",
            "boolean-set",
        ),
        (
            "semodule",
            vec!["-i", "/usr/share/selinux/packages/demo.pp"],
            "selinux-policy-module",
            "/usr/share/selinux/packages/demo.pp",
            "module-install",
        ),
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation(command, &argv),
            payload: &payload,
        });

        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = classification
        else {
            panic!("{command} should be modeled as SELinux policy intent");
        };

        assert_eq!(reason_code, "helper-complete-selinux-policy");
        assert_eq!(effects.len(), 1);
        let effect = &effects[0];
        assert_eq!(effect.kind, kind);
        assert_eq!(effect.adapter_id.as_deref(), Some("selinux-policy/v1"));
        assert_eq!(effect.replacement, EffectReplacement::Complete);
        assert_eq!(effect.path.as_deref(), Some(path));
        assert_eq!(extra_str(effect, "selinux_operation"), Some(operation));
        assert_eq!(
            extra_str(effect, "target_security_policy"),
            Some("selinux-optional")
        );
        assert_eq!(
            extra_str(effect, "host_policy_behavior"),
            Some("apply-when-selinux-present-dormant-when-absent")
        );
    }
}

#[test]
fn selinux_adapter_leaves_broad_or_unbacked_mutation_blocked() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::from_files(&[file("/usr/bin/demo")]);

    for (command, argv) in [
        ("restorecon", vec!["-R", "/"]),
        ("restorecon", vec!["-Rv", "/usr"]),
        ("semodule", vec!["-i", "/tmp/demo.pp"]),
        ("semodule", vec!["-r", "demo"]),
        ("semanage", vec!["permissive", "-a", "demo_t"]),
        ("setsebool", vec!["demo_can_network", "on"]),
        ("fixfiles", vec!["restore"]),
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation(command, &argv),
            payload: &payload,
        });

        assert!(matches!(
            classification,
            ScriptletClassification::Blocked {
                reason_code,
                class_id,
                command: Some(_),
            } if reason_code == "blocked-class-selinux" && class_id == "selinux"
        ));
    }
}

#[test]
fn apparmor_adapter_models_payload_backed_profile_reload_as_portable_effect() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::from_files(&[file("/etc/apparmor.d/usr.bin.demo")]);

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("apparmor_parser", &["-r", "/etc/apparmor.d/usr.bin.demo"]),
        payload: &payload,
    });

    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = classification
    else {
        panic!("payload-backed AppArmor profile reload should be modeled as policy intent");
    };

    assert_eq!(reason_code, "helper-complete-apparmor-policy");
    assert_eq!(effects.len(), 1);
    let effect = &effects[0];
    assert_eq!(effect.kind, "apparmor-profile-reload");
    assert_eq!(effect.adapter_id.as_deref(), Some("apparmor-policy/v1"));
    assert_eq!(effect.replacement, EffectReplacement::Complete);
    assert_eq!(effect.path.as_deref(), Some("/etc/apparmor.d/usr.bin.demo"));
    assert_eq!(
        extra_str(effect, "apparmor_operation"),
        Some("profile-reload")
    );
    assert_eq!(
        extra_str(effect, "profile_path"),
        Some("/etc/apparmor.d/usr.bin.demo")
    );
    assert_eq!(extra_str(effect, "profile_name"), Some("usr.bin.demo"));
    assert_eq!(extra_bool(effect, "payload_backed"), Some(true));
    assert_eq!(
        extra_string_array(effect, "paths"),
        vec!["/etc/apparmor.d/usr.bin.demo"]
    );
}

#[test]
fn apparmor_adapter_leaves_broad_or_unbacked_profile_mutation_blocked() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::from_files(&[file("/etc/apparmor.d/usr.bin.demo")]);

    for (command, argv) in [
        ("apparmor_parser", vec!["-r", "/etc/apparmor.d"]),
        ("apparmor_parser", vec!["-r", "/tmp/usr.bin.demo"]),
        (
            "apparmor_parser",
            vec!["-R", "/etc/apparmor.d/usr.bin.demo"],
        ),
        (
            "apparmor_parser",
            vec![
                "--replace",
                "/etc/apparmor.d/usr.bin.demo",
                "/etc/apparmor.d/usr.bin.other",
            ],
        ),
        (
            "apparmor_parser",
            vec!["--replace", "/etc/apparmor.d/subdir/usr.bin.demo"],
        ),
        ("aa-enforce", vec!["/etc/apparmor.d/usr.bin.demo"]),
        ("aa-complain", vec!["/etc/apparmor.d/usr.bin.demo"]),
        ("aa-disable", vec!["/etc/apparmor.d/usr.bin.demo"]),
        ("aa-status", vec![]),
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation(command, &argv),
            payload: &payload,
        });

        assert!(matches!(
            classification,
            ScriptletClassification::Blocked {
                reason_code,
                class_id,
                command: Some(_),
            } if reason_code == "blocked-class-apparmor" && class_id == "apparmor"
        ));
    }
}

#[test]
fn sysctl_adapter_models_safe_write_as_complete_native_intent() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("sysctl", &["-w", "net.ipv4.ip_forward=1"]),
        payload: &payload,
    });

    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = classification
    else {
        panic!("safe sysctl write should be modeled as native intent");
    };
    assert_eq!(reason_code, "helper-complete-sysctl");
    assert_eq!(effects.len(), 1);
    let effect = &effects[0];
    assert_eq!(effect.kind, "sysctl-setting");
    assert_eq!(effect.adapter_id.as_deref(), Some("sysctl/v1"));
    assert_eq!(effect.replacement, EffectReplacement::Complete);
    assert_eq!(effect.path.as_deref(), Some("net.ipv4.ip_forward"));
    assert_eq!(extra_str(effect, "key"), Some("net.ipv4.ip_forward"));
    assert_eq!(extra_str(effect, "value"), Some("1"));
}

#[test]
fn sysctl_adapter_leaves_broad_and_denied_forms_blocked() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    for argv in [
        vec!["-p"],
        vec!["-w", "kernel.modules_disabled=1"],
        vec!["-w", "net.ipv4.ip_forward=1", "vm.swappiness=10"],
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("sysctl", &argv),
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Blocked {
                reason_code,
                class_id,
                command: Some(_),
            } if reason_code == "blocked-class-sysctl" && class_id == "sysctl"
        ));
    }
}

#[test]
fn setuid_adapter_requires_payload_executable_and_leaves_other_privilege_forms_blocked() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload.payload_paths.insert("/usr/bin/demo".to_string());
    payload
        .file_modes
        .insert("/usr/bin/demo".to_string(), 0o755);
    payload.executable_paths.insert("/usr/bin/demo".to_string());

    for (command, argv) in [
        ("chmod", vec!["u+s", "/usr/bin/missing"]),
        ("chmod", vec!["g+s", "/usr/bin/demo"]),
        ("chmod", vec!["+s", "/usr/bin/demo"]),
        ("chmod", vec!["6755", "/usr/bin/demo"]),
        ("setpriv", vec!["--no-new-privs", "/usr/bin/demo"]),
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation(command, &argv),
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Blocked {
                reason_code,
                class_id,
                command: Some(_),
            } if reason_code == "blocked-class-setuid-setcap"
                && class_id == "setuid-setcap"
        ));
    }
}

#[test]
fn file_capability_adapter_models_supported_payload_executable_setcap() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload.payload_paths.insert("/usr/bin/demo".to_string());
    payload
        .file_modes
        .insert("/usr/bin/demo".to_string(), 0o755);
    payload.executable_paths.insert("/usr/bin/demo".to_string());

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("setcap", &["cap_net_bind_service=+ep", "/usr/bin/demo"]),
        payload: &payload,
    });

    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = classification
    else {
        panic!("supported setcap should be modeled as file capability authority");
    };
    assert_eq!(reason_code, "helper-complete-file-capability");
    assert_eq!(effects.len(), 1);
    let effect = &effects[0];
    assert_eq!(effect.kind, "file-capability");
    assert_eq!(effect.adapter_id.as_deref(), Some("file-capability/v1"));
    assert_eq!(effect.replacement, EffectReplacement::Complete);
    assert_eq!(effect.path.as_deref(), Some("/usr/bin/demo"));
    assert_eq!(
        extra_string_array(effect, "capabilities"),
        vec!["cap_net_bind_service"]
    );
    assert_eq!(extra_bool(effect, "permitted"), Some(true));
    assert_eq!(extra_bool(effect, "effective"), Some(true));
    assert_eq!(extra_bool(effect, "inheritable"), Some(false));
}

#[test]
fn file_capability_adapter_keeps_broad_unknown_and_non_payload_setcap_blocked() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload.payload_paths.insert("/usr/bin/demo".to_string());
    payload
        .file_modes
        .insert("/usr/bin/demo".to_string(), 0o755);
    payload.executable_paths.insert("/usr/bin/demo".to_string());

    for argv in [
        vec!["-r", "/usr/bin/demo"],
        vec!["cap_net_bind_service=+eip", "/usr/bin/demo"],
        vec!["cap_not_real=+ep", "/usr/bin/demo"],
        vec!["cap_net_bind_service=+ep", "/usr/bin/missing"],
        vec!["cap_net_bind_service=+ep", "/etc/demo.conf"],
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("setcap", &argv),
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Blocked {
                reason_code,
                class_id,
                command: Some(_),
            } if reason_code == "blocked-class-setuid-setcap"
                && class_id == "setuid-setcap"
        ));
    }
}
