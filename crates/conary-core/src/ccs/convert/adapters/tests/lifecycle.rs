// conary-core/src/ccs/convert/adapters/tests/lifecycle.rs

use super::*;

#[test]
fn adapter_registry_uses_payload_context_for_systemd_units() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload.systemd_units.insert("demo.service".to_string());

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["enable", "demo.service"]),
        payload: &payload,
    });

    let ScriptletClassification::Known { effects, .. } = classification else {
        panic!("systemctl enable should be known through context dispatch");
    };
    assert_eq!(effects[0].command.as_deref(), Some("systemctl"));
    assert_eq!(effects[0].args, vec!["enable", "demo.service"]);
}

#[test]
fn ldconfig_complete_only_for_simple_cache_refresh_forms() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    let complete = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("ldconfig", &[]),
        payload: &payload,
    });
    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = complete
    else {
        panic!("simple ldconfig should be known");
    };
    assert_eq!(reason_code, "helper-complete-ldconfig");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].kind, "dynamic-linker-cache");

    let review = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("ldconfig", &["-p"]),
        payload: &payload,
    });
    assert!(matches!(
        review,
        ScriptletClassification::Review {
            reason_code,
            class_id,
            ..
        }
            if reason_code == "review-class-ldconfig-nonstandard"
                && class_id.as_deref() == Some("ldconfig-nonstandard")
    ));
}

#[test]
fn systemd_daemon_reload_is_complete_but_runtime_actions_are_review() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    let reload = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["daemon-reload"]),
        payload: &payload,
    });
    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = reload
    else {
        panic!("daemon-reload should be known");
    };
    assert_eq!(reason_code, "helper-complete-systemd-daemon-reload");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);

    let system_scope = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["--system", "daemon-reload"]),
        payload: &payload,
    });
    assert!(matches!(
        system_scope,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-systemd-daemon-reload"
    ));

    let restart = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["restart", "demo.service"]),
        payload: &payload,
    });
    assert!(matches!(
        restart,
        ScriptletClassification::Review {
            reason_code,
            class_id,
            ..
        }
            if reason_code == "review-class-systemd-runtime-action"
                && class_id.as_deref() == Some("systemd-runtime-action")
    ));
}

#[test]
fn systemd_unit_state_requires_payload_evidence_for_complete() {
    let registry = AdapterRegistry::default();
    let empty_payload = PayloadHints::default();

    let partial = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["enable", "demo.service"]),
        payload: &empty_payload,
    });
    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = partial
    else {
        panic!("systemctl enable should be known");
    };
    assert_eq!(reason_code, "known-helper-partial-coverage");
    assert_eq!(effects[0].replacement, EffectReplacement::Partial);

    let mut payload = PayloadHints::default();
    payload.systemd_units.insert("demo.service".to_string());
    let complete = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["preset", "demo.service"]),
        payload: &payload,
    });
    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = complete
    else {
        panic!("systemctl preset should be known");
    };
    assert_eq!(reason_code, "helper-complete-systemd-unit-state");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].path.as_deref(), Some("demo.service"));
}

#[test]
fn deb_systemd_helper_enable_disable_are_complete_with_state_model_for_packaged_units() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload.systemd_units.insert("demo.service".to_string());

    for (action, state_model) in [
        ("enable", "first-enable-state-file"),
        ("disable", "enablement-state-update"),
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("deb-systemd-helper", &[action, "demo.service"]),
            payload: &payload,
        });

        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = classification
        else {
            panic!("deb-systemd-helper {action} should be modeled as known evidence");
        };
        assert_eq!(reason_code, "helper-complete-deb-systemd-helper-unit-state");
        assert_eq!(effects.len(), 1);
        let effect = &effects[0];
        assert_eq!(effect.adapter_id.as_deref(), Some("deb-systemd-helper/v1"));
        assert_eq!(effect.kind, "debian-systemd-helper-state");
        assert_eq!(effect.replacement, EffectReplacement::Complete);
        assert_eq!(effect.path.as_deref(), Some("demo.service"));
        assert_eq!(extra_str(effect, "debian_helper_action"), Some(action));
        assert_eq!(extra_str(effect, "state_model"), Some(state_model));
        assert_eq!(extra_bool(effect, "payload_backed"), Some(true));
        assert_eq!(extra_bool(effect, "documented_action"), Some(true));
        assert_eq!(extra_bool(effect, "maintscript_only"), Some(true));
        assert_eq!(extra_bool(effect, "dpkg_root_aware"), Some(true));
        assert_eq!(
            extra_string_array(effect, "units"),
            vec!["demo.service".to_string()]
        );
    }
}

#[test]
fn deb_systemd_helper_documented_review_actions_are_typed_partial_evidence() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload.systemd_units.insert("demo.service".to_string());

    for (action, state_model) in [
        ("purge", "state-file-purge"),
        ("mask", "mask-state-save"),
        ("unmask", "mask-state-restore"),
        ("is-enabled", "enablement-query"),
        ("was-enabled", "previous-enablement-query"),
        ("debian-installed", "state-file-presence-query"),
        ("update-state", "state-file-reconcile"),
        ("reenable", "reenable-from-recorded-state"),
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("deb-systemd-helper", &[action, "demo.service"]),
            payload: &payload,
        });

        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = classification
        else {
            panic!("deb-systemd-helper {action} should be modeled as known partial evidence");
        };
        assert_eq!(reason_code, "helper-review-deb-systemd-helper-state");
        assert_eq!(effects.len(), 1);
        let effect = &effects[0];
        assert_eq!(effect.adapter_id.as_deref(), Some("deb-systemd-helper/v1"));
        assert_eq!(effect.kind, "debian-systemd-helper-state");
        assert_eq!(effect.replacement, EffectReplacement::Partial);
        assert_eq!(effect.path.as_deref(), Some("demo.service"));
        assert_eq!(extra_str(effect, "debian_helper_action"), Some(action));
        assert_eq!(extra_str(effect, "state_model"), Some(state_model));
        assert_eq!(extra_bool(effect, "payload_backed"), Some(true));
        assert_eq!(extra_bool(effect, "documented_action"), Some(true));
        assert_eq!(extra_bool(effect, "review_required"), Some(true));
    }
}

#[test]
fn deb_systemd_helper_unbacked_documented_action_is_typed_partial_evidence() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("deb-systemd-helper", &["enable", "demo.service"]),
        payload: &payload,
    });

    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = classification
    else {
        panic!("unbacked documented deb-systemd-helper action should be typed evidence");
    };
    assert_eq!(reason_code, "helper-review-deb-systemd-helper-state");
    assert_eq!(effects[0].replacement, EffectReplacement::Partial);
    assert_eq!(extra_bool(&effects[0], "payload_backed"), Some(false));
    assert_eq!(extra_bool(&effects[0], "review_required"), Some(true));
}

#[test]
fn deb_systemd_invoke_and_undocumented_helper_forms_stay_review() {
    let registry = AdapterRegistry::default();
    let empty_payload = PayloadHints::default();
    let invoke = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("deb-systemd-invoke", &["restart", "demo.service"]),
        payload: &empty_payload,
    });
    assert!(matches!(
        invoke,
        ScriptletClassification::Review {
            reason_code,
            class_id,
            command: Some(_),
        }
            if reason_code == "review-class-deb-systemd-helper"
                && class_id.as_deref() == Some("deb-systemd-helper")
    ));

    let mut payload = PayloadHints::default();
    payload.systemd_units.insert("demo.service".to_string());
    let flagged = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("deb-systemd-helper", &["enable", "--quiet", "demo.service"]),
        payload: &payload,
    });
    assert!(matches!(
        flagged,
        ScriptletClassification::Review {
            reason_code,
            class_id,
            command: Some(_),
        }
            if reason_code == "review-class-deb-systemd-helper"
                && class_id.as_deref() == Some("deb-systemd-helper")
    ));
}

#[test]
fn dpkg_maintscript_rm_conffile_uses_exact_forwarded_argv_contract() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints {
        package_name: Some("demo".to_string()),
        ..PayloadHints::default()
    };
    let mut command = invocation(
        "dpkg-maintscript-helper",
        &[
            "rm_conffile",
            "/etc/demo.conf",
            "2.0-1~",
            "demo",
            "--",
            "\"$@\"",
        ],
    );
    *command.argument_provenance.last_mut().unwrap() = CommandArgumentProvenance::Expansion;

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &command,
        payload: &payload,
    });

    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = classification
    else {
        panic!("exact dpkg-maintscript-helper grammar should be typed");
    };
    assert_eq!(reason_code, "helper-complete-dpkg-maintscript-transition");
    assert_eq!(
        effects[0].adapter_id.as_deref(),
        Some("dpkg-maintscript-helper/v1")
    );
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(
        extra_str(&effects[0], "native_replacement_model"),
        Some("generation-etc-orphan-preservation")
    );
}

#[test]
fn dpkg_maintscript_transitions_without_native_equivalence_stay_typed_partial() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints {
        package_name: Some("demo".to_string()),
        ..PayloadHints::default()
    };
    let mut command = invocation(
        "dpkg-maintscript-helper",
        &[
            "dir_to_symlink",
            "/usr/share/demo",
            "../demo-data",
            "2.0-1~",
            "demo",
            "--",
            "\"$@\"",
        ],
    );
    *command.argument_provenance.last_mut().unwrap() = CommandArgumentProvenance::Expansion;

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &command,
        payload: &payload,
    });

    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = classification
    else {
        panic!("documented dir_to_symlink grammar should be typed");
    };
    assert_eq!(reason_code, "helper-review-dpkg-maintscript-transition");
    assert_eq!(effects[0].replacement, EffectReplacement::Partial);
    assert_eq!(
        extra_str(&effects[0], "missing_native_model"),
        Some("payload-final-state-proof")
    );
}

#[test]
fn dpkg_maintscript_requires_quoted_forwarding_and_an_exact_package_identity() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints {
        package_name: Some("demo".to_string()),
        ..PayloadHints::default()
    };

    for forwarded in ["$@", "'$@'"] {
        let mut command = invocation(
            "dpkg-maintscript-helper",
            &[
                "rm_conffile",
                "/etc/demo.conf",
                "2.0-1~",
                "demo",
                "--",
                forwarded,
            ],
        );
        *command.argument_provenance.last_mut().unwrap() = CommandArgumentProvenance::Expansion;
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &command,
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Review { .. }
        ));
    }

    let mut malformed_package = invocation(
        "dpkg-maintscript-helper",
        &[
            "rm_conffile",
            "/etc/demo.conf",
            "2.0-1~",
            "demo:amd64:unexpected",
            "--",
            "\"$@\"",
        ],
    );
    *malformed_package.argument_provenance.last_mut().unwrap() =
        CommandArgumentProvenance::Expansion;
    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &malformed_package,
        payload: &payload,
    });
    assert!(matches!(
        classification,
        ScriptletClassification::Review { .. }
    ));
}

#[test]
fn dynamic_or_conditional_helper_forms_are_discovery_only() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload.systemd_units.insert("demo.service".to_string());
    let mut command = invocation("systemctl", &["enable", "demo.service"]);
    command.argument_provenance[1] = CommandArgumentProvenance::Expansion;
    command.execution_context = CommandExecutionContext::Conditional;

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &command,
        payload: &payload,
    });

    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = classification
    else {
        panic!("dynamic helper form should remain typed discovery evidence");
    };
    assert_eq!(reason_code, "review-class-helper-form-not-authoritative");
    assert_eq!(effects[0].replacement, EffectReplacement::Partial);
    assert_eq!(extra_str(&effects[0], "authority"), Some("discovery-only"));
    assert_eq!(
        extra_str(&effects[0], "execution_context"),
        Some("conditional")
    );
}

#[test]
fn tmpfiles_create_is_complete_with_packaged_config() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload
        .tmpfiles_configs
        .insert("/usr/lib/tmpfiles.d/demo.conf".to_string());

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation(
            "systemd-tmpfiles",
            &["--create", "/usr/lib/tmpfiles.d/demo.conf"],
        ),
        payload: &payload,
    });

    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = classification
    else {
        panic!("tmpfiles create should be known");
    };
    assert_eq!(reason_code, "helper-complete-tmpfiles-create");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].kind, "tmpfiles");
}

#[test]
fn tmpfiles_remove_and_boot_are_review() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    for argv in [
        vec!["--remove"],
        vec!["--boot", "--create"],
        vec!["--create", "--boot"],
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("systemd-tmpfiles", &argv),
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Review {
                reason_code,
                class_id,
                ..
            }
                if reason_code == "review-class-tmpfiles-noncreate"
                    && class_id.as_deref() == Some("tmpfiles-noncreate")
        ));
    }
}

#[test]
fn sysusers_is_complete_with_packaged_config() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload
        .sysusers_configs
        .insert("/usr/lib/sysusers.d/demo.conf".to_string());

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemd-sysusers", &["/usr/lib/sysusers.d/demo.conf"]),
        payload: &payload,
    });

    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = classification
    else {
        panic!("sysusers should be known");
    };
    assert_eq!(reason_code, "helper-complete-sysusers");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].kind, "sysusers");
}

#[test]
fn sysusers_replace_and_root_are_review() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    for argv in [
        vec!["--replace=/usr/lib/sysusers.d/demo.conf"],
        vec!["--root=/tmp/root"],
        vec!["/usr/lib/sysusers.d/demo.conf", "--root=/tmp/root"],
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("systemd-sysusers", &argv),
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Review {
                reason_code,
                class_id,
                ..
            }
                if reason_code == "review-class-sysusers-nonstandard"
                    && class_id.as_deref() == Some("sysusers-nonstandard")
        ));
    }
}
