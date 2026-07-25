// conary-core/src/ccs/convert/converter/evidence.rs

use super::*;

/// Errors that can occur during conversion
#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error("I/O error: {0}")]
    IoError(String),

    #[error("Manifest error: {0}")]
    ManifestError(String),

    #[error("Build error: {0}")]
    BuildError(String),

    #[error("Fidelity too low: {0}")]
    FidelityTooLow(String),
}

pub(super) fn foreign_conversion_evidence(
    format: &str,
    checksum: &str,
    metadata: &PackageMetadata,
    build_risk_report: &CommandRiskReport,
) -> HermeticBuildEvidence {
    HermeticBuildEvidence {
        schema_version: HERMETIC_EVIDENCE_SCHEMA,
        build_input: BuildInputIdentity {
            recipe: RecipeIdentity::ForeignPackageConversion {
                format: format.to_string(),
                contract_hash: crate::hash::sha256_prefixed(
                    format!(
                        "{}:{}:{}:{}",
                        format, metadata.name, metadata.version, checksum
                    )
                    .as_bytes(),
                ),
            },
            source: SourceIdentity::Archive {
                url: metadata.package_path.display().to_string(),
                checksum: checksum.to_string(),
            },
            additional_sources: Vec::new(),
            patches: Vec::new(),
            local_tree: None,
            builder_environment: BuilderEnvironmentIdentity {
                kind: BuilderEnvironmentKind::Pristine,
                sysroot_hash: None,
                toolchain_hash: None,
                diagnostics: vec![
                    "foreign package converted without host script execution".to_string(),
                ],
            },
        },
        dependency_lock: DependencyLock::default(),
        command_risk: build_command_risk_report_from_shared(build_risk_report),
        reproducibility: ReproducibilityRecord {
            source_date_epoch: None,
            path_remap_count: 0,
            env_keys: Vec::new(),
        },
        divergence: DivergenceReport::default(),
        diagnostics: Vec::new(),
    }
}

pub(super) fn classify_foreign_build_body_risk(
    format: &str,
    files: &[ExtractedFile],
) -> CommandRiskReport {
    merge_command_risk_reports(files.iter().filter_map(|file| {
        let path = file.path.trim_start_matches('/');
        if path != "PKGBUILD" && !path.ends_with("/PKGBUILD") {
            return None;
        }
        let content = std::str::from_utf8(&file.content).ok()?;
        Some(classify_shell_text(
            &format!("foreign-build-body:{format}:{}", file.path),
            content,
        ))
    }))
}

pub(super) fn classify_foreign_scriptlet_risk(metadata: &PackageMetadata) -> CommandRiskReport {
    let flattened = metadata
        .diagnostic_scriptlet_evidence
        .iter()
        .map(|scriptlet| {
            classify_shell_text(
                &format!("foreign-scriptlet:{}:{}", metadata.name, scriptlet.phase),
                &scriptlet.content,
            )
        });
    let native = metadata.native_scriptlet_abi.iter().filter_map(|entry| {
        let text = entry.body.text.as_ref()?;
        Some(classify_shell_text(
            &format!("foreign-native-scriptlet:{}:{}", metadata.name, entry.id),
            text,
        ))
    });

    merge_command_risk_reports(flattened.chain(native))
}

pub(super) fn merge_command_risk_reports(
    reports: impl IntoIterator<Item = CommandRiskReport>,
) -> CommandRiskReport {
    let mut highest_severity = CommandRiskSeverity::None;
    let mut entries = Vec::new();

    for report in reports {
        highest_severity = highest_severity.max(report.highest_severity);
        entries.extend(report.entries);
    }

    if highest_severity == CommandRiskSeverity::None && entries.is_empty() {
        CommandRiskReport::no_findings()
    } else {
        CommandRiskReport {
            highest_severity,
            classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries,
        }
    }
}

pub(super) fn build_command_risk_report_from_shared(
    report: &CommandRiskReport,
) -> BuildCommandRiskReport {
    BuildCommandRiskReport {
        highest_severity: report.highest_severity,
        classifier_version: report.classifier_version.clone(),
        entries: report
            .entries
            .iter()
            .map(|entry| BuildCommandRiskEntry {
                phase: entry.source.clone(),
                command: entry.command.clone(),
                reason_code: entry.reason_code.clone(),
                severity: entry.severity,
                evidence: entry.evidence.clone(),
            })
            .collect(),
    }
}

pub(super) fn merge_native_provides(
    provides: &mut Provides,
    native_provides: &[crate::packages::traits::Dependency],
) {
    let mut capabilities: std::collections::BTreeSet<String> =
        provides.capabilities.iter().cloned().collect();

    for native in native_provides {
        if native.dep_type != DependencyType::Runtime {
            continue;
        }

        capabilities.insert(native.name.clone());
        if let Some(version) = native.version.as_deref().map(str::trim)
            && !version.is_empty()
        {
            capabilities.insert(format!("{} {}", native.name, version));
        }
    }

    provides.capabilities = capabilities.into_iter().collect();
}
