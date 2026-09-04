// crates/conary-core/src/ccs/v3/validation.rs

mod config;
mod content_layout;
mod file_capabilities;
mod identity;

use super::diagnostics::{V3Diagnostic, V3DiagnosticCode, V3ValidationError};
use super::schema::*;
use crate::ccs::budget::{AuthorityCensus, CCS_BUDGET};
use config::{validate_config_authority, validate_package_policy};
use content_layout::validate_file_content_layout;
use file_capabilities::validate_file_capabilities;
use identity::{validate_capabilities, validate_identity};

pub fn validate_authority(authority: &AuthorityDocumentV3) -> Result<(), V3ValidationError> {
    validate_authority_common(authority).map(|_| ())
}

pub fn validate_authority_structure(
    authority: &AuthorityDocumentV3,
) -> Result<(), V3ValidationError> {
    validate_authority_common(authority).map(|_| ())
}

/// Validate and return the structural census the shared budget measured.
///
/// Authoring uses the census to prove the bytes it is about to sign fit the
/// ceiling those exact bytes derive; verification uses it to prove the archived
/// bytes do. Neither side owns a private limit table.
pub fn authority_census(
    authority: &AuthorityDocumentV3,
) -> Result<AuthorityCensus, V3ValidationError> {
    validate_authority_common(authority)
}

fn validate_authority_common(
    authority: &AuthorityDocumentV3,
) -> Result<AuthorityCensus, V3ValidationError> {
    let mut diagnostics = Vec::new();

    // The shared structural budget runs first: a document that declares
    // hostile counts, lengths, or depth is refused before any other pass walks
    // it, and the same admission runs in authoring preflight and verification.
    let census = match CCS_BUDGET.admit_authority(authority) {
        Ok(census) => census,
        Err(error) => {
            return Err(V3ValidationError {
                diagnostics: vec![V3Diagnostic::error(
                    V3DiagnosticCode::StructuralBudgetExceeded,
                    error.to_string(),
                    Some(error.field.clone()),
                    "keep signed authority inside the canonical CCS structural budget",
                )],
            });
        }
    };

    if authority.format_version != FORMAT_VERSION_V3 {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::UnsupportedFormatVersion,
            format!(
                "unsupported CCS authority format {}",
                authority.format_version
            ),
            Some("format_version".to_string()),
            "rebuild or regenerate the package as CCS v3",
        ));
    }
    validate_identity(authority, &mut diagnostics);
    validate_provenance(&authority.provenance, &mut diagnostics);
    for (index, requirement) in authority.requirements.iter().enumerate() {
        if requirement.kind.is_negative_relation() {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                "negative relation stored in positive v3 requirements",
                Some(format!("requirements[{index}]")),
                "store conflicts, breaks, replacements, and obsoletes in relations",
            ));
            continue;
        }
        if let Err(error) = crate::repository::requirement::validate_requirement_group(
            requirement,
            authority.identity.version_scheme,
        ) {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!("invalid package requirement authority: {error}"),
                Some(format!("requirements[{index}]")),
                "encode an exact typed requirement using the package identity version scheme",
            ));
        }
    }
    validate_capabilities(authority, &mut diagnostics);
    for (index, relation) in authority.relations.iter().enumerate() {
        if let Err(error) = crate::repository::package_relation::validate_native_relation(
            relation,
            authority.identity.version_scheme,
        ) {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!("invalid package relation authority: {error}"),
                Some(format!("relations[{index}]")),
                "encode a typed relation using the package identity version scheme",
            ));
        }
    }
    if let Some(capabilities) = &authority.execution_capabilities
        && let Err(error) = capabilities.validate_for_target_arch(
            authority.identity.version_scheme,
            authority.identity.architecture.as_deref(),
        )
    {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::KindContractViolation,
            format!("invalid package capability authority: {error}"),
            Some("capabilities".to_string()),
            "encode a complete capability declaration for the package target architecture",
        ));
    }

    match (&authority.identity.kind, &authority.kind) {
        (PackageKindTagV3::Package, PackageKindV3::Package(data)) => {
            validate_component_defaults(authority, &mut diagnostics);
            validate_package_policy(&data.policy, "kind.package.policy", &mut diagnostics);
            validate_files(data, authority, &mut diagnostics);
            validate_file_capabilities(data, &authority.file_capabilities, &mut diagnostics);
            validate_config_authority(data, authority, &mut diagnostics);
            validate_component_totals(data, authority, &mut diagnostics);
            validate_lifecycle(&authority.lifecycle, &mut diagnostics);
            validate_repository_enrollment_files(data, authority, &mut diagnostics);
        }
        (PackageKindTagV3::Group, PackageKindV3::Group(data)) => {
            reject_group_redirect_payload_authority(authority, &mut diagnostics);
            validate_package_policy(&data.policy, "kind.group.policy", &mut diagnostics);
            if data.members.is_empty() {
                diagnostics.push(V3Diagnostic::error(
                    V3DiagnosticCode::KindContractViolation,
                    "v3 group packages require at least one member",
                    Some("kind.group.members".to_string()),
                    "add required or recommended group member requirements",
                ));
            }
        }
        (PackageKindTagV3::Redirect, PackageKindV3::Redirect(data)) => {
            reject_group_redirect_payload_authority(authority, &mut diagnostics);
            if data.to.trim().is_empty() {
                diagnostics.push(V3Diagnostic::error(
                    V3DiagnosticCode::KindContractViolation,
                    "v3 redirect packages require redirect.to",
                    Some("kind.redirect.to".to_string()),
                    "set redirect target package name",
                ));
            }
        }
        _ => diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::KindContractViolation,
            "v3 package kind tag does not match payload",
            Some("identity.kind".to_string()),
            "make identity.kind match the package/group/redirect payload",
        )),
    }

    if diagnostics.is_empty() {
        Ok(census)
    } else {
        Err(V3ValidationError { diagnostics })
    }
}

fn validate_repository_enrollment_files(
    package: &PackageDataV3,
    authority: &AuthorityDocumentV3,
    diagnostics: &mut Vec<V3Diagnostic>,
) {
    for intent in &authority.lifecycle.repository_enrollments {
        for projection in &intent.projections {
            let Some(file) = package.files.iter().find(|file| {
                file.path.trim_start_matches('/') == projection.path.trim_start_matches('/')
            }) else {
                diagnostics.push(V3Diagnostic::error(
                    V3DiagnosticCode::KindContractViolation,
                    format!(
                        "repository enrollment projection '{}' is absent from signed package files",
                        projection.path
                    ),
                    Some("lifecycle.repository_enrollments".to_string()),
                    "bind every repository projection to one exact signed regular file",
                ));
                continue;
            };
            let matches = file
                .content
                .as_ref()
                .is_some_and(|content| content.sha256 == projection.sha256)
                && file.node.mode & 0o7777 == projection.mode;
            if !matches {
                diagnostics.push(V3Diagnostic::error(
                    V3DiagnosticCode::KindContractViolation,
                    format!(
                        "repository enrollment projection '{}' disagrees with signed file authority",
                        projection.path
                    ),
                    Some("lifecycle.repository_enrollments".to_string()),
                    "copy the exact file digest and mode into the repository projection",
                ));
            }
        }
    }
}

fn validate_provenance(provenance: &ProvenanceAuthorityV3, diagnostics: &mut Vec<V3Diagnostic>) {
    for (field, value) in [
        (
            "provenance.origin_class",
            provenance.origin_class.as_deref(),
        ),
        (
            "provenance.hardening_level",
            provenance.hardening_level.as_deref(),
        ),
        (
            "provenance.build_input_identity",
            provenance.build_input_identity.as_deref(),
        ),
        (
            "provenance.hermetic_evidence_hash",
            provenance.hermetic_evidence_hash.as_deref(),
        ),
    ] {
        if value.is_none_or(|value| value.trim().is_empty()) {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::MissingAuthority,
                format!("v3 authority requires {field}"),
                Some(field.to_string()),
                "write complete provenance authority into signed v3 MANIFEST",
            ));
        }
    }
}

fn validate_component_defaults(
    authority: &AuthorityDocumentV3,
    diagnostics: &mut Vec<V3Diagnostic>,
) {
    let default_count = authority
        .components
        .values()
        .filter(|component| component.default)
        .count();
    if default_count != 1 {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::ComponentAuthorityMismatch,
            "v3 package authority requires exactly one default component",
            Some("components.default".to_string()),
            "mark one and only one component as default",
        ));
    }
}

fn validate_files(
    data: &PackageDataV3,
    authority: &AuthorityDocumentV3,
    diagnostics: &mut Vec<V3Diagnostic>,
) {
    let mut paths = std::collections::BTreeSet::new();
    for file in &data.files {
        if !paths.insert(file.path.as_str()) {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!("signed file path {} is declared more than once", file.path),
                Some("kind.package.files.path".to_string()),
                "declare one exact signed file authority record per package path",
            ));
        }
        if file.path.trim().is_empty() || file.component.trim().is_empty() {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::MissingAuthority,
                "v3 file authority requires path, payload node, and component",
                Some("kind.package.files".to_string()),
                "write complete file authority into signed v3 authority",
            ));
        }
        if let Err(error) = file.node.validate() {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!("payload node {} is invalid: {error}", file.path),
                Some("kind.package.files.node".to_string()),
                "write a complete exact POSIX payload node",
            ));
        }
        if let Err(error) = file.node.validate_content(file.content.as_ref()) {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!(
                    "payload content authority {} is invalid: {error}",
                    file.path
                ),
                Some("kind.package.files.content".to_string()),
                "attach digest and size only to regular payload nodes",
            ));
        }
        validate_file_content_layout(file, diagnostics);
        if !authority.components.contains_key(&file.component) {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::ComponentAuthorityMismatch,
                format!(
                    "file {} references unknown component {}",
                    file.path, file.component
                ),
                Some("kind.package.files.component".to_string()),
                "add matching component authority for every file component",
            ));
        }
        if file.conflict != ConflictPolicyV3::Error {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!(
                    "file {} requests conflict replacement that the installer does not implement",
                    file.path
                ),
                Some("kind.package.files.conflict".to_string()),
                "use error conflict policy until typed replacement is implemented end to end",
            ));
        }
    }
}

fn validate_component_totals(
    data: &PackageDataV3,
    authority: &AuthorityDocumentV3,
    diagnostics: &mut Vec<V3Diagnostic>,
) {
    for (name, component) in &authority.components {
        let files = data
            .files
            .iter()
            .filter(|file| file.component == *name)
            .collect::<Vec<_>>();
        let total_size: u64 = files
            .iter()
            .filter_map(|file| file.content.as_ref().map(|content| content.size))
            .sum();
        if component.file_count as usize != files.len() || component.total_size != total_size {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::ComponentAuthorityMismatch,
                format!("component {name} count or size does not match signed file authority"),
                Some("components".to_string()),
                "make component file_count and total_size match package file authority",
            ));
        }
    }
}

fn validate_lifecycle(lifecycle: &LifecycleAuthorityV3, diagnostics: &mut Vec<V3Diagnostic>) {
    use crate::ccs::hooks::{
        is_denied_sysctl_key, is_safe_declarative_unit_name, validate_shell, validate_sysctl_key,
        validate_sysctl_value, validate_tmpfiles_fields, validate_username,
    };

    let mut invalid = |field: &str, message: String| {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::KindContractViolation,
            message,
            Some(format!("lifecycle.{field}")),
            "write a complete lifecycle value that satisfies the signed v3 grammar",
        ));
    };

    if let Some(native_lifecycle) = &lifecycle.native_lifecycle
        && let Err(error) = native_lifecycle.validate()
    {
        invalid(
            "native_lifecycle",
            format!("invalid native lifecycle authority: {error}"),
        );
    }

    for enrollment in &lifecycle.repository_enrollments {
        if let Err(error) = enrollment.validate() {
            invalid(
                "repository_enrollments",
                format!("invalid package repository enrollment authority: {error}"),
            );
        }
    }

    for user in &lifecycle.users {
        if let Err(error) = validate_username(&user.name) {
            invalid("users.name", format!("invalid lifecycle user: {error}"));
        }
        if !user.system {
            invalid(
                "users.system",
                format!("lifecycle user {} must be a system user", user.name),
            );
        }
        if let Some(group) = &user.group
            && let Err(error) = validate_username(group)
        {
            invalid(
                "users.group",
                format!("invalid lifecycle user group: {error}"),
            );
        }
        if let Some(shell) = &user.shell
            && let Err(error) = validate_shell(shell)
        {
            invalid(
                "users.shell",
                format!("invalid lifecycle user shell: {error}"),
            );
        }
        if let Some(home) = &user.home
            && let Err(error) = validate_absolute_path(home)
        {
            invalid("users.home", error);
        }
    }
    for group in &lifecycle.groups {
        if let Err(error) = validate_username(&group.name) {
            invalid("groups.name", format!("invalid lifecycle group: {error}"));
        }
        if !group.system {
            invalid(
                "groups.system",
                format!("lifecycle group {} must be a system group", group.name),
            );
        }
    }
    for directory in &lifecycle.directories {
        if let Err(error) = validate_absolute_path(&directory.path) {
            invalid("directories.path", error);
        }
        if let Err(error) = validate_mode(&directory.mode) {
            invalid("directories.mode", error);
        }
        if directory.owner.trim().is_empty() || directory.group.trim().is_empty() {
            invalid(
                "directories",
                format!(
                    "lifecycle directory {} requires owner and group",
                    directory.path
                ),
            );
        }
    }
    for service in &lifecycle.services {
        if !is_safe_declarative_unit_name(&service.name) {
            invalid(
                "services.name",
                format!(
                    "lifecycle service name must be pathless, nonempty, and contain no NUL: {}",
                    service.name
                ),
            );
        }
    }
    for unit in &lifecycle.systemd {
        if !is_safe_declarative_unit_name(&unit.unit) {
            invalid(
                "systemd.unit",
                format!(
                    "lifecycle systemd unit must be pathless, nonempty, and contain no NUL: {}",
                    unit.unit
                ),
            );
        }
    }
    for tmpfiles in &lifecycle.tmpfiles {
        if let Err(error) = validate_tmpfiles_fields(
            &tmpfiles.entry_type,
            &tmpfiles.path,
            &tmpfiles.mode,
            &tmpfiles.user,
            &tmpfiles.group,
            &tmpfiles.age,
            &tmpfiles.argument,
        ) {
            invalid(
                "tmpfiles",
                format!(
                    "invalid lifecycle tmpfiles entry {}: {error}",
                    tmpfiles.path
                ),
            );
        }
    }
    for sysctl in &lifecycle.sysctl {
        if let Err(error) = validate_sysctl_key(&sysctl.key) {
            invalid(
                "sysctl.key",
                format!("invalid lifecycle sysctl key: {error}"),
            );
        } else if is_denied_sysctl_key(&sysctl.key) {
            invalid(
                "sysctl.key",
                format!("lifecycle sysctl key {} is denied", sysctl.key),
            );
        }
        if let Err(error) = validate_sysctl_value(&sysctl.value) {
            invalid(
                "sysctl.value",
                format!("invalid lifecycle sysctl value: {error}"),
            );
        }
    }
    for alternative in &lifecycle.alternatives {
        if alternative.name.trim().is_empty()
            || alternative.name.contains('/')
            || alternative.name.contains('\\')
        {
            invalid(
                "alternatives.name",
                format!("invalid lifecycle alternative name {}", alternative.name),
            );
        }
        if let Err(error) = validate_absolute_path(&alternative.path) {
            invalid("alternatives.path", error);
        }
    }
    validate_script_capabilities(
        "script_capabilities",
        &lifecycle.script_capabilities,
        &mut invalid,
    );
    for (field, script) in [
        ("post_install", lifecycle.post_install.as_ref()),
        ("pre_remove", lifecycle.pre_remove.as_ref()),
    ] {
        let Some(script) = script else {
            continue;
        };
        if script.interpreter != "/bin/sh" {
            invalid(
                &format!("{field}.interpreter"),
                format!(
                    "lifecycle script interpreter {} is not implemented by the CCS hook executor",
                    script.interpreter
                ),
            );
        }
        if script.body.trim().is_empty() {
            invalid(
                &format!("{field}.body"),
                "lifecycle script body must not be empty".to_string(),
            );
        }
        validate_script_capabilities(
            &format!("{field}.capabilities"),
            &script.capabilities,
            &mut invalid,
        );
        if script.capabilities != lifecycle.script_capabilities {
            invalid(
                &format!("{field}.capabilities"),
                format!(
                    "{field} lifecycle capabilities differ from the exact package-scoped hook capability set"
                ),
            );
        }
    }
}

fn validate_script_capabilities(
    field: &str,
    capabilities: &[LifecycleScriptCapabilityV3],
    invalid: &mut impl FnMut(&str, String),
) {
    for capability in capabilities {
        if capability.name.trim().is_empty() {
            invalid(
                &format!("{field}.name"),
                "lifecycle script capability name must not be empty".to_string(),
            );
        }
        for path in &capability.paths {
            if let Err(error) = validate_absolute_path(path) {
                invalid(&format!("{field}.paths"), error);
            }
        }
    }
}

fn validate_absolute_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/') {
        return Err(format!("lifecycle path must be absolute: {path}"));
    }
    crate::filesystem::path::sanitize_path(path)
        .map(|_| ())
        .map_err(|error| format!("invalid lifecycle path {path}: {error}"))
}

fn validate_mode(mode: &str) -> Result<(), String> {
    if mode.len() != 4
        || !mode.starts_with('0')
        || !mode.bytes().all(|byte| matches!(byte, b'0'..=b'7'))
    {
        return Err(format!(
            "lifecycle mode must use four-digit octal notation: {mode}"
        ));
    }
    Ok(())
}

fn reject_group_redirect_payload_authority(
    authority: &AuthorityDocumentV3,
    diagnostics: &mut Vec<V3Diagnostic>,
) {
    if !authority.components.is_empty()
        || !authority.file_capabilities.is_empty()
        || authority.lifecycle != LifecycleAuthorityV3::default()
    {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::KindContractViolation,
            "v3 group and redirect packages must not carry file, file-capability, component, or lifecycle payload authority",
            Some("components".to_string()),
            "move file/lifecycle authority to package kind payloads only",
        ));
    }
}

#[cfg(test)]
mod tests;
