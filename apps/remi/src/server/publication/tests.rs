// apps/remi/src/server/publication/tests.rs

use super::*;
use conary_core::ccs::convert::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
use conary_core::ccs::legacy_scriptlets::{
    BootSecurityIntentEvidence, CommandArgumentProvenance, CommandEvidenceSource,
    CommandExecutionContext, UnknownCommandEvidence,
};
use conary_core::ccs::security_policy::{
    SECURITY_POLICY_INTENT_SCHEMA_V1, SecurityPolicyFallback, SecurityPolicyIntent,
    SecurityPolicyPayloadEvidence, SecurityPolicyProvider, SecurityPolicyReconciliation,
    SecurityPolicyReconciliationState, SecurityPolicyRequirements, SecurityPolicyScope,
    SecurityPolicySource,
};
use conary_core::db::models::{
    ChunkPublicationState, ConvertedPackage, ScriptletSummaryForPublication,
};
use std::collections::BTreeMap;

fn summary(status: &str) -> ScriptletBundleSummary {
    ScriptletBundleSummary {
        publication_status: status.to_string(),
        scriptlet_fidelity: status.to_string(),
        target_compatibility: status.to_string(),
        ..ScriptletBundleSummary::default()
    }
}

fn unknown_command(command: &str, argv: &[&str], phase: &str) -> UnknownCommandEvidence {
    UnknownCommandEvidence {
        command: command.to_string(),
        command_provenance: CommandArgumentProvenance::Literal,
        argv: argv.iter().map(|arg| (*arg).to_string()).collect(),
        argument_provenance: vec![CommandArgumentProvenance::Literal; argv.len()],
        execution_context: CommandExecutionContext::Unconditional,
        phase: Some(phase.to_string()),
        lifecycle_paths: vec![phase.to_string()],
        source: CommandEvidenceSource::ShellAst,
        environment: Vec::new(),
        pipeline_id: None,
    }
}

fn boot_security_intent(
    class_id: &str,
    reason_code: &str,
    command: &str,
    argv: Vec<String>,
    lifecycle_paths: Vec<String>,
) -> BootSecurityIntentEvidence {
    BootSecurityIntentEvidence {
        class_id: class_id.to_string(),
        reason_code: reason_code.to_string(),
        command: command.to_string(),
        command_provenance: CommandArgumentProvenance::Literal,
        argument_provenance: vec![CommandArgumentProvenance::Literal; argv.len()],
        argv,
        execution_context: CommandExecutionContext::Unconditional,
        phase: Some("post-install".to_string()),
        lifecycle_paths,
        source: CommandEvidenceSource::ShellAst,
        environment: Vec::new(),
        pipeline_id: None,
    }
}

fn golden_summary(
    scriptlet_fidelity: &str,
    target_compatibility: &str,
    publication_status: &str,
) -> ScriptletBundleSummary {
    ScriptletBundleSummary {
        scriptlet_fidelity: scriptlet_fidelity.to_string(),
        target_compatibility: target_compatibility.to_string(),
        publication_status: publication_status.to_string(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{scriptlet_fidelity}:{publication_status}").as_bytes(),
        )),
        ..ScriptletBundleSummary::default()
    }
}

fn insert_golden_converted(
    conn: &rusqlite::Connection,
    name: &str,
    chunk: &str,
    summary: &ScriptletBundleSummary,
) {
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        name.to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        format!("sha256:{name}-source"),
        &[chunk.to_string()],
        10,
        format!("sha256:{name}-content"),
        format!("/cache/{name}.ccs"),
    );
    converted.package_architecture = Some("x86_64".to_string());
    converted.set_scriptlet_metadata(summary).unwrap();
    converted.insert(conn).unwrap();
}

#[test]
fn publication_policy_maps_statuses_to_decisions() {
    assert!(matches!(
        classify_summary(ScriptletSummaryForPublication {
            summary: summary("public"),
            valid: true,
        }),
        PublicationDecision::Ready
    ));
    assert!(matches!(
        classify_summary(ScriptletSummaryForPublication {
            summary: summary("private-review"),
            valid: true,
        }),
        PublicationDecision::ReviewRequired(_)
    ));
    assert!(matches!(
        classify_summary(ScriptletSummaryForPublication {
            summary: summary("local-only"),
            valid: true,
        }),
        PublicationDecision::ReviewRequired(_)
    ));
    assert!(matches!(
        classify_summary(ScriptletSummaryForPublication {
            summary: summary("blocked"),
            valid: true,
        }),
        PublicationDecision::Blocked(_)
    ));
    assert!(matches!(
        classify_summary(ScriptletSummaryForPublication {
            summary: summary("public"),
            valid: false,
        }),
        PublicationDecision::ReviewRequired(_)
    ));
}

#[test]
fn publication_golden_outcomes_filter_public_listing_and_chunks() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();

    let native_free = golden_summary("native-free", "source-native", "public");

    let mut fully_replaced = golden_summary("fully-replaced", "source-native", "public");
    fully_replaced.decision_counts = ScriptletDecisionCountsSummary {
        replaced: 1,
        ..ScriptletDecisionCountsSummary::default()
    };

    let mut legacy_replay = golden_summary("legacy-replay", "source-native", "private-review");
    legacy_replay.decision_counts = ScriptletDecisionCountsSummary {
        legacy: 1,
        ..ScriptletDecisionCountsSummary::default()
    };
    legacy_replay
        .review_reason_codes
        .push("legacy-replay-required".to_string());

    let mut review_required =
        golden_summary("review-required", "review-required", "private-review");
    review_required.decision_counts = ScriptletDecisionCountsSummary {
        review: 1,
        ..ScriptletDecisionCountsSummary::default()
    };
    review_required
        .review_reason_codes
        .push("review-class-deb-trigger".to_string());

    let local_only = golden_summary("local-only", "local-only", "local-only");

    let mut blocked = golden_summary("blocked", "blocked", "blocked");
    blocked.decision_counts = ScriptletDecisionCountsSummary {
        blocked: 1,
        ..ScriptletDecisionCountsSummary::default()
    };
    blocked
        .blocked_reason_codes
        .push("blocked-class-package-manager-recursion".to_string());

    let cases = [
        ("goal8a-native-free", "native-free-chunk", native_free, true),
        (
            "goal8a-fully-replaced",
            "fully-replaced-chunk",
            fully_replaced,
            true,
        ),
        (
            "goal8a-legacy-replay",
            "legacy-replay-chunk",
            legacy_replay,
            false,
        ),
        (
            "goal8a-review-required",
            "review-required-chunk",
            review_required,
            false,
        ),
        ("goal8a-local-only", "local-only-chunk", local_only, false),
        ("goal8a-blocked", "blocked-chunk", blocked, false),
    ];

    for (name, chunk, summary, _public_ready) in &cases {
        insert_golden_converted(&conn, name, chunk, summary);
    }

    let public_ready_names: std::collections::BTreeSet<_> =
        ConvertedPackage::find_publication_candidates(&conn, "fedora", None)
            .unwrap()
            .into_iter()
            .filter(|converted| converted.is_scriptlet_public_ready())
            .map(|converted| converted.package_name.unwrap())
            .collect();
    assert_eq!(
        public_ready_names,
        std::collections::BTreeSet::from([
            "goal8a-fully-replaced".to_string(),
            "goal8a-native-free".to_string(),
        ])
    );

    for (_name, chunk, _summary, public_ready) in cases {
        let expected = if public_ready {
            ChunkPublicationState::PublicReady
        } else {
            ChunkPublicationState::NonPublicOnly
        };
        assert_eq!(
            local_chunk_servable_by_public_gate(&db_path, chunk).unwrap(),
            public_ready,
            "{chunk}"
        );
        assert_eq!(
            ConvertedPackage::chunk_publication_state(&conn, chunk).unwrap(),
            expected,
            "{chunk}"
        );
    }
}

#[test]
fn publication_gate_does_not_promote_regex_like_signals_to_authority() {
    let mut summary = golden_summary("fully-replaced", "source-native", "public");
    summary
        .review_reason_codes
        .push("regex-advisory-review".to_string());
    summary.unknown_command_evidence.push(unknown_command(
        "systemctl",
        &["daemon-reload"],
        "post-install",
    ));

    assert!(matches!(
        classify_summary(ScriptletSummaryForPublication {
            summary,
            valid: true,
        }),
        PublicationDecision::Ready
    ));
}

#[test]
fn boot_security_intent_does_not_make_blocked_summary_public() {
    let summary = ScriptletBundleSummary {
        publication_status: "blocked".to_string(),
        blocked_classes: vec!["kernel-module".to_string()],
        boot_security_intents: vec![boot_security_intent(
            "kernel-module",
            "blocked-class-kernel-module",
            "depmod",
            vec!["6.10.0".to_string()],
            vec!["post-install".to_string()],
        )],
        ..ScriptletBundleSummary::default()
    };

    assert!(matches!(
        classify_summary(ScriptletSummaryForPublication {
            summary,
            valid: true,
        }),
        PublicationDecision::Blocked(_)
    ));
}

#[test]
fn blocked_apparmor_report_stays_private_and_carries_security_policy_intent() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut summary = golden_summary("blocked", "blocked", "blocked");
    summary.decision_counts = ScriptletDecisionCountsSummary {
        blocked: 1,
        ..ScriptletDecisionCountsSummary::default()
    };
    summary
        .blocked_reason_codes
        .push("blocked-class-apparmor".to_string());
    summary.blocked_classes.push("apparmor".to_string());
    summary.security_policy_intents = vec![apparmor_policy_intent()];
    insert_golden_converted(&conn, "apparmor-private", "apparmor-chunk", &summary);

    let converted = ConvertedPackage::find_publication_candidates(&conn, "fedora", None)
        .unwrap()
        .into_iter()
        .find(|converted| converted.package_name.as_deref() == Some("apparmor-private"))
        .expect("private converted AppArmor row should remain queryable as server state");
    assert!(!converted.is_scriptlet_public_ready());
    assert_eq!(
        ConvertedPackage::chunk_publication_state(&conn, "apparmor-chunk").unwrap(),
        ChunkPublicationState::NonPublicOnly
    );

    let report = match classify_converted_package(&converted) {
        PublicationDecision::Blocked(report) => report,
        other => panic!("expected blocked AppArmor report, got {other:?}"),
    };
    assert_eq!(report.publication_status, "blocked");
    assert_eq!(report.blocked_classes, vec!["apparmor"]);
    assert_eq!(report.security_policy_intents.len(), 1);
    let intent = &report.security_policy_intents[0];
    assert_eq!(intent.provider.as_str(), "apparmor");
    assert_eq!(intent.operation, "profile-reload");
    assert_eq!(intent.fallback.as_str(), "block-on-enforcing-target");
    assert_eq!(intent.reconciliation.state.as_str(), "review");
    assert_eq!(intent.scope.paths, vec!["/etc/apparmor.d/usr.bin.demo"]);
}

#[test]
fn blocked_pam_report_stays_private_and_non_public_only() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut summary = golden_summary("blocked", "blocked", "blocked");
    summary.decision_counts = ScriptletDecisionCountsSummary {
        blocked: 1,
        ..ScriptletDecisionCountsSummary::default()
    };
    summary
        .blocked_reason_codes
        .push("blocked-class-pam".to_string());
    summary.blocked_classes.push("pam".to_string());
    insert_golden_converted(&conn, "pam-private", "pam-chunk", &summary);

    let converted = ConvertedPackage::find_publication_candidates(&conn, "fedora", None)
        .unwrap()
        .into_iter()
        .find(|converted| converted.package_name.as_deref() == Some("pam-private"))
        .expect("private converted PAM row should remain queryable as server state");
    assert!(!converted.is_scriptlet_public_ready());
    assert_eq!(
        ConvertedPackage::chunk_publication_state(&conn, "pam-chunk").unwrap(),
        ChunkPublicationState::NonPublicOnly
    );

    let report = match classify_converted_package(&converted) {
        PublicationDecision::Blocked(report) => report,
        other => panic!("expected blocked PAM report, got {other:?}"),
    };
    assert_eq!(report.publication_status, "blocked");
    assert_eq!(report.blocked_classes, vec!["pam"]);
    assert!(report.boot_security_intents.is_empty());
    assert!(report.security_policy_intents.is_empty());
    assert!(report.message.contains("pam"));
}

#[test]
fn blocked_live_fetch_and_package_manager_reports_stay_private_and_non_public_only() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();

    for (name, chunk, class_id, reason_code) in [
        (
            "network-private",
            "network-chunk",
            "network",
            "blocked-class-network",
        ),
        (
            "pm-private",
            "pm-chunk",
            "package-manager-recursion",
            "blocked-class-package-manager-recursion",
        ),
    ] {
        let mut summary = golden_summary("blocked", "blocked", "blocked");
        summary.decision_counts = ScriptletDecisionCountsSummary {
            blocked: 1,
            ..ScriptletDecisionCountsSummary::default()
        };
        summary.blocked_reason_codes.push(reason_code.to_string());
        summary.blocked_classes.push(class_id.to_string());
        insert_golden_converted(&conn, name, chunk, &summary);

        let converted = ConvertedPackage::find_publication_candidates(&conn, "fedora", None)
            .unwrap()
            .into_iter()
            .find(|converted| converted.package_name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("private converted row {name} should remain queryable"));
        assert!(!converted.is_scriptlet_public_ready());
        assert_eq!(
            ConvertedPackage::chunk_publication_state(&conn, chunk).unwrap(),
            ChunkPublicationState::NonPublicOnly
        );

        let report = match classify_converted_package(&converted) {
            PublicationDecision::Blocked(report) => report,
            other => panic!("expected blocked {class_id} report, got {other:?}"),
        };
        assert_eq!(report.publication_status, "blocked");
        assert_eq!(report.blocked_classes, vec![class_id.to_string()]);
        assert!(report.boot_security_intents.is_empty());
        assert!(report.security_policy_intents.is_empty());
    }
}

#[test]
fn publication_report_reasons_are_deterministic_and_deduplicated() {
    let summary = ScriptletBundleSummary {
        publication_status: "private-review".to_string(),
        decision_counts: ScriptletDecisionCountsSummary {
            review: 2,
            ..ScriptletDecisionCountsSummary::default()
        },
        blocked_reason_codes: vec!["blocked-b".to_string(), "blocked-a".to_string()],
        review_reason_codes: vec!["review-a".to_string(), "review-a".to_string()],
        unknown_command_evidence: vec![
            unknown_command("zz", &[], "post-install"),
            unknown_command("aa", &[], "post-install"),
        ],
        blocked_classes: vec!["class-b".to_string(), "class-a".to_string()],
        ..ScriptletBundleSummary::default()
    };

    let report = report_from_summary(&summary, true);

    assert_eq!(
        report.reason_codes,
        vec![
            "blocked-b",
            "blocked-a",
            "review-a",
            "unknown-command:aa",
            "unknown-command:zz",
            "class-a",
            "class-b",
        ]
    );
}

#[test]
fn publication_report_message_names_blocked_classes() {
    let summary = ScriptletBundleSummary {
        publication_status: "blocked".to_string(),
        blocked_classes: vec!["kernel-module".to_string(), "initramfs".to_string()],
        ..ScriptletBundleSummary::default()
    };

    let report = report_from_summary(&summary, true);

    assert!(report.message.contains("Remi public preview"));
    assert!(report.message.contains("initramfs, kernel-module"));
    assert!(!report.message.contains("legacy scriptlet policy"));
}

#[test]
fn publication_report_includes_boot_security_intents() {
    let summary = ScriptletBundleSummary {
        publication_status: "blocked".to_string(),
        blocked_classes: vec!["initramfs".to_string()],
        boot_security_intents: vec![boot_security_intent(
            "initramfs",
            "blocked-class-initramfs",
            "dracut",
            vec!["--force".to_string()],
            vec!["post-install".to_string()],
        )],
        ..ScriptletBundleSummary::default()
    };

    let report = report_from_summary(&summary, true);

    assert_eq!(report.boot_security_intents.len(), 1);
    assert_eq!(report.boot_security_intents[0].class_id, "initramfs");
    assert_eq!(report.boot_security_intents[0].command, "dracut");
}

#[test]
fn publication_report_sanitizes_boot_and_security_policy_intents() {
    let mut summary = ScriptletBundleSummary {
        publication_status: "blocked".to_string(),
        blocked_classes: vec!["apparmor".to_string(), "initramfs".to_string()],
        boot_security_intents: vec![boot_security_intent(
            "initramfs",
            "blocked-class-initramfs",
            "dracut",
            vec![
                "--force".to_string(),
                "/home/remi/private-initramfs.img".to_string(),
                "SECRET=/home/remi/token".to_string(),
            ],
            vec!["/home/remi/private-phase".to_string()],
        )],
        security_policy_intents: vec![apparmor_policy_intent()],
        review_artifact_path: Some("/tmp/private-review-secret.json".to_string()),
        ..ScriptletBundleSummary::default()
    };
    summary.security_policy_intents[0]
        .source
        .argv
        .push("SECRET=/home/remi/token".to_string());
    summary.security_policy_intents[0]
        .scope
        .paths
        .push("/home/remi/private.pp".to_string());
    summary.security_policy_intents[0]
        .payload_evidence
        .paths
        .push("/home/remi/private.pp".to_string());

    let report = report_from_summary(&summary, true);
    let json = serde_json::to_string(&report).unwrap();

    assert!(report.review_artifact_available);
    assert_eq!(report.boot_security_intents[0].argv[1], "<path>");
    assert_eq!(report.boot_security_intents[0].argv[2], "<env-assignment>");
    assert_eq!(
        report.boot_security_intents[0].lifecycle_paths,
        vec!["<path>"]
    );
    assert!(
        report.security_policy_intents[0]
            .scope
            .paths
            .contains(&"/etc/apparmor.d/usr.bin.demo".to_string())
    );
    assert!(
        report.security_policy_intents[0]
            .scope
            .paths
            .contains(&"<path>".to_string())
    );
    assert!(!json.contains("/home/remi"));
    assert!(!json.contains("SECRET="));
    assert!(!json.contains("review_artifact_path"));
    assert!(!json.contains("private-review-secret"));
}

#[test]
fn publication_reports_share_the_same_sanitized_typed_command_evidence() {
    let summary = ScriptletBundleSummary {
        publication_status: "blocked".to_string(),
        unknown_command_evidence: vec![unknown_command(
            "private-helper",
            &["<path>", "<env-assignment>"],
            "post-install",
        )],
        ..ScriptletBundleSummary::default()
    };

    let sanitized = report_from_summary(&summary, true);
    let sanitized_json = serde_json::to_string(&sanitized).unwrap();
    let raw = raw_report_from_summary(&summary, true);

    assert_eq!(
        sanitized.unknown_command_evidence,
        raw.unknown_command_evidence
    );
    assert_eq!(
        sanitized.unknown_command_evidence[0].argv,
        ["<path>", "<env-assignment>"]
    );
    assert!(sanitized_json.contains("\"phase\":\"post-install\""));
    assert!(!sanitized_json.contains("/home/remi"));
    assert!(!sanitized_json.contains("SECRET="));
}

#[test]
fn raw_publication_report_retains_private_intents_for_review_artifacts() {
    let mut summary = ScriptletBundleSummary {
        publication_status: "blocked".to_string(),
        blocked_classes: vec!["apparmor".to_string()],
        security_policy_intents: vec![apparmor_policy_intent()],
        ..ScriptletBundleSummary::default()
    };
    summary.security_policy_intents[0]
        .source
        .argv
        .push("SECRET=/home/remi/token".to_string());
    summary.security_policy_intents[0]
        .scope
        .paths
        .push("/home/remi/private.pp".to_string());

    let report = raw_report_from_summary(&summary, true);
    let json = serde_json::to_string(&report).unwrap();

    assert!(json.contains("/home/remi/private.pp"));
    assert!(json.contains("SECRET=/home/remi/token"));
}

fn apparmor_policy_intent() -> SecurityPolicyIntent {
    SecurityPolicyIntent {
        schema: SECURITY_POLICY_INTENT_SCHEMA_V1.to_string(),
        id: "scriptlet:0:post-install:apparmor:apparmor_parser".to_string(),
        source: SecurityPolicySource {
            source_format: Some("deb".to_string()),
            source_distro: Some("ubuntu".to_string()),
            entry_id: Some("scriptlet:0:post-install".to_string()),
            command: Some("apparmor_parser".to_string()),
            argv: vec!["-r".to_string(), "/etc/apparmor.d/usr.bin.demo".to_string()],
            adapter_id: None,
        },
        provider: SecurityPolicyProvider::Apparmor,
        operation: "profile-reload".to_string(),
        scope: SecurityPolicyScope {
            kind: "profile".to_string(),
            name: Some("/etc/apparmor.d/usr.bin.demo".to_string()),
            paths: vec!["/etc/apparmor.d/usr.bin.demo".to_string()],
            service: None,
            port: None,
            extra: BTreeMap::new(),
        },
        desired_state: BTreeMap::new(),
        requirements: SecurityPolicyRequirements {
            required_on_active_provider: false,
            provider_mode: None,
            tools: vec!["apparmor_parser".to_string()],
            modules: Vec::new(),
        },
        fallback: SecurityPolicyFallback::BlockOnEnforcingTarget,
        payload_evidence: SecurityPolicyPayloadEvidence {
            payload_backed: true,
            paths: vec!["/etc/apparmor.d/usr.bin.demo".to_string()],
            digest: None,
        },
        reconciliation: SecurityPolicyReconciliation {
            state: SecurityPolicyReconciliationState::Review,
            reason: Some("blocked-class-apparmor".to_string()),
            target_provider: None,
        },
        extra: BTreeMap::new(),
    }
}
