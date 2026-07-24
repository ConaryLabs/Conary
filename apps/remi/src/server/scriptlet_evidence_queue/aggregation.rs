// apps/remi/src/server/scriptlet_evidence_queue/aggregation.rs

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::db::models::{
    ConvertedPackage, NewScriptletEvidenceCluster, NewScriptletEvidenceSample,
    ScriptletEvidenceKind, ScriptletEvidenceRecord,
};

use crate::server::publication;

use super::normalization::{
    normalize_command_shape, sanitize_boot_security_intent, sanitize_boot_security_intents,
    sanitize_security_policy_intents, sanitize_unknown_command_evidence, stable_cluster_key,
    target_profile_for_distro,
};
use super::types::{ClusterKeyInput, PendingEvidenceSample};

const CLUSTER_SCHEMA_VERSION: u32 = 1;

struct ClusterEvidence<'a> {
    blocked_class: &'a str,
    command: &'a str,
    normalized_command_shape: &'a str,
    lifecycle_phase: &'a str,
    typed: &'a ScriptletEvidenceRecord,
}

pub fn evidence_samples_from_converted(
    converted: &ConvertedPackage,
    cache_dir: &Path,
) -> Result<Vec<PendingEvidenceSample>> {
    let publication = converted.scriptlet_summary_for_publication();
    let summary = publication.summary;
    if publication.valid && summary.publication_status == "public" {
        return Ok(Vec::new());
    }

    let mut samples = Vec::new();
    if !summary.boot_security_intents.is_empty() {
        for intent in &summary.boot_security_intents {
            let lifecycle_phase = intent.phase.as_deref().unwrap_or("unknown");
            let normalized_command_shape = normalize_command_shape(&intent.command, &intent.argv);
            let evidence = ScriptletEvidenceRecord::new(ScriptletEvidenceKind::BootSecurity {
                intent: sanitize_boot_security_intent(intent),
            });
            samples.push(build_pending_sample(
                converted,
                cache_dir,
                &summary,
                ClusterEvidence {
                    blocked_class: &intent.class_id,
                    command: &intent.command,
                    normalized_command_shape: &normalized_command_shape,
                    lifecycle_phase,
                    typed: &evidence,
                },
            )?);
        }
    }

    if !summary.unknown_command_evidence.is_empty() {
        for evidence in &summary.unknown_command_evidence {
            let normalized_command_shape =
                normalize_command_shape(&evidence.command, &evidence.argv);
            let lifecycle_phase = evidence
                .phase
                .as_deref()
                .or_else(|| evidence.lifecycle_paths.first().map(String::as_str))
                .unwrap_or("unknown");
            let typed_evidence =
                ScriptletEvidenceRecord::new(ScriptletEvidenceKind::UnknownCommand {
                    command: sanitize_unknown_command_evidence(evidence),
                });
            samples.push(build_pending_sample(
                converted,
                cache_dir,
                &summary,
                ClusterEvidence {
                    blocked_class: "unknown-command",
                    command: &evidence.command,
                    normalized_command_shape: &normalized_command_shape,
                    lifecycle_phase,
                    typed: &typed_evidence,
                },
            )?);
        }
    }

    if !samples.is_empty() {
        return Ok(samples);
    }

    let blocked_classes = sorted_unique(summary.blocked_classes.iter().map(String::as_str));
    if !blocked_classes.is_empty() {
        let reason_shape = sorted_reason_codes(&summary).join(",");
        for blocked_class in blocked_classes {
            let normalized_command_shape = if reason_shape.is_empty() {
                blocked_class.clone()
            } else {
                format!("{blocked_class}:{reason_shape}")
            };
            let evidence = ScriptletEvidenceRecord::new(ScriptletEvidenceKind::SummaryClass {
                blocked_class: blocked_class.clone(),
                reason_codes: sorted_reason_codes(&summary),
            });
            samples.push(build_pending_sample(
                converted,
                cache_dir,
                &summary,
                ClusterEvidence {
                    blocked_class: &blocked_class,
                    command: "unknown",
                    normalized_command_shape: &normalized_command_shape,
                    lifecycle_phase: "unknown",
                    typed: &evidence,
                },
            )?);
        }
        return Ok(samples);
    }

    if !publication.valid {
        let evidence = ScriptletEvidenceRecord::new(ScriptletEvidenceKind::MalformedSummary);
        samples.push(build_pending_sample(
            converted,
            cache_dir,
            &summary,
            ClusterEvidence {
                blocked_class: "malformed-scriptlet-summary",
                command: "unknown",
                normalized_command_shape: "malformed-scriptlet-summary",
                lifecycle_phase: "unknown",
                typed: &evidence,
            },
        )?);
    }

    Ok(samples)
}

fn build_pending_sample(
    converted: &ConvertedPackage,
    cache_dir: &Path,
    summary: &ScriptletBundleSummary,
    evidence: ClusterEvidence<'_>,
) -> Result<PendingEvidenceSample> {
    let distro = converted
        .distro
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let target_profile = target_profile_for_distro(&distro);
    let key_input = ClusterKeyInput {
        schema_version: CLUSTER_SCHEMA_VERSION,
        distro: distro.clone(),
        target_profile: target_profile.clone(),
        blocked_class: evidence.blocked_class.to_string(),
        command: evidence.command.to_string(),
        normalized_command_shape: evidence.normalized_command_shape.to_string(),
        lifecycle_phase: evidence.lifecycle_phase.to_string(),
    };
    let stable = stable_cluster_key(&key_input);

    Ok(PendingEvidenceSample {
        cluster: NewScriptletEvidenceCluster {
            cluster_key: stable.cluster_key.clone(),
            schema_version: i64::from(CLUSTER_SCHEMA_VERSION),
            distro: distro.clone(),
            target_profile,
            blocked_class: evidence.blocked_class.to_string(),
            command: evidence.command.to_string(),
            normalized_command_shape: evidence.normalized_command_shape.to_string(),
            normalized_command_shape_hash: stable.normalized_command_shape_hash,
            lifecycle_phase: evidence.lifecycle_phase.to_string(),
        },
        sample: NewScriptletEvidenceSample {
            cluster_key: stable.cluster_key,
            converted_package_id: converted.id,
            original_checksum: converted.original_checksum.clone(),
            distro,
            package_name: converted
                .package_name
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            package_version: converted
                .package_version
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            package_architecture: converted.package_architecture.clone(),
            publication_status: summary.publication_status.clone(),
            scriptlet_fidelity: summary.scriptlet_fidelity.clone(),
            target_compatibility: summary.target_compatibility.clone(),
            typed_evidence: evidence.typed.clone(),
            reason_codes_json: serde_json::to_string(&sorted_reason_codes(summary))?,
            blocked_classes_json: serde_json::to_string(&summary.blocked_classes)?,
            boot_security_intents_json: serde_json::to_string(&sanitize_boot_security_intents(
                &summary.boot_security_intents,
            ))?,
            security_policy_intents_json: serde_json::to_string(
                &sanitize_security_policy_intents(&summary.security_policy_intents),
            )?,
            review_artifact_path: summary.review_artifact_path.clone(),
            review_artifact_stale: review_artifact_is_stale(
                cache_dir,
                summary.review_artifact_path.as_deref(),
            ),
            evidence_digest: summary.evidence_digest.clone(),
            curation_evidence_digest: summary.curation_evidence_digest.clone(),
        },
    })
}

fn review_artifact_is_stale(cache_dir: &Path, path: Option<&str>) -> bool {
    let Some(path) = path else {
        return true;
    };
    let path = Path::new(path);
    if !path.exists() {
        return true;
    }
    !matches!(
        publication::validate_review_artifact_path(cache_dir, path),
        Ok(true)
    )
}

fn sorted_reason_codes(summary: &ScriptletBundleSummary) -> Vec<String> {
    sorted_unique(
        summary
            .blocked_reason_codes
            .iter()
            .chain(summary.review_reason_codes.iter())
            .map(String::as_str),
    )
}

fn sorted_unique<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
    values
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::convert::ScriptletBundleSummary;
    use conary_core::ccs::legacy_scriptlets::{
        BootSecurityIntentEvidence, CommandArgumentProvenance, CommandEvidenceSource,
        CommandExecutionContext, UnknownCommandEvidence,
    };
    use conary_core::ccs::security_policy::{
        SECURITY_POLICY_INTENT_SCHEMA_V1, SecurityPolicyIntent, SecurityPolicyPayloadEvidence,
        SecurityPolicyProvider, SecurityPolicyScope, SecurityPolicySource,
    };
    use conary_core::db::models::ConvertedPackage;
    use conary_core::db::models::SCRIPTLET_EVIDENCE_RECORD_SCHEMA_V1;
    use conary_core::db::schema::migrate;
    use rusqlite::Connection;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
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
            source: conary_core::ccs::legacy_scriptlets::CommandEvidenceSource::ShellAst,
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
            phase: Some("postinstall".to_string()),
            lifecycle_paths,
            source: CommandEvidenceSource::ShellAst,
            environment: Vec::new(),
            pipeline_id: None,
        }
    }

    fn converted_package_with_summary(summary: ScriptletBundleSummary) -> ConvertedPackage {
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "kernel".to_string(),
            "1.0.0".to_string(),
            "rpm".to_string(),
            "sha256:kernel".to_string(),
            &["sha256:chunk".to_string()],
            42,
            "sha256:content".to_string(),
            "/cache/kernel.ccs".to_string(),
        );
        converted.package_architecture = Some("x86_64".to_string());
        converted.set_scriptlet_metadata(&summary).unwrap();
        converted
    }

    fn blocked_initramfs_summary() -> ScriptletBundleSummary {
        ScriptletBundleSummary {
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            publication_status: "blocked".to_string(),
            evidence_digest: Some("sha256:evidence".to_string()),
            curation_evidence_digest: Some("sha256:curation".to_string()),
            blocked_reason_codes: vec!["boot-security-initramfs".to_string()],
            blocked_classes: vec!["initramfs".to_string()],
            boot_security_intents: vec![boot_security_intent(
                "initramfs",
                "boot-security-initramfs",
                "dracut",
                vec![
                    "--force".to_string(),
                    "/boot/initramfs-6.10.12-200.fc40.x86_64.img".to_string(),
                ],
                vec!["/boot".to_string()],
            )],
            ..ScriptletBundleSummary::default()
        }
    }

    #[test]
    fn blocked_boot_security_summary_creates_command_cluster() {
        let conn = test_conn();
        let mut converted = converted_package_with_summary(blocked_initramfs_summary());
        converted.insert(&conn).unwrap();

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].cluster.blocked_class, "initramfs");
        assert_eq!(samples[0].cluster.command, "dracut");
        assert_eq!(samples[0].cluster.target_profile, "fedora-44");
        assert_eq!(samples[0].cluster.lifecycle_phase, "postinstall");
        assert!(
            samples[0]
                .cluster
                .normalized_command_shape
                .contains("<boot>/initramfs-<kver>.img")
        );
        assert_eq!(samples[0].sample.package_name, "kernel");
        assert!(samples[0].sample.review_artifact_stale);
    }

    #[test]
    fn boot_security_intents_are_sanitized_before_storage() {
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            publication_status: "blocked".to_string(),
            blocked_reason_codes: vec!["boot-security-selinux".to_string()],
            blocked_classes: vec!["selinux-module".to_string()],
            boot_security_intents: vec![boot_security_intent(
                "selinux-module",
                "boot-security-selinux",
                "semodule",
                vec![
                    "--module=/tmp/foo.pp".to_string(),
                    "--install=/home/remi/private.pp".to_string(),
                    "SECRET=/home/remi/token".to_string(),
                ],
                vec!["/home/remi/private.pp".to_string()],
            )],
            ..ScriptletBundleSummary::default()
        };
        let converted = converted_package_with_summary(summary);

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();
        assert_eq!(samples.len(), 1);
        let intents = &samples[0].sample.boot_security_intents_json;
        assert!(intents.contains("--module=<path>"));
        assert!(intents.contains("--install=<path>"));
        assert!(intents.contains("<env-assignment>"));
        assert!(!intents.contains("/tmp/foo.pp"));
        assert!(!intents.contains("/home/remi"));
        assert!(!intents.contains("SECRET=/home"));
        let typed = serde_json::to_value(&samples[0].sample.typed_evidence).unwrap();
        assert_eq!(typed["schema"], SCRIPTLET_EVIDENCE_RECORD_SCHEMA_V1);
        assert_eq!(typed["kind"], "boot-security");
        assert_eq!(typed["intent"]["source"], "shell-ast");
        assert_eq!(typed["intent"]["execution_context"], "unconditional");
        assert_eq!(typed["intent"]["argv"][0], "--module=<path>");
        assert!(
            !serde_json::to_string(&samples[0].sample.typed_evidence)
                .unwrap()
                .contains("/home/remi")
        );
    }

    #[test]
    fn security_policy_intents_are_sanitized_before_storage() {
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            publication_status: "blocked".to_string(),
            blocked_reason_codes: vec!["security-policy-selinux".to_string()],
            blocked_classes: vec!["selinux-module".to_string()],
            security_policy_intents: vec![SecurityPolicyIntent {
                schema: SECURITY_POLICY_INTENT_SCHEMA_V1.to_string(),
                id: "selinux-policy-install".to_string(),
                source: SecurityPolicySource {
                    command: Some("semodule".to_string()),
                    argv: vec![
                        "--module=/tmp/foo.pp".to_string(),
                        "--install=/home/remi/private.pp".to_string(),
                        "SECRET=/home/remi/token".to_string(),
                    ],
                    ..SecurityPolicySource::default()
                },
                provider: SecurityPolicyProvider::Selinux,
                operation: "install-module".to_string(),
                scope: SecurityPolicyScope {
                    kind: "path".to_string(),
                    paths: vec!["/home/remi/private.pp".to_string()],
                    ..SecurityPolicyScope::default()
                },
                desired_state: BTreeMap::new(),
                requirements: Default::default(),
                fallback: Default::default(),
                payload_evidence: SecurityPolicyPayloadEvidence {
                    payload_backed: true,
                    paths: vec![
                        "/tmp/foo.pp".to_string(),
                        "/usr/share/selinux/packages/public.pp".to_string(),
                    ],
                    digest: Some("sha256:payload".to_string()),
                },
                reconciliation: Default::default(),
                extra: BTreeMap::new(),
            }],
            ..ScriptletBundleSummary::default()
        };
        let converted = converted_package_with_summary(summary);

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();
        assert_eq!(samples.len(), 1);
        let intents = &samples[0].sample.security_policy_intents_json;
        assert!(intents.contains("\"provider\":\"selinux\""));
        assert!(intents.contains("--module=<path>"));
        assert!(intents.contains("--install=<path>"));
        assert!(intents.contains("<env-assignment>"));
        assert!(
            intents.contains("\"paths\":[\"<path>\",\"/usr/share/selinux/packages/public.pp\"]")
        );
        assert!(!intents.contains("/tmp/foo.pp"));
        assert!(!intents.contains("/home/remi"));
        assert!(!intents.contains("SECRET=/home"));
    }

    #[test]
    fn queue_preserves_formal_parser_evidence_without_string_reclassification() {
        let mut piped = unknown_command(
            "custom-rpm-helper",
            &["--do-it", "/tmp/private"],
            "post-install",
        );
        piped.execution_context = CommandExecutionContext::Pipeline;
        piped.pipeline_id = Some(42);
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "legacy".to_string(),
            target_compatibility: "private-review".to_string(),
            publication_status: "private-review".to_string(),
            unknown_command_evidence: vec![
                unknown_command("set", &[], "pre-install"),
                unknown_command("true", &[], "pre-install"),
                piped,
            ],
            ..ScriptletBundleSummary::default()
        };
        let converted = converted_package_with_summary(summary);

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();

        assert_eq!(samples.len(), 3);
        assert_eq!(samples[2].cluster.command, "custom-rpm-helper");
        assert_eq!(
            samples[2].cluster.normalized_command_shape,
            "custom-rpm-helper --do-it <path>"
        );
        assert_eq!(samples[2].cluster.lifecycle_phase, "post-install");
        let typed = serde_json::to_value(&samples[2].sample.typed_evidence).unwrap();
        assert_eq!(typed["schema"], SCRIPTLET_EVIDENCE_RECORD_SCHEMA_V1);
        assert_eq!(typed["kind"], "unknown-command");
        assert_eq!(typed["command"]["source"], "shell-ast");
        assert_eq!(typed["command"]["execution_context"], "pipeline");
        assert_eq!(typed["command"]["pipeline_id"], 42);
        assert_eq!(typed["command"]["argv"][1], "<path>");
        assert!(
            !serde_json::to_string(&samples[2].sample.typed_evidence)
                .unwrap()
                .contains("/tmp/private")
        );
    }

    #[test]
    fn queue_preserves_deb_command_nodes() {
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "legacy".to_string(),
            target_compatibility: "private-review".to_string(),
            publication_status: "private-review".to_string(),
            unknown_command_evidence: vec![
                unknown_command("true", &[], "post-install"),
                unknown_command("custom-deb-helper", &["configure"], "post-install"),
            ],
            ..ScriptletBundleSummary::default()
        };
        let mut converted = converted_package_with_summary(summary);
        converted.distro = Some("ubuntu".to_string());

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();

        assert_eq!(samples.len(), 2);
        assert_eq!(samples[1].cluster.command, "custom-deb-helper");
        assert_eq!(samples[1].cluster.target_profile, "ubuntu-26.04");
    }

    #[test]
    fn queue_preserves_arch_function_body_command_nodes() {
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "legacy".to_string(),
            target_compatibility: "private-review".to_string(),
            publication_status: "private-review".to_string(),
            unknown_command_evidence: vec![
                unknown_command("return", &[], "post-install"),
                unknown_command("custom-arch-helper", &["1.0"], "post-install"),
            ],
            ..ScriptletBundleSummary::default()
        };
        let mut converted = converted_package_with_summary(summary);
        converted.distro = Some("arch".to_string());

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();

        assert_eq!(samples.len(), 2);
        assert_eq!(samples[1].cluster.command, "custom-arch-helper");
        assert_eq!(samples[1].cluster.target_profile, "arch");
    }

    #[test]
    fn command_evidence_remains_primary_over_summary_class_fallback() {
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            publication_status: "blocked".to_string(),
            unknown_command_evidence: vec![
                unknown_command("set", &[], "pre-install"),
                unknown_command("true", &[], "post-install"),
            ],
            blocked_reason_codes: vec!["blocked-class-package-manager-recursion".to_string()],
            blocked_classes: vec!["package-manager-recursion".to_string()],
            ..ScriptletBundleSummary::default()
        };
        let converted = converted_package_with_summary(summary);

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();

        assert_eq!(samples.len(), 2);
        assert!(
            samples
                .iter()
                .all(|sample| sample.cluster.blocked_class == "unknown-command")
        );
    }

    #[test]
    fn queue_keeps_boot_security_and_unknown_command_evidence_from_one_summary() {
        let mut summary = blocked_initramfs_summary();
        summary.unknown_command_evidence = vec![unknown_command(
            "custom-helper",
            &["refresh"],
            "post-install",
        )];
        let converted = converted_package_with_summary(summary);

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();

        assert_eq!(samples.len(), 2);
        assert_eq!(
            samples
                .iter()
                .map(|sample| sample.cluster.blocked_class.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["initramfs", "unknown-command"])
        );
    }

    #[test]
    fn identical_commands_in_different_lifecycle_phases_form_distinct_clusters() {
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "legacy".to_string(),
            target_compatibility: "private-review".to_string(),
            publication_status: "private-review".to_string(),
            unknown_command_evidence: vec![
                unknown_command("custom-helper", &["refresh"], "pre-install"),
                unknown_command("custom-helper", &["refresh"], "post-install"),
            ],
            ..ScriptletBundleSummary::default()
        };
        let converted = converted_package_with_summary(summary);

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();

        assert_eq!(samples.len(), 2);
        assert_ne!(
            samples[0].cluster.cluster_key,
            samples[1].cluster.cluster_key
        );
        assert_eq!(
            samples
                .iter()
                .map(|sample| sample.cluster.lifecycle_phase.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["post-install", "pre-install"])
        );
    }

    #[test]
    fn review_required_without_command_evidence_creates_class_fallback() {
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "private-review".to_string(),
            publication_status: "private-review".to_string(),
            review_reason_codes: vec!["systemd-runtime-user".to_string()],
            blocked_classes: vec!["systemd-runtime-user".to_string()],
            ..ScriptletBundleSummary::default()
        };
        let converted = converted_package_with_summary(summary);

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].cluster.blocked_class, "systemd-runtime-user");
        assert_eq!(samples[0].cluster.command, "unknown");
        assert_eq!(
            samples[0].cluster.normalized_command_shape,
            "systemd-runtime-user:systemd-runtime-user"
        );
    }

    #[test]
    fn malformed_summary_json_creates_malformed_metadata_cluster() {
        let mut converted = converted_package_with_summary(ScriptletBundleSummary::default());
        converted.publication_status = "blocked".to_string();
        converted.scriptlet_summary_json = "{not-json".to_string();

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(
            samples[0].cluster.blocked_class,
            "malformed-scriptlet-summary"
        );
        assert_eq!(
            samples[0].cluster.normalized_command_shape,
            "malformed-scriptlet-summary"
        );
    }

    #[test]
    fn public_ready_row_creates_no_cluster() {
        let converted = converted_package_with_summary(ScriptletBundleSummary::default());
        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();
        assert!(samples.is_empty());
    }
}
