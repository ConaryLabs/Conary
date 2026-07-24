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
        schema_version: HERMETIC_EVIDENCE_SCHEMA_V1,
        build_input: BuildInputIdentity {
            recipe: RecipeIdentity::GeneratedRecipe {
                generator: "conary-foreign-converter".to_string(),
                canonical_hash: crate::hash::sha256_prefixed(
                    format!(
                        "{}:{}:{}:{}",
                        format, metadata.name, metadata.version, checksum
                    )
                    .as_bytes(),
                ),
                inference_trace_hash: crate::hash::sha256_prefixed(
                    format!("foreign-conversion:{format}").as_bytes(),
                ),
            },
            source: SourceIdentity::Archive {
                url: metadata.package_path.display().to_string(),
                checksum: checksum.to_string(),
            },
            additional_sources: Vec::new(),
            patches: Vec::new(),
            local_tree: None,
            ecosystem_dependencies: Vec::new(),
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
        ecosystem_policy: EcosystemPolicyReport::clean("foreign-conversion"),
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
    let flattened = metadata.scriptlets.iter().map(|scriptlet| {
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
    let mut status = CommandRiskStatus::Clean;
    let mut entries = Vec::new();

    for report in reports {
        status = max_command_risk_status(status, report.status);
        entries.extend(report.entries);
    }

    if status == CommandRiskStatus::Clean && entries.is_empty() {
        CommandRiskReport::clean()
    } else {
        CommandRiskReport {
            status,
            classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries,
        }
    }
}

pub(super) fn max_command_risk_status(
    left: CommandRiskStatus,
    right: CommandRiskStatus,
) -> CommandRiskStatus {
    match (left, right) {
        (CommandRiskStatus::Blocked, _) | (_, CommandRiskStatus::Blocked) => {
            CommandRiskStatus::Blocked
        }
        (CommandRiskStatus::Review, _) | (_, CommandRiskStatus::Review) => {
            CommandRiskStatus::Review
        }
        (CommandRiskStatus::Clean, CommandRiskStatus::Clean) => CommandRiskStatus::Clean,
    }
}

pub(super) fn build_command_risk_report_from_shared(
    report: &CommandRiskReport,
) -> BuildCommandRiskReport {
    BuildCommandRiskReport {
        status: policy_status_from_command_risk(report.status),
        classifier_version: report.classifier_version.clone(),
        entries: report
            .entries
            .iter()
            .map(|entry| BuildCommandRiskEntry {
                phase: entry.source.clone(),
                command: entry.command.clone(),
                reason_code: entry.reason_code.clone(),
                severity: policy_status_from_command_risk(entry.severity),
                evidence: entry.evidence.clone(),
            })
            .collect(),
    }
}

pub(super) fn policy_status_from_command_risk(status: CommandRiskStatus) -> PolicyStatus {
    match status {
        CommandRiskStatus::Clean => PolicyStatus::Clean,
        CommandRiskStatus::Review => PolicyStatus::Review,
        CommandRiskStatus::Blocked => PolicyStatus::Blocked,
    }
}

pub(super) fn derive_provides(files: &[ExtractedFile]) -> Provides {
    let file_paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    let detected = LanguageDepDetector::detect_all_provides(&file_paths);

    let mut provides = Provides::default();
    let mut sonames = std::collections::BTreeSet::new();
    let mut binaries = std::collections::BTreeSet::new();
    let mut pkgconfig = std::collections::BTreeSet::new();

    for dep in detected {
        if dep.class == DependencyClass::Soname {
            sonames.insert(dep.name);
        }
    }

    for file in files {
        if file.mode & 0o111 != 0
            && let Some(name) = binary_name(&file.path)
        {
            binaries.insert(name);
        }

        if let Some(name) = pkgconfig_name(&file.path) {
            pkgconfig.insert(name);
        }
    }

    provides.sonames = sonames.into_iter().collect();
    provides.binaries = binaries.into_iter().collect();
    provides.pkgconfig = pkgconfig.into_iter().collect();
    provides
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

pub(super) fn binary_name(path: &str) -> Option<String> {
    let name = Path::new(path).file_name()?.to_str()?;
    if path.starts_with("/usr/bin/")
        || path.starts_with("/usr/sbin/")
        || path.starts_with("/bin/")
        || path.starts_with("/sbin/")
    {
        return Some(name.to_string());
    }

    None
}

pub(super) fn pkgconfig_name(path: &str) -> Option<String> {
    if !path.contains("/pkgconfig/") || !path.ends_with(".pc") {
        return None;
    }

    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(std::string::ToString::to_string)
}
