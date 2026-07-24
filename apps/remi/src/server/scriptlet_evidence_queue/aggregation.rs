// apps/remi/src/server/scriptlet_evidence_queue/aggregation.rs

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::db::models::{
    ConvertedPackage, NewScriptletEvidenceCluster, NewScriptletEvidenceSample,
};

use crate::server::publication;

use super::classification::{
    UNKNOWN_COMMAND_NORMALIZATION_VERSION, UnknownCommandDisposition, classify_unknown_command,
};
use super::normalization::{
    normalize_command_shape, sanitize_boot_security_intents, sanitize_security_policy_intents,
    stable_cluster_key, target_profile_for_distro,
};
use super::types::{ClusterKeyInput, PendingEvidenceSample};

const CLUSTER_SCHEMA_VERSION: u32 = 1;

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
            samples.push(build_pending_sample(
                converted,
                cache_dir,
                &summary,
                &intent.class_id,
                &intent.command,
                &normalized_command_shape,
                lifecycle_phase,
            )?);
        }
        return Ok(samples);
    }

    let unknown_commands = sorted_unique(summary.unknown_commands.iter().map(String::as_str));
    if !unknown_commands.is_empty() {
        let signal_commands = unknown_commands.into_iter().filter(|command| {
            matches!(
                classify_unknown_command(command),
                UnknownCommandDisposition::Signal
            )
        });
        for command in signal_commands {
            samples.push(build_pending_sample(
                converted,
                cache_dir,
                &summary,
                "unknown-command",
                &command,
                &command,
                "unknown",
            )?);
        }
        if !samples.is_empty() {
            return Ok(samples);
        }
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
            samples.push(build_pending_sample(
                converted,
                cache_dir,
                &summary,
                &blocked_class,
                "unknown",
                &normalized_command_shape,
                "unknown",
            )?);
        }
        return Ok(samples);
    }

    if !publication.valid {
        samples.push(build_pending_sample(
            converted,
            cache_dir,
            &summary,
            "malformed-scriptlet-summary",
            "unknown",
            "malformed-scriptlet-summary",
            "unknown",
        )?);
    }

    Ok(samples)
}

fn build_pending_sample(
    converted: &ConvertedPackage,
    cache_dir: &Path,
    summary: &ScriptletBundleSummary,
    blocked_class: &str,
    command: &str,
    normalized_command_shape: &str,
    lifecycle_phase: &str,
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
        blocked_class: blocked_class.to_string(),
        command: command.to_string(),
        normalized_command_shape: normalized_command_shape.to_string(),
        lifecycle_phase: lifecycle_phase.to_string(),
    };
    let stable = stable_cluster_key(&key_input);

    Ok(PendingEvidenceSample {
        cluster: NewScriptletEvidenceCluster {
            cluster_key: stable.cluster_key.clone(),
            schema_version: i64::from(CLUSTER_SCHEMA_VERSION),
            normalization_version: UNKNOWN_COMMAND_NORMALIZATION_VERSION,
            distro: distro.clone(),
            target_profile,
            blocked_class: blocked_class.to_string(),
            command: command.to_string(),
            normalized_command_shape: normalized_command_shape.to_string(),
            normalized_command_shape_hash: stable.normalized_command_shape_hash,
            lifecycle_phase: lifecycle_phase.to_string(),
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
    use conary_core::ccs::legacy_scriptlets::BootSecurityIntentEvidence;
    use conary_core::ccs::security_policy::{
        SECURITY_POLICY_INTENT_SCHEMA_V1, SecurityPolicyIntent, SecurityPolicyPayloadEvidence,
        SecurityPolicyProvider, SecurityPolicyScope, SecurityPolicySource,
    };
    use conary_core::db::models::ConvertedPackage;
    use conary_core::db::schema::migrate;
    use rusqlite::Connection;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    fn converted_package_with_summary(summary: ScriptletBundleSummary) -> ConvertedPackage {
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "kernel".to_string(),
            "1.0.0".to_string(),
            "rpm".to_string(),
            "sha256:kernel".to_string(),
            "partial".to_string(),
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
            boot_security_intents: vec![BootSecurityIntentEvidence {
                class_id: "initramfs".to_string(),
                reason_code: "boot-security-initramfs".to_string(),
                command: "dracut".to_string(),
                argv: vec![
                    "--force".to_string(),
                    "/boot/initramfs-6.10.12-200.fc40.x86_64.img".to_string(),
                ],
                phase: Some("postinstall".to_string()),
                lifecycle_paths: vec!["/boot".to_string()],
            }],
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
            boot_security_intents: vec![BootSecurityIntentEvidence {
                class_id: "selinux-module".to_string(),
                reason_code: "boot-security-selinux".to_string(),
                command: "semodule".to_string(),
                argv: vec![
                    "--module=/tmp/foo.pp".to_string(),
                    "--install=/home/remi/private.pp".to_string(),
                    "SECRET=/home/remi/token".to_string(),
                ],
                phase: Some("postinstall".to_string()),
                lifecycle_paths: vec!["/home/remi/private.pp".to_string()],
            }],
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
    fn queue_filters_rpm_shell_structure_but_keeps_real_unknown_commands() {
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "legacy".to_string(),
            target_compatibility: "private-review".to_string(),
            publication_status: "private-review".to_string(),
            unknown_commands: vec![
                "set".to_string(),
                "$1".to_string(),
                "{".to_string(),
                "custom-rpm-helper".to_string(),
            ],
            ..ScriptletBundleSummary::default()
        };
        let converted = converted_package_with_summary(summary);

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();

        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].cluster.command, "custom-rpm-helper");
        assert_eq!(
            samples[0].cluster.normalization_version,
            UNKNOWN_COMMAND_NORMALIZATION_VERSION
        );
    }

    #[test]
    fn queue_filters_deb_dispatch_structure_but_keeps_real_unknown_commands() {
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "legacy".to_string(),
            target_compatibility: "private-review".to_string(),
            publication_status: "private-review".to_string(),
            unknown_commands: vec![
                "true".to_string(),
                "configure)".to_string(),
                "postinst".to_string(),
                "custom-deb-helper".to_string(),
            ],
            ..ScriptletBundleSummary::default()
        };
        let mut converted = converted_package_with_summary(summary);
        converted.distro = Some("ubuntu".to_string());

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();

        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].cluster.command, "custom-deb-helper");
        assert_eq!(samples[0].cluster.target_profile, "ubuntu-26.04");
    }

    #[test]
    fn queue_filters_arch_function_structure_but_keeps_real_unknown_commands() {
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "legacy".to_string(),
            target_compatibility: "private-review".to_string(),
            publication_status: "private-review".to_string(),
            unknown_commands: vec![
                "post_install".to_string(),
                "return".to_string(),
                "custom-arch-helper".to_string(),
            ],
            ..ScriptletBundleSummary::default()
        };
        let mut converted = converted_package_with_summary(summary);
        converted.distro = Some("arch".to_string());

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();

        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].cluster.command, "custom-arch-helper");
        assert_eq!(samples[0].cluster.target_profile, "arch");
    }

    #[test]
    fn all_noise_unknown_commands_fall_through_to_high_signal_blocked_class() {
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            publication_status: "blocked".to_string(),
            unknown_commands: vec!["set".to_string(), "true".to_string()],
            blocked_reason_codes: vec!["blocked-class-package-manager-recursion".to_string()],
            blocked_classes: vec!["package-manager-recursion".to_string()],
            ..ScriptletBundleSummary::default()
        };
        let converted = converted_package_with_summary(summary);

        let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();

        assert_eq!(samples.len(), 1);
        assert_eq!(
            samples[0].cluster.blocked_class,
            "package-manager-recursion"
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
