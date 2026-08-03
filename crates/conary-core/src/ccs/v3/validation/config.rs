// conary-core/src/ccs/v3/validation/config.rs

//! Exact signed configuration and package-policy validation.

use super::super::diagnostics::{V3Diagnostic, V3DiagnosticCode};
use super::super::schema::{AuthorityDocumentV3, PackageDataV3, PackagePolicyV3};

pub(super) fn validate_package_policy(
    policy: &PackagePolicyV3,
    field: &str,
    diagnostics: &mut Vec<V3Diagnostic>,
) {
    if policy.allow_host_mutation {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::KindContractViolation,
            "package requests host mutation that the selected-root installer does not implement",
            Some(format!("{field}.allow_host_mutation")),
            "keep host mutation disabled until a typed transaction consumer exists",
        ));
    }
}

pub(super) fn validate_config_authority(
    data: &PackageDataV3,
    authority: &AuthorityDocumentV3,
    diagnostics: &mut Vec<V3Diagnostic>,
) {
    let source_format = authority
        .lifecycle
        .native_lifecycle
        .as_ref()
        .map(|lifecycle| lifecycle.source_format);
    let mut config_by_path = std::collections::BTreeMap::new();

    for config in &data.config {
        if let Err(error) = validate_config_path(&config.path) {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                error,
                Some("kind.package.config.path".to_string()),
                "write a canonical absolute config path into signed authority",
            ));
        }
        if config_by_path
            .insert(config.path.as_str(), config.semantics)
            .is_some()
        {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!("config path {} is declared more than once", config.path),
                Some("kind.package.config".to_string()),
                "write exactly one signed config declaration per path",
            ));
        }

        let semantics = config.semantics;
        if semantics.ghost && semantics.remove_on_upgrade {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!(
                    "config path {} cannot be both ghost and remove-on-upgrade",
                    config.path
                ),
                Some("kind.package.config.semantics".to_string()),
                "encode the one exact source-package config semantic",
            ));
        }
        if semantics.ghost && source_format != Some(crate::ccs::native_lifecycle::SourceFormat::Rpm)
        {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!(
                    "ghost config path {} requires signed RPM source authority",
                    config.path
                ),
                Some("kind.package.config.semantics.ghost".to_string()),
                "carry the exact RPM native lifecycle source contract",
            ));
        }
        if semantics.remove_on_upgrade
            && source_format != Some(crate::ccs::native_lifecycle::SourceFormat::Deb)
        {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!(
                    "remove-on-upgrade config path {} requires signed Debian source authority",
                    config.path
                ),
                Some("kind.package.config.semantics.remove_on_upgrade".to_string()),
                "carry the exact Debian native lifecycle source contract",
            ));
        }

        let matching_files = data
            .files
            .iter()
            .filter(|file| file.path == config.path)
            .collect::<Vec<_>>();
        if semantics.ghost || semantics.remove_on_upgrade {
            if !matching_files.is_empty() {
                diagnostics.push(V3Diagnostic::error(
                    V3DiagnosticCode::KindContractViolation,
                    format!(
                        "config path {} must be absent from payload for its signed semantics",
                        config.path
                    ),
                    Some("kind.package.config".to_string()),
                    "remove ghost or remove-on-upgrade config paths from payload authority",
                ));
            }
            continue;
        }

        if matching_files.len() != 1 {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!(
                    "config path {} must identify exactly one signed payload file",
                    config.path
                ),
                Some("kind.package.config".to_string()),
                "attach the config declaration to exactly one payload file",
            ));
            continue;
        }
        let file = matching_files[0];
        if !matches!(
            file.node.kind,
            crate::payload::PayloadNodeKind::Regular { .. }
                | crate::payload::PayloadNodeKind::Symlink { .. }
        ) {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!(
                    "config path {} is not a regular file or symlink",
                    config.path
                ),
                Some("kind.package.config".to_string()),
                "declare configuration only for an installable config artifact",
            ));
        }
        if file.config != Some(semantics) {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!(
                    "config path {} semantics do not match signed file authority",
                    config.path
                ),
                Some("kind.package.files.config".to_string()),
                "write the same exact semantics into package and file authority",
            ));
        }
    }

    for file in &data.files {
        let Some(semantics) = file.config else {
            continue;
        };
        if config_by_path.get(file.path.as_str()) != Some(&semantics) {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!(
                    "file {} config semantics have no matching package authority",
                    file.path
                ),
                Some("kind.package.files.config".to_string()),
                "write one matching package config declaration",
            ));
        }
    }
}

fn validate_config_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/') {
        return Err(format!("config path must be absolute: {path}"));
    }
    let normalized = crate::filesystem::path::sanitize_path(path)
        .map_err(|error| format!("invalid config path {path}: {error}"))?;
    let canonical = format!("/{}", normalized.display());
    if path != canonical {
        return Err(format!(
            "config path must use canonical absolute spelling: {path}"
        ));
    }
    Ok(())
}
