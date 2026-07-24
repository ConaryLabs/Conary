// conary-core/src/ccs/convert/converter/tests/policy.rs

use super::*;

#[test]
fn selinux_adapter_records_four_generic_policy_intents_in_bundle_and_summary() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "\
restorecon -R /usr/bin/test
semanage fcontext -a -t demo_exec_t /usr/bin/test
semodule -i /usr/share/selinux/packages/demo.pp
setsebool -P demo_can_network on
"
        .to_string(),
        flags: None,
    }];
    let mut files = make_test_files();
    files.push(ExtractedFile {
        path: "/usr/share/selinux/packages/demo.pp".to_string(),
        content: b"selinux policy module placeholder".to_vec(),
        size: 33,
        mode: 0o644,
        sha256: None,
        symlink_target: None,
    });
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");
    let package =
        crate::ccs::CcsPackage::parse(result.package_path.as_ref().unwrap().to_str().unwrap())
            .expect("converted CCS package should parse");
    let bundle = package
        .manifest()
        .legacy_scriptlets
        .as_ref()
        .expect("written CCS archive should carry passive scriptlet bundle");

    assert_eq!(bundle.publication_status.as_str(), "public");
    assert_eq!(bundle.security_policy_intents.len(), 4);
    assert_eq!(result.scriptlet_metadata.security_policy_intents.len(), 4);
    let label_refresh = bundle
        .security_policy_intents
        .iter()
        .find(|intent| intent.provider.as_str() == "selinux" && intent.operation == "label-refresh")
        .expect("SELinux label refresh intent should be recorded");
    assert_eq!(label_refresh.fallback.as_str(), "dormant");
    assert_eq!(label_refresh.scope.paths, vec!["/usr/bin/test"]);
    assert!(
        result
            .scriptlet_metadata
            .security_policy_intents
            .iter()
            .any(|intent| intent == label_refresh)
    );
}

#[test]
fn apparmor_profile_reload_records_public_adapter_backed_policy_intent() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "apparmor_parser -r /etc/apparmor.d/usr.bin.demo\n".to_string(),
        flags: None,
    }];
    let mut files = make_test_files();
    files.push(ExtractedFile {
        path: "/etc/apparmor.d/usr.bin.demo".to_string(),
        content: b"profile usr.bin.demo /usr/bin/demo { }\n".to_vec(),
        size: 38,
        mode: 0o644,
        sha256: None,
        symlink_target: None,
    });
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "deb", "sha256:test")
        .expect("conversion succeeds");
    let package =
        crate::ccs::CcsPackage::parse(result.package_path.as_ref().unwrap().to_str().unwrap())
            .expect("converted CCS package should parse");
    let bundle = package
        .manifest()
        .legacy_scriptlets
        .as_ref()
        .expect("written CCS archive should carry passive scriptlet bundle");

    assert_eq!(bundle.publication_status.as_str(), "public");
    assert_eq!(bundle.decision_counts.replaced, 1);
    assert_eq!(bundle.entries.len(), 1);
    let entry = &bundle.entries[0];
    assert_eq!(entry.decision.as_str(), "replaced");
    assert_eq!(entry.reason_code, "helper-complete-apparmor-policy");
    assert!(entry.blocked_classes.is_empty());
    assert_eq!(entry.effects.len(), 1);
    assert_eq!(entry.effects[0].kind, "apparmor-profile-reload");
    assert_eq!(
        entry.effects[0].adapter_id.as_deref(),
        Some("apparmor-policy/v1")
    );
    assert_eq!(entry.security_policy_intents.len(), 1);
    let intent = &entry.security_policy_intents[0];
    assert_eq!(intent.provider.as_str(), "apparmor");
    assert_eq!(intent.operation, "profile-reload");
    assert_eq!(
        intent.scope.name.as_deref(),
        Some("/etc/apparmor.d/usr.bin.demo")
    );
    assert_eq!(intent.scope.paths, vec!["/etc/apparmor.d/usr.bin.demo"]);
    assert_eq!(intent.fallback.as_str(), "dormant");
    assert_eq!(intent.reconciliation.state.as_str(), "pending");
    assert!(intent.payload_evidence.payload_backed);
    assert_eq!(bundle.security_policy_intents, vec![intent.clone()]);
    assert_eq!(result.scriptlet_metadata.publication_status, "public");
    assert_eq!(
        result.scriptlet_metadata.security_policy_intents,
        vec![intent.clone()]
    );
}

#[test]
fn apparmor_mode_helper_remains_blocked_with_review_policy_intent() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "aa-enforce /etc/apparmor.d/usr.bin.demo\n".to_string(),
        flags: None,
    }];
    let mut files = make_test_files();
    files.push(ExtractedFile {
        path: "/etc/apparmor.d/usr.bin.demo".to_string(),
        content: b"profile usr.bin.demo /usr/bin/demo { }\n".to_vec(),
        size: 38,
        mode: 0o644,
        sha256: None,
        symlink_target: None,
    });
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "deb", "sha256:test")
        .expect("conversion succeeds");
    let package =
        crate::ccs::CcsPackage::parse(result.package_path.as_ref().unwrap().to_str().unwrap())
            .expect("converted CCS package should parse");
    let bundle = package
        .manifest()
        .legacy_scriptlets
        .as_ref()
        .expect("written CCS archive should carry passive scriptlet bundle");

    assert_eq!(bundle.publication_status.as_str(), "blocked");
    assert_eq!(bundle.decision_counts.blocked, 1);
    assert_eq!(bundle.entries.len(), 1);
    let entry = &bundle.entries[0];
    assert_eq!(entry.decision.as_str(), "blocked");
    assert_eq!(entry.reason_code, "blocked-class-apparmor");
    assert_eq!(entry.blocked_classes, vec!["apparmor"]);
    assert!(entry.effects.is_empty());
    assert_eq!(entry.security_policy_intents.len(), 1);
    let intent = &entry.security_policy_intents[0];
    assert_eq!(intent.provider.as_str(), "apparmor");
    assert_eq!(intent.operation, "mode-enforce");
    assert_eq!(intent.fallback.as_str(), "block-on-enforcing-target");
    assert_eq!(intent.reconciliation.state.as_str(), "review");
    assert!(!intent.payload_evidence.payload_backed);
    assert_eq!(bundle.security_policy_intents, vec![intent.clone()]);
    assert_eq!(result.scriptlet_metadata.publication_status, "blocked");
    assert_ne!(result.scriptlet_metadata.publication_status, "public");
    assert_eq!(
        result.scriptlet_metadata.security_policy_intents,
        vec![intent.clone()]
    );
}

fn assert_blocked_scriptlet_has_no_native_authority(
    scriptlet_content: &str,
    expected_class: &str,
    expected_reason: &str,
) {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: scriptlet_content.to_string(),
        flags: None,
    }];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");
    let package =
        crate::ccs::CcsPackage::parse(result.package_path.as_ref().unwrap().to_str().unwrap())
            .expect("converted CCS package should parse");
    let bundle = package
        .manifest()
        .legacy_scriptlets
        .as_ref()
        .expect("written CCS archive should carry passive scriptlet bundle");
    let bundle_summary =
        ScriptletBundleSummary::from_bundle(bundle, bundle.evidence_digest.clone());

    assert_eq!(bundle.publication_status.as_str(), "blocked");
    assert_eq!(bundle.decision_counts.blocked, 1);
    assert_eq!(bundle.entries.len(), 1);
    let entry = &bundle.entries[0];
    assert_eq!(entry.decision.as_str(), "blocked");
    assert_eq!(entry.reason_code, expected_reason);
    assert_eq!(entry.blocked_classes, vec![expected_class]);
    assert!(entry.effects.is_empty());
    assert!(entry.boot_security_intents.is_empty());
    assert!(entry.security_policy_intents.is_empty());
    assert!(bundle_summary.boot_security_intents.is_empty());
    assert!(bundle_summary.security_policy_intents.is_empty());
    assert!(bundle.security_policy_intents.is_empty());
    assert_eq!(result.scriptlet_metadata.publication_status, "blocked");
    assert_eq!(
        result.scriptlet_metadata.blocked_classes,
        vec![expected_class.to_string()]
    );
    assert!(result.scriptlet_metadata.boot_security_intents.is_empty());
    assert!(result.scriptlet_metadata.security_policy_intents.is_empty());
    assert_ne!(result.scriptlet_metadata.publication_status, "public");
}

#[test]
fn live_fetch_and_package_manager_helpers_remain_blocked_without_manifest_authority() {
    assert_blocked_scriptlet_has_no_native_authority(
        "git -C /tmp clone https://example.invalid/repo.git\n",
        "network",
        "blocked-class-network",
    );
    assert_blocked_scriptlet_has_no_native_authority(
        "microdnf install demo\n",
        "package-manager-recursion",
        "blocked-class-package-manager-recursion",
    );
}

#[test]
fn pam_helper_remains_blocked_without_manifest_authority() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "authconfig --enablefaillock --update\n".to_string(),
        flags: None,
    }];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");
    let package =
        crate::ccs::CcsPackage::parse(result.package_path.as_ref().unwrap().to_str().unwrap())
            .expect("converted CCS package should parse");
    let bundle = package
        .manifest()
        .legacy_scriptlets
        .as_ref()
        .expect("written CCS archive should carry passive scriptlet bundle");
    let bundle_summary =
        ScriptletBundleSummary::from_bundle(bundle, bundle.evidence_digest.clone());

    assert_eq!(bundle.publication_status.as_str(), "blocked");
    assert_eq!(bundle.decision_counts.blocked, 1);
    assert_eq!(bundle.entries.len(), 1);
    let entry = &bundle.entries[0];
    assert_eq!(entry.decision.as_str(), "blocked");
    assert_eq!(entry.reason_code, "blocked-class-pam");
    assert_eq!(entry.blocked_classes, vec!["pam"]);
    assert!(entry.effects.is_empty());
    assert!(entry.boot_security_intents.is_empty());
    assert!(entry.security_policy_intents.is_empty());
    assert!(bundle_summary.boot_security_intents.is_empty());
    assert!(bundle_summary.security_policy_intents.is_empty());
    assert!(bundle.security_policy_intents.is_empty());
    assert_eq!(result.scriptlet_metadata.publication_status, "blocked");
    assert_eq!(result.scriptlet_metadata.blocked_classes, vec!["pam"]);
    assert!(result.scriptlet_metadata.boot_security_intents.is_empty());
    assert!(result.scriptlet_metadata.security_policy_intents.is_empty());
    assert_ne!(result.scriptlet_metadata.publication_status, "public");
}

#[test]
fn conversion_integration_reviews_deb_private_helpers_without_manifest_changes() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "deb-systemd-helper enable demo.service\ndebconf-communicate demo\n".to_string(),
        flags: None,
    }];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "deb", "sha256:test")
        .expect("conversion succeeds");

    let helper_effect = result
        .scriptlet_classification
        .entries
        .iter()
        .find_map(|entry| match &entry.classification {
            ScriptletClassification::Known {
                reason_code,
                effects,
            } if reason_code == "helper-review-deb-systemd-helper-state" => effects.first(),
            _ => None,
        })
        .expect("unbacked deb-systemd-helper should produce typed partial evidence");
    assert_eq!(
        helper_effect.adapter_id.as_deref(),
        Some("deb-systemd-helper/v1")
    );
    assert_eq!(helper_effect.replacement, EffectReplacement::Partial);
    assert_eq!(
        classification_extra_str(helper_effect, "debian_helper_action"),
        Some("enable")
    );
    assert_eq!(
        classification_extra_bool(helper_effect, "payload_backed"),
        Some(false)
    );
    assert!(
        !result
            .scriptlet_classification
            .entries
            .iter()
            .any(|entry| matches!(
                &entry.classification,
                ScriptletClassification::Review {
                    reason_code,
                    class_id,
                    ..
                } if reason_code == "review-class-deb-systemd-helper"
                    && class_id.as_deref() == Some("deb-systemd-helper")
            ))
    );
    assert!(
        result
            .scriptlet_classification
            .entries
            .iter()
            .any(|entry| matches!(
                &entry.classification,
                ScriptletClassification::Review {
                    reason_code,
                    class_id,
                    ..
                } if reason_code == "review-class-debconf"
                    && class_id.as_deref() == Some("debconf")
            ))
    );
    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.scriptlet_fidelity.as_str(), "review-required");
    bundle.validate().unwrap();
}

#[test]
fn conversion_integration_deb_systemd_helper_enable_records_state_model_and_is_public() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "deb-systemd-helper enable demo.service\n".to_string(),
        flags: None,
    }];
    let files = make_test_files_with_demo_unit();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "deb", "sha256:test")
        .expect("conversion succeeds");

    let complete_effect = result
        .scriptlet_classification
        .entries
        .iter()
        .find_map(|entry| match &entry.classification {
            ScriptletClassification::Known {
                reason_code,
                effects,
            } if reason_code == "helper-complete-deb-systemd-helper-unit-state" => effects.first(),
            _ => None,
        })
        .expect("payload-backed deb-systemd-helper enable should be complete evidence");
    assert_eq!(
        complete_effect.adapter_id.as_deref(),
        Some("deb-systemd-helper/v1")
    );
    assert_eq!(complete_effect.replacement, EffectReplacement::Complete);
    assert_eq!(
        classification_extra_str(complete_effect, "debian_helper_action"),
        Some("enable")
    );
    assert_eq!(
        classification_extra_str(complete_effect, "state_model"),
        Some("first-enable-state-file")
    );
    assert_eq!(
        classification_extra_bool(complete_effect, "payload_backed"),
        Some(true)
    );
    assert_eq!(result.scriptlet_classification.review_count, 0);
    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.scriptlet_fidelity.as_str(), "fully-replaced");
    assert_eq!(bundle.publication_status.as_str(), "public");
    assert_eq!(bundle.decision_counts.replaced, 1);
    assert_eq!(bundle.entries[0].effects.len(), 1);
    bundle.validate().unwrap();
}

#[test]
fn conversion_integration_dpkg_maintscript_rm_conffile_is_fully_replaced() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
            phase: ScriptletPhase::PreInstall,
            interpreter: "/bin/sh".to_string(),
            content: "dpkg-maintscript-helper rm_conffile /etc/test-package.conf 2.0-1~ test-package -- \"$@\"\n"
                .to_string(),
            flags: None,
        }];
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &make_test_files(), "deb", "sha256:test")
        .expect("conversion succeeds");

    let complete_effect = result
        .scriptlet_classification
        .entries
        .iter()
        .find_map(|entry| match &entry.classification {
            ScriptletClassification::Known {
                reason_code,
                effects,
            } if reason_code == "helper-complete-dpkg-maintscript-transition" => effects.first(),
            _ => None,
        })
        .expect("documented rm_conffile form should produce authoritative evidence");
    assert_eq!(
        complete_effect.adapter_id.as_deref(),
        Some("dpkg-maintscript-helper/v1")
    );
    assert_eq!(complete_effect.replacement, EffectReplacement::Complete);
    assert_eq!(
        classification_extra_str(complete_effect, "native_replacement_model"),
        Some("generation-etc-orphan-preservation")
    );

    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.scriptlet_fidelity.as_str(), "fully-replaced");
    assert_eq!(bundle.publication_status.as_str(), "public");
    assert_eq!(bundle.decision_counts.replaced, 1);
    assert_eq!(bundle.entries[0].effects.len(), 1);
    bundle.validate().unwrap();
}

#[test]
fn conversion_integration_deb_systemd_helper_documented_review_actions_emit_ccs_effects() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "\
deb-systemd-helper purge demo.service
deb-systemd-helper mask demo.service
deb-systemd-helper unmask demo.service
deb-systemd-helper is-enabled demo.service
deb-systemd-helper was-enabled demo.service
deb-systemd-helper debian-installed demo.service
deb-systemd-helper update-state demo.service
deb-systemd-helper reenable demo.service
"
        .to_string(),
        flags: None,
    }];
    let files = make_test_files_with_demo_unit();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "deb", "sha256:test")
        .expect("conversion succeeds");

    assert_eq!(result.scriptlet_classification.unknown_count, 0);
    assert_eq!(result.scriptlet_classification.review_count, 0);
    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.scriptlet_fidelity.as_str(), "review-required");
    assert_eq!(bundle.publication_status.as_str(), "private-review");
    assert_eq!(bundle.decision_counts.review, 1);
    assert_eq!(bundle.entries[0].effects.len(), 8);
    let actions = bundle.entries[0]
        .effects
        .iter()
        .map(|effect| {
            assert_eq!(effect.adapter_id.as_deref(), Some("deb-systemd-helper/v1"));
            assert_eq!(effect.replacement, EffectReplacement::Partial);
            assert_eq!(effect_extra_bool(effect, "review_required"), Some(true));
            effect_extra_str(effect, "debian_helper_action")
                .expect("helper effect records documented action")
                .to_string()
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        actions,
        std::collections::BTreeSet::from([
            "debian-installed".to_string(),
            "is-enabled".to_string(),
            "mask".to_string(),
            "purge".to_string(),
            "reenable".to_string(),
            "unmask".to_string(),
            "update-state".to_string(),
            "was-enabled".to_string(),
        ])
    );
    bundle.validate().unwrap();
}

#[test]
fn native_parser_support_status_is_preserved_in_classification_report() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets.clear();
    metadata.native_scriptlet_abi = vec![
        rpm_native_entry(
            "rpm:%verify",
            "%verify",
            "echo verify\n",
            RpmScriptletSlot::Verify,
            NativeLifecyclePath::Verify,
            NativeTransactionPosition::Verification,
            NativeScriptletSupport::DeferredReview {
                reason_code: "rpm-verify-scriptlet-deferred".to_string(),
            },
        ),
        rpm_native_entry(
            "rpm:broken",
            "%broken",
            "echo broken\n",
            RpmScriptletSlot::Verify,
            NativeLifecyclePath::Verify,
            NativeTransactionPosition::Verification,
            NativeScriptletSupport::Unpreservable {
                reason_code: "native-abi-parser-limitation".to_string(),
            },
        ),
    ];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert!(
        result
            .scriptlet_classification
            .entries
            .iter()
            .any(|entry| matches!(
                &entry.classification,
                crate::ccs::convert::effects::ScriptletClassification::Review {
                    reason_code,
                    class_id,
                    ..
                } if reason_code == "rpm-verify-scriptlet-deferred"
                    && class_id.as_deref() == Some("rpm-verify")
            ))
    );
    assert!(
        result
            .scriptlet_classification
            .entries
            .iter()
            .any(|entry| matches!(
                &entry.classification,
                crate::ccs::convert::effects::ScriptletClassification::Blocked {
                    reason_code,
                    class_id,
                    ..
                } if reason_code == "native-abi-parser-limitation"
                    && class_id == "native-abi-unpreservable"
            ))
    );
}

#[test]
fn parsed_native_abi_body_uses_adapter_classification_when_flattened_scriptlets_are_empty() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets.clear();
    metadata.native_scriptlet_abi = vec![rpm_native_entry(
        "rpm:%post",
        "%post",
        "/sbin/ldconfig\n",
        RpmScriptletSlot::Post,
        NativeLifecyclePath::PostInstall,
        NativeTransactionPosition::AfterPayload,
        NativeScriptletSupport::Parsed,
    )];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert!(result.scriptlet_classification.entries.iter().any(|entry| {
        entry.entry_id == "rpm:%post"
            && matches!(
                &entry.classification,
                crate::ccs::convert::effects::ScriptletClassification::Known {
                    reason_code,
                    effects,
                    ..
                } if reason_code == "helper-complete-ldconfig"
                    && effects.iter().any(|effect| {
                        effect.adapter_id.as_deref() == Some("ldconfig/v2")
                            && effect.replacement == EffectReplacement::Complete
                    })
            )
    }));
}

#[test]
fn deferred_native_trigger_with_complete_adapter_evidence_stays_private_review() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets.clear();
    metadata.native_scriptlet_abi = vec![rpm_native_entry(
        "rpm:%filetriggerin:0",
        "%filetriggerin",
        "/sbin/ldconfig\n",
        RpmScriptletSlot::Trigger,
        NativeLifecyclePath::FileTrigger,
        NativeTransactionPosition::Trigger,
        NativeScriptletSupport::DeferredReview {
            reason_code: "rpm-file-trigger-semantics-deferred".to_string(),
        },
    )];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert!(result.scriptlet_classification.entries.iter().any(|entry| {
        entry.entry_id == "rpm:%filetriggerin:0"
            && matches!(
                &entry.classification,
                crate::ccs::convert::effects::ScriptletClassification::Known {
                    reason_code,
                    effects,
                    ..
                } if reason_code == "helper-complete-ldconfig"
                    && effects.iter().any(|effect| {
                        effect.adapter_id.as_deref() == Some("ldconfig/v2")
                            && effect.replacement == EffectReplacement::Complete
                    })
            )
    }));
    assert!(
        result
            .scriptlet_classification
            .unsupported_class_counts
            .contains_key("rpm-trigger")
    );
    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.entries[0].decision.as_str(), "review");
    assert_eq!(
        bundle.entries[0].reason_code,
        "rpm-file-trigger-semantics-deferred"
    );
    assert_eq!(bundle.decision_counts.review, 1);
    assert_eq!(bundle.publication_status.as_str(), "private-review");
    assert!(
        result
            .scriptlet_metadata
            .review_reason_codes
            .contains(&"rpm-file-trigger-semantics-deferred".to_string())
    );
    assert_eq!(
        result.scriptlet_metadata.publication_status,
        "private-review"
    );
}

#[test]
fn deferred_native_trigger_with_runtime_action_stays_private_review() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets.clear();
    metadata.native_scriptlet_abi = vec![rpm_native_entry(
        "rpm:%triggerin:0",
        "%triggerin",
        "systemctl restart demo.service\n",
        RpmScriptletSlot::Trigger,
        NativeLifecyclePath::Trigger,
        NativeTransactionPosition::Trigger,
        NativeScriptletSupport::DeferredReview {
            reason_code: "rpm-trigger-semantics-deferred".to_string(),
        },
    )];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.entries[0].decision.as_str(), "review");
    assert_eq!(bundle.publication_status.as_str(), "private-review");
    assert_ne!(result.scriptlet_metadata.publication_status, "public");
    assert!(result.scriptlet_classification.entries.iter().any(|entry| {
        matches!(
            &entry.classification,
            crate::ccs::convert::effects::ScriptletClassification::Review {
                reason_code,
                class_id,
                ..
            } if reason_code == "review-class-systemd-runtime-action"
                && class_id.as_deref() == Some("systemd-runtime-action")
        )
    }));
}

#[test]
fn arch_install_function_body_uses_adapter_evidence_without_wrapper_noise() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets.clear();
    let install_source = "\
post_install() {
    /sbin/ldconfig
}
";
    metadata.native_scriptlet_abi = vec![arch_install_function_entry(
        "post_install",
        install_source,
        "/sbin/ldconfig\n",
    )];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "arch", "sha256:test")
        .expect("conversion succeeds");

    assert!(result.scriptlet_classification.entries.iter().any(|entry| {
        entry.entry_id == "arch:post_install"
            && matches!(
                &entry.classification,
                crate::ccs::convert::effects::ScriptletClassification::Known {
                    reason_code,
                    effects,
                    ..
                } if reason_code == "helper-complete-ldconfig"
                    && effects.iter().any(|effect| {
                        effect.adapter_id.as_deref() == Some("ldconfig/v2")
                            && effect.replacement == EffectReplacement::Complete
                    })
            )
    }));
    assert_eq!(result.scriptlet_classification.unknown_count, 0);
    assert_eq!(result.scriptlet_classification.review_count, 0);
    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.entries[0].decision.as_str(), "replaced");
    assert_eq!(bundle.publication_status.as_str(), "public");
    assert_eq!(result.scriptlet_metadata.publication_status, "public");
}

#[test]
fn arch_deferred_native_reason_is_preserved_with_arch_class_id() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets.clear();
    metadata.native_scriptlet_abi = vec![arch_alpm_entry(
        "arch:hook:demo",
        NativeScriptletSupport::DeferredReview {
            reason_code: "arch-alpm-hook-semantics-deferred".to_string(),
        },
    )];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "arch", "sha256:test")
        .expect("conversion succeeds");

    assert!(
        result
            .scriptlet_classification
            .entries
            .iter()
            .any(|entry| matches!(
                &entry.classification,
                crate::ccs::convert::effects::ScriptletClassification::Review {
                    reason_code,
                    class_id,
                    ..
                } if reason_code == "arch-alpm-hook-semantics-deferred"
                    && class_id.as_deref() == Some("arch-alpm-hook")
            ))
    );
}
