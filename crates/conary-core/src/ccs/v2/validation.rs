// conary-core/src/ccs/v2/validation.rs

mod config;
mod identity;

use super::diagnostics::{V2Diagnostic, V2DiagnosticCode, V2ValidationError};
use super::schema::*;
use config::{validate_config_authority, validate_package_policy};
use identity::{validate_identity, validate_provides};

pub fn validate_authority(authority: &AuthorityDocumentV2) -> Result<(), V2ValidationError> {
    validate_authority_common(authority)
}

pub fn validate_authority_structure(
    authority: &AuthorityDocumentV2,
) -> Result<(), V2ValidationError> {
    validate_authority_common(authority)
}

fn validate_authority_common(authority: &AuthorityDocumentV2) -> Result<(), V2ValidationError> {
    let mut diagnostics = Vec::new();

    if authority.format_version != FORMAT_VERSION_V2 {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::UnsupportedFormatVersion,
            format!(
                "unsupported CCS authority format {}",
                authority.format_version
            ),
            Some("format_version".to_string()),
            "rebuild or regenerate the package as CCS v2",
        ));
    }
    validate_identity(authority, &mut diagnostics);
    validate_provenance(&authority.provenance, &mut diagnostics);
    for (index, requirement) in authority.requirements.iter().enumerate() {
        if requirement.kind.is_negative_relation() {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                "negative relation stored in positive v2 requirements",
                Some(format!("requirements[{index}]")),
                "store conflicts, breaks, replacements, and obsoletes in relations",
            ));
            continue;
        }
        if let Err(error) = crate::repository::requirement::validate_requirement_group(
            requirement,
            authority.identity.version_scheme,
        ) {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                format!("invalid package requirement authority: {error}"),
                Some(format!("requirements[{index}]")),
                "encode an exact typed requirement using the package identity version scheme",
            ));
        }
    }
    validate_provides(authority, &mut diagnostics);
    for (index, relation) in authority.relations.iter().enumerate() {
        if let Err(error) = crate::repository::package_relation::validate_native_relation(
            relation,
            authority.identity.version_scheme,
        ) {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                format!("invalid package relation authority: {error}"),
                Some(format!("relations[{index}]")),
                "encode a typed relation using the package identity version scheme",
            ));
        }
    }
    if let Some(capabilities) = &authority.capabilities
        && let Err(error) = capabilities.validate_for_target_arch(
            authority.identity.version_scheme,
            authority.identity.architecture.as_deref(),
        )
    {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            format!("invalid package capability authority: {error}"),
            Some("capabilities".to_string()),
            "encode a complete capability declaration for the package target architecture",
        ));
    }

    match (&authority.identity.kind, &authority.kind) {
        (PackageKindTagV2::Package, PackageKindV2::Package(data)) => {
            validate_component_defaults(authority, &mut diagnostics);
            validate_package_policy(&data.policy, "kind.package.policy", &mut diagnostics);
            validate_files(data, authority, &mut diagnostics);
            validate_config_authority(data, authority, &mut diagnostics);
            validate_component_totals(data, authority, &mut diagnostics);
            validate_lifecycle(&authority.lifecycle, &mut diagnostics);
        }
        (PackageKindTagV2::Group, PackageKindV2::Group(data)) => {
            reject_group_redirect_payload_authority(authority, &mut diagnostics);
            validate_package_policy(&data.policy, "kind.group.policy", &mut diagnostics);
            if data.members.is_empty() {
                diagnostics.push(V2Diagnostic::error(
                    V2DiagnosticCode::KindContractViolation,
                    "v2 group packages require at least one member",
                    Some("kind.group.members".to_string()),
                    "add required or recommended group member requirements",
                ));
            }
        }
        (PackageKindTagV2::Redirect, PackageKindV2::Redirect(data)) => {
            reject_group_redirect_payload_authority(authority, &mut diagnostics);
            if data.to.trim().is_empty() {
                diagnostics.push(V2Diagnostic::error(
                    V2DiagnosticCode::KindContractViolation,
                    "v2 redirect packages require redirect.to",
                    Some("kind.redirect.to".to_string()),
                    "set redirect target package name",
                ));
            }
        }
        _ => diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            "v2 package kind tag does not match payload",
            Some("identity.kind".to_string()),
            "make identity.kind match the package/group/redirect payload",
        )),
    }

    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(V2ValidationError { diagnostics })
    }
}

fn validate_provenance(provenance: &ProvenanceAuthorityV2, diagnostics: &mut Vec<V2Diagnostic>) {
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
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::MissingAuthority,
                format!("v2 authority requires {field}"),
                Some(field.to_string()),
                "write complete provenance authority into signed v2 MANIFEST",
            ));
        }
    }
}

fn validate_component_defaults(
    authority: &AuthorityDocumentV2,
    diagnostics: &mut Vec<V2Diagnostic>,
) {
    let default_count = authority
        .components
        .values()
        .filter(|component| component.default)
        .count();
    if default_count != 1 {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::ComponentAuthorityMismatch,
            "v2 package authority requires exactly one default component",
            Some("components.default".to_string()),
            "mark one and only one component as default",
        ));
    }
}

fn validate_files(
    data: &PackageDataV2,
    authority: &AuthorityDocumentV2,
    diagnostics: &mut Vec<V2Diagnostic>,
) {
    for file in &data.files {
        if file.path.trim().is_empty() || file.component.trim().is_empty() {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::MissingAuthority,
                "v2 file authority requires path, payload node, and component",
                Some("kind.package.files".to_string()),
                "write complete file authority into signed v2 authority",
            ));
        }
        if let Err(error) = file.node.validate() {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                format!("payload node {} is invalid: {error}", file.path),
                Some("kind.package.files.node".to_string()),
                "write a complete exact POSIX payload node",
            ));
        }
        if let Err(error) = file.node.validate_content(file.content.as_ref()) {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                format!(
                    "payload content authority {} is invalid: {error}",
                    file.path
                ),
                Some("kind.package.files.content".to_string()),
                "attach digest and size only to regular payload nodes",
            ));
        }
        if !authority.components.contains_key(&file.component) {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::ComponentAuthorityMismatch,
                format!(
                    "file {} references unknown component {}",
                    file.path, file.component
                ),
                Some("kind.package.files.component".to_string()),
                "add matching component authority for every file component",
            ));
        }
        if file.conflict != ConflictPolicyV2::Error {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
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
    data: &PackageDataV2,
    authority: &AuthorityDocumentV2,
    diagnostics: &mut Vec<V2Diagnostic>,
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
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::ComponentAuthorityMismatch,
                format!("component {name} count or size does not match signed file authority"),
                Some("components".to_string()),
                "make component file_count and total_size match package file authority",
            ));
        }
    }
}

fn validate_lifecycle(lifecycle: &LifecycleAuthorityV2, diagnostics: &mut Vec<V2Diagnostic>) {
    use crate::ccs::hooks::{
        is_denied_sysctl_key, is_safe_declarative_unit_name, validate_shell, validate_sysctl_key,
        validate_sysctl_value, validate_tmpfiles_fields, validate_username,
    };

    let mut invalid = |field: &str, message: String| {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            message,
            Some(format!("lifecycle.{field}")),
            "write a complete lifecycle value that satisfies the signed v2 grammar",
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
    capabilities: &[LifecycleScriptCapabilityV2],
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
    authority: &AuthorityDocumentV2,
    diagnostics: &mut Vec<V2Diagnostic>,
) {
    if !authority.components.is_empty() || authority.lifecycle != LifecycleAuthorityV2::default() {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            "v2 group and redirect packages must not carry file components or lifecycle payload authority",
            Some("components".to_string()),
            "move file/lifecycle authority to package kind payloads only",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::v2::diagnostics::V2DiagnosticCode;
    use crate::ccs::v2::schema::{
        AuthorityDocumentV2, GroupDataV2, PackageKindTagV2, PackageKindV2, RedirectDataV2,
    };

    #[test]
    fn accepts_payloadless_package_with_explicit_empty_component_authority() {
        let mut authority = AuthorityDocumentV2::empty_package_for_tests("empty-package");
        authority.components.insert(
            "runtime".to_string(),
            crate::ccs::v2::schema::ComponentAuthorityV2 {
                name: "runtime".to_string(),
                default: true,
                file_count: 0,
                total_size: 0,
            },
        );
        validate_authority(&authority).unwrap();
    }

    #[test]
    fn rejects_payloadless_package_without_default_component_authority() {
        let authority = AuthorityDocumentV2::empty_package_for_tests("empty-package");
        let error = validate_authority(&authority).unwrap_err();
        assert!(
            error.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == V2DiagnosticCode::ComponentAuthorityMismatch
            })
        );
    }

    #[test]
    fn rejects_invalid_package_capability_authority() {
        let mut authority = AuthorityDocumentV2::package_for_tests("invalid-capabilities");
        authority.capabilities = Some(crate::capability::CapabilityDeclaration {
            version: crate::capability::CAPABILITY_SCHEMA_VERSION + 1,
            ..Default::default()
        });

        let error = validate_authority(&authority).unwrap_err();

        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == V2DiagnosticCode::KindContractViolation
                && diagnostic.field.as_deref() == Some("capabilities")
        }));
    }

    #[test]
    fn rejects_group_without_members() {
        let mut authority = AuthorityDocumentV2::empty_package_for_tests("empty-group");
        authority.identity.kind = PackageKindTagV2::Group;
        authority.kind = PackageKindV2::Group(GroupDataV2 {
            members: Vec::new(),
            provides: Vec::new(),
            conflicts: Vec::new(),
            policy: Default::default(),
        });
        let error = validate_authority(&authority).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.code == V2DiagnosticCode::KindContractViolation)
        );
    }

    #[test]
    fn accepts_redirect_with_target() {
        let mut authority = AuthorityDocumentV2::empty_package_for_tests("old-name");
        authority.identity.kind = PackageKindTagV2::Redirect;
        authority.kind = PackageKindV2::Redirect(RedirectDataV2 {
            to: "new-name".to_string(),
            version_constraint: None,
            reason: Some("renamed".to_string()),
        });
        validate_authority(&authority).unwrap();
    }

    #[test]
    fn rejects_kind_tag_payload_mismatch() {
        let mut authority = AuthorityDocumentV2::package_for_tests("mismatch");
        authority.identity.kind = PackageKindTagV2::Group;
        let error = validate_authority(&authority).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.code == V2DiagnosticCode::KindContractViolation)
        );
    }

    #[test]
    fn requirement_groups_need_valid_atoms() {
        let mut authority = AuthorityDocumentV2::package_for_tests("bad-dep");
        authority.requirements.push(
            crate::repository::dependency_model::RepositoryRequirementGroup::simple(
                crate::repository::dependency_model::RepositoryRequirementKind::Depends,
                crate::repository::dependency_model::RepositoryRequirementClause::name_only(
                    String::new(),
                ),
            ),
        );
        let error = validate_authority(&authority).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.field.as_deref() == Some("requirements[0]"))
        );
    }

    #[test]
    fn rejects_incomplete_identity_provenance_and_component_totals() {
        let mut authority = AuthorityDocumentV2::package_for_tests("bad-authority");
        authority.identity.release.clear();
        authority.provenance.origin_class = None;
        authority.provenance.hardening_level = None;
        authority.components.get_mut("main").unwrap().file_count = 2;
        let error = validate_authority(&authority).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.field.as_deref() == Some("identity.release"))
        );
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.field.as_deref() == Some("provenance.origin_class"))
        );
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.code == V2DiagnosticCode::ComponentAuthorityMismatch)
        );
    }

    #[test]
    fn rejects_symlink_with_invalid_signed_target() {
        let mut authority = AuthorityDocumentV2::package_for_tests("bad-link");
        let PackageKindV2::Package(data) = &mut authority.kind else {
            panic!("fixture should be package");
        };
        data.files[0].node.kind = crate::payload::PayloadNodeKind::Symlink {
            target: String::new(),
        };
        data.files[0].node.mode = libc::S_IFLNK | 0o777;
        data.files[0].content = None;
        let error = validate_authority(&authority).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.field.as_deref() == Some("kind.package.files.node"))
        );
    }

    #[test]
    fn rejects_unimplemented_signed_file_conflict_replacement() {
        let mut authority = AuthorityDocumentV2::package_for_tests("replace-conflict");
        let PackageKindV2::Package(data) = &mut authority.kind else {
            panic!("fixture should be package");
        };
        data.files[0].conflict = ConflictPolicyV2::Replace;

        let error = validate_authority(&authority).unwrap_err();

        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.field.as_deref() == Some("kind.package.files.conflict")
        }));
    }

    #[test]
    fn rejects_unimplemented_signed_host_mutation_policy() {
        let mut authority = AuthorityDocumentV2::package_for_tests("host-mutation");
        let PackageKindV2::Package(data) = &mut authority.kind else {
            panic!("fixture should be package");
        };
        data.policy.allow_host_mutation = true;

        let error = validate_authority(&authority).unwrap_err();

        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.field.as_deref() == Some("kind.package.policy.allow_host_mutation")
        }));
    }

    #[test]
    fn config_package_and_file_authority_must_match_exactly() {
        let mut authority = AuthorityDocumentV2::package_for_tests("config-mirror");
        let PackageKindV2::Package(data) = &mut authority.kind else {
            panic!("fixture should be package");
        };
        let semantics = ConfigSemanticsV2 {
            noreplace: true,
            ghost: false,
            remove_on_upgrade: false,
        };
        data.files[0].config = Some(semantics);
        data.config.push(ConfigAuthorityV2 {
            path: data.files[0].path.clone(),
            semantics,
        });
        validate_authority(&authority).unwrap();

        let PackageKindV2::Package(data) = &mut authority.kind else {
            unreachable!();
        };
        data.files[0].config.as_mut().unwrap().noreplace = false;
        let error = validate_authority(&authority).unwrap_err();

        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.field.as_deref() == Some("kind.package.files.config")
        }));
    }

    #[test]
    fn rejects_noncanonical_signed_config_path() {
        let mut authority = AuthorityDocumentV2::package_for_tests("config-path");
        let PackageKindV2::Package(data) = &mut authority.kind else {
            panic!("fixture should be package");
        };
        let semantics = ConfigSemanticsV2 {
            noreplace: true,
            ghost: false,
            remove_on_upgrade: false,
        };
        data.files[0].path = "/usr//bin/hello".to_string();
        data.files[0].config = Some(semantics);
        data.config.push(ConfigAuthorityV2 {
            path: data.files[0].path.clone(),
            semantics,
        });

        let error = validate_authority(&authority).unwrap_err();

        assert!(
            error.diagnostics.iter().any(|diagnostic| {
                diagnostic.field.as_deref() == Some("kind.package.config.path")
            })
        );
    }

    #[test]
    fn lifecycle_authority_is_source_independent_structural_data() {
        let mut authority = AuthorityDocumentV2::package_for_tests("lifecycle");
        authority.lifecycle.services = vec![LifecycleServiceV2 {
            name: "example".to_string(),
            action: LifecycleServiceActionV2::Restart,
            reversible: Some(false),
        }];
        authority.lifecycle.systemd = vec![LifecycleSystemdV2 {
            unit: "example.service".to_string(),
            enable: true,
            reversible: Some(true),
        }];
        authority.lifecycle.tmpfiles = vec![LifecycleTmpfilesV2 {
            entry_type: "C+!$".to_string(),
            path: "/var/lib/example".to_string(),
            mode: "-".to_string(),
            user: "-".to_string(),
            group: "example-group".to_string(),
            age: "~30d".to_string(),
            argument: "/usr/share/example seed".to_string(),
            reversible: Some(true),
        }];
        authority.lifecycle.sysctl = vec![LifecycleSysctlV2 {
            key: "net.ipv4.ip_forward".to_string(),
            value: "1".to_string(),
            reversible: Some(false),
        }];
        authority.lifecycle.users = vec![LifecycleUserV2 {
            name: "example-user".to_string(),
            system: true,
            home: Some("/var/lib/example".to_string()),
            shell: Some("/usr/sbin/nologin".to_string()),
            group: Some("example-group".to_string()),
            reversible: Some(true),
        }];
        authority.lifecycle.groups = vec![LifecycleGroupV2 {
            name: "example-group".to_string(),
            system: true,
            reversible: Some(true),
        }];
        authority.lifecycle.directories = vec![LifecycleDirectoryV2 {
            path: "/var/lib/example".to_string(),
            mode: "0750".to_string(),
            owner: "example-user".to_string(),
            group: "example-group".to_string(),
            cleanup: Some("30d".to_string()),
            reversible: Some(true),
        }];
        authority.lifecycle.alternatives = vec![LifecycleAlternativeV2 {
            link: "/usr/bin/example".to_string(),
            name: "example".to_string(),
            path: "/usr/bin/example-v2".to_string(),
            priority: 70,
            reversible: Some(true),
        }];
        authority.lifecycle.script_capabilities = vec![LifecycleScriptCapabilityV2 {
            name: "systemd-service-registration".to_string(),
            paths: vec!["/etc/systemd/system".to_string()],
        }];
        authority.lifecycle.post_install = Some(LifecycleScriptV2 {
            interpreter: "/bin/sh".to_string(),
            body: "printf installed".to_string(),
            capabilities: authority.lifecycle.script_capabilities.clone(),
            reversible: Some(false),
            execution: LifecycleScriptExecutionV2::SandboxedTargetRoot,
        });

        validate_authority(&authority).unwrap();
    }

    #[test]
    fn rejects_script_contract_the_hook_executor_cannot_honor() {
        let mut authority = AuthorityDocumentV2::package_for_tests("lifecycle");
        authority.lifecycle.post_install = Some(LifecycleScriptV2 {
            interpreter: "/usr/bin/python3".to_string(),
            body: "print('installed')".to_string(),
            capabilities: Vec::new(),
            reversible: None,
            execution: LifecycleScriptExecutionV2::SandboxedTargetRoot,
        });

        let error = validate_authority(&authority).unwrap_err();
        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.field.as_deref() == Some("lifecycle.post_install.interpreter")
        }));
    }
}
