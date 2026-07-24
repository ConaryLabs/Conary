// apps/conary/src/commands/query/scripts/tests.rs

#[cfg(test)]
mod query_scripts {
    use super::super::*;
    use conary_core::ccs::legacy_scriptlets::{
        CommandArgumentProvenance, CommandEvidenceSource, CommandExecutionContext, DecisionCounts,
        EffectConfidence, EffectReplacement, EffectSource, ForeignReplayPolicy,
        LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle, LegacyScriptletEntry, LifecyclePath,
        NativeInvocation, PublicationPolicy, PublicationStatus, ScriptletDecision, ScriptletEffect,
        ScriptletFidelity, SourceFormat, TargetCompatibility, TransactionOrder, VersionScheme,
    };
    use std::collections::BTreeMap;

    fn bundle_fixture() -> LegacyScriptletBundle {
        let legacy_body = "systemctl daemon-reload\n";
        let replaced_body = "ldconfig\n";
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
                "sha256:3333333333333333333333333333333333333333333333333333333333333333"
                    .to_string(),
            ),
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "remi".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "safe-or-legacy".to_string(),
            adapter_registry_digest: Some(
                "sha256:4444444444444444444444444444444444444444444444444444444444444444"
                    .to_string(),
            ),
            target_policy_digest: None,
            evidence_digest: Some(
                "sha256:5555555555555555555555555555555555555555555555555555555555555555"
                    .to_string(),
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
                entry_fixture("rpm:%preun", ScriptletDecision::Replaced, replaced_body),
                entry_fixture("rpm:%post", ScriptletDecision::Legacy, legacy_body),
            ],
            extra: BTreeMap::new(),
        }
    }

    fn entry_fixture(id: &str, decision: ScriptletDecision, body: &str) -> LegacyScriptletEntry {
        LegacyScriptletEntry {
            id: id.to_string(),
            native_slot: id.split(':').nth(1).unwrap_or("%post").to_string(),
            phase: if id.ends_with("%preun") {
                LifecyclePath::PreRemove
            } else {
                LifecyclePath::PostInstall
            },
            lifecycle_paths: vec!["install:first".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: vec!["-e".to_string()],
            body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
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
            sandbox: None,
            capabilities: vec!["ldconfig".to_string()],
            decision,
            reason_code: "protected-replay-required".to_string(),
            human_reason: Some("fixture reason".to_string()),
            evidence_digest: Some(
                "sha256:6666666666666666666666666666666666666666666666666666666666666666"
                    .to_string(),
            ),
            source_evidence_refs: vec!["capture:rpm:%post".to_string()],
            effects: vec![ScriptletEffect {
                kind: "ldconfig".to_string(),
                source: EffectSource::ShellAst,
                confidence: EffectConfidence::Declared,
                replacement: EffectReplacement::Complete,
                adapter_id: Some("ldconfig/v1".to_string()),
                adapter_digest: Some(
                    "sha256:7777777777777777777777777777777777777777777777777777777777777777"
                        .to_string(),
                ),
                command: Some("ldconfig".to_string()),
                args: vec!["-X".to_string()],
                path: Some("/usr/lib64".to_string()),
                reason_code: Some("ldconfig-cache-refresh".to_string()),
                extra: BTreeMap::new(),
            }],
            unknown_command_evidence: vec![UnknownCommandEvidence {
                command: "systemctl".to_string(),
                command_provenance: CommandArgumentProvenance::Literal,
                argv: vec!["restart".to_string(), "nginx.service".to_string()],
                argument_provenance: vec![
                    CommandArgumentProvenance::Literal,
                    CommandArgumentProvenance::Literal,
                ],
                execution_context: CommandExecutionContext::Unconditional,
                phase: Some("post-install".to_string()),
                lifecycle_paths: vec!["post-install".to_string()],
                source: CommandEvidenceSource::ShellAst,
                environment: Vec::new(),
                pipeline_id: None,
            }],
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

    fn package_identity() -> PackageQueryIdentity {
        PackageQueryIdentity {
            name: "nginx".to_string(),
            version: "1.28.0".to_string(),
        }
    }

    #[test]
    fn script_query_summary_renders_bundle_counts() {
        let output = render_ccs_bundle_text(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions::default(),
        )
        .expect("render summary");

        assert!(output.contains("Package: nginx 1.28.0"));
        assert!(output.contains("Legacy scriptlet bundle: conary.legacy-scriptlets.v1"));
        assert!(output.contains("Entries: 1 replaced, 1 legacy, 0 blocked, 0 review"));
        assert!(output.contains("rpm:%post"));
        assert!(!output.contains("systemctl daemon-reload"));
    }

    #[test]
    fn script_query_verbose_renders_entry_details() {
        let output = render_ccs_bundle_text(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions {
                verbose: true,
                ..ScriptQueryOptions::default()
            },
        )
        .expect("render verbose");

        assert!(output.contains("Interpreter: /bin/sh"));
        assert!(output.contains("Timeout: 30000ms"));
        assert!(output.contains("Effects:"));
        assert!(output.contains("body_sha256="));
        assert!(!output.contains("systemctl daemon-reload"));
    }

    #[test]
    fn script_query_entry_filter_renders_one_entry() {
        let output = render_ccs_bundle_text(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions {
                entry: Some("rpm:%post".to_string()),
                ..ScriptQueryOptions::default()
            },
        )
        .expect("render entry");

        assert!(output.contains("rpm:%post"));
        assert!(!output.contains("rpm:%preun"));
    }

    #[test]
    fn script_query_json_omits_raw_bodies_by_default() {
        let output = render_ccs_bundle_json(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions::default(),
        )
        .expect("render json");
        let json: serde_json::Value = serde_json::from_str(&output).expect("valid json");

        assert_eq!(json["bundle_present"], true);
        assert!(output.contains("body_sha256"));
        assert!(!output.contains("systemctl daemon-reload"));
        assert!(json["entries"][0].get("body").is_none());
    }

    #[test]
    fn script_query_json_reports_no_bundle_without_entries() {
        let output =
            render_ccs_bundle_json(&package_identity(), None, &ScriptQueryOptions::default())
                .expect("render json");
        let json: serde_json::Value = serde_json::from_str(&output).expect("valid json");

        assert_eq!(json["bundle_present"], false);
        assert!(json["bundle"].is_null());
        assert!(
            json["entries"]
                .as_array()
                .expect("entries array")
                .is_empty()
        );
    }

    #[test]
    fn script_query_json_reports_zero_entry_bundle() {
        let mut bundle = bundle_fixture();
        bundle.entries.clear();
        bundle.decision_counts = DecisionCounts::default();
        bundle.scriptlet_fidelity = ScriptletFidelity::NativeFree;

        let output = render_ccs_bundle_json(
            &package_identity(),
            Some(&bundle),
            &ScriptQueryOptions::default(),
        )
        .expect("render json");
        let json: serde_json::Value = serde_json::from_str(&output).expect("valid json");

        assert_eq!(json["bundle_present"], true);
        assert!(
            json["entries"]
                .as_array()
                .expect("entries array")
                .is_empty()
        );
    }
}
