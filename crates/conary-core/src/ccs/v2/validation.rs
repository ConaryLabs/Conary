// conary-core/src/ccs/v2/validation.rs

use super::diagnostics::{V2Diagnostic, V2DiagnosticCode, V2ValidationError};
use super::schema::*;

pub fn validate_authority(authority: &AuthorityDocumentV2) -> Result<(), V2ValidationError> {
    let mut diagnostics = Vec::new();

    if authority.format_version != FORMAT_VERSION_V2 {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::LegacyV1Package,
            format!(
                "unsupported CCS authority format {}",
                authority.format_version
            ),
            Some("format_version".to_string()),
            "rebuild or regenerate the package as CCS v2",
        ));
    }
    if authority.identity.name.trim().is_empty() {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::MissingAuthority,
            "v2 package identity name is required",
            Some("identity.name".to_string()),
            "set identity.name in signed v2 authority",
        ));
    }
    if authority.identity.version.trim().is_empty() {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::MissingAuthority,
            "v2 package identity version is required",
            Some("identity.version".to_string()),
            "set identity.version in signed v2 authority",
        ));
    }
    if authority.identity.release.trim().is_empty() {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::MissingAuthority,
            "v2 package identity release is required",
            Some("identity.release".to_string()),
            "set identity.release in signed v2 authority",
        ));
    }
    validate_provenance(&authority.provenance, &mut diagnostics);
    validate_dependencies("requires", &authority.requires, &mut diagnostics);
    validate_dependencies("provides", &authority.provides, &mut diagnostics);

    match (&authority.identity.kind, &authority.kind) {
        (PackageKindTagV2::Package, PackageKindV2::Package(data)) => {
            validate_component_defaults(authority, &mut diagnostics);
            if data.files.is_empty() {
                diagnostics.push(V2Diagnostic::error(
                    V2DiagnosticCode::MissingAuthority,
                    "v2 package kind requires at least one file authority entry",
                    Some("kind.package.files".to_string()),
                    "write file path/hash/component authority into v2 MANIFEST",
                ));
            }
            validate_files(data, authority, &mut diagnostics);
            validate_component_totals(data, authority, &mut diagnostics);
            validate_lifecycle(&authority.lifecycle, &mut diagnostics);
        }
        (PackageKindTagV2::Group, PackageKindV2::Group(data)) => {
            reject_group_redirect_payload_authority(authority, &mut diagnostics);
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
        if value.map_or(true, |value| value.trim().is_empty()) {
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

fn validate_dependencies(
    prefix: &str,
    entries: &[DependencyEntryV2],
    diagnostics: &mut Vec<V2Diagnostic>,
) {
    for entry in entries {
        if entry.name.trim().is_empty() {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::MissingAuthority,
                format!("{prefix} entry requires a name"),
                Some(format!("{prefix}.name")),
                "write typed dependency/provide name into signed v2 authority",
            ));
        }
    }
}

fn validate_files(
    data: &PackageDataV2,
    authority: &AuthorityDocumentV2,
    diagnostics: &mut Vec<V2Diagnostic>,
) {
    for file in &data.files {
        if file.path.trim().is_empty()
            || file.sha256.trim().is_empty()
            || file.component.trim().is_empty()
        {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::MissingAuthority,
                "v2 file authority requires path, sha256, size, and component",
                Some("kind.package.files".to_string()),
                "write complete file authority into signed v2 authority",
            ));
        }
        match file.file_type {
            FileTypeV2::Regular => {
                if file.symlink_target.is_some() {
                    diagnostics.push(V2Diagnostic::error(
                        V2DiagnosticCode::KindContractViolation,
                        format!("regular file {} must not carry symlink target", file.path),
                        Some("kind.package.files.symlink_target".to_string()),
                        "clear symlink_target for regular files",
                    ));
                }
            }
            FileTypeV2::Directory => {
                if file.size != 0 || file.symlink_target.is_some() {
                    diagnostics.push(V2Diagnostic::error(
                        V2DiagnosticCode::KindContractViolation,
                        format!(
                            "directory {} must have size 0 and no symlink target",
                            file.path
                        ),
                        Some("kind.package.files".to_string()),
                        "encode directory authority without blob size or symlink target",
                    ));
                }
            }
            FileTypeV2::Symlink => {
                if file.symlink_target.as_deref().map_or(true, str::is_empty) {
                    diagnostics.push(V2Diagnostic::error(
                        V2DiagnosticCode::MissingAuthority,
                        format!("symlink {} requires signed target authority", file.path),
                        Some("kind.package.files.symlink_target".to_string()),
                        "write symlink target into signed v2 authority",
                    ));
                }
            }
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
        let total_size: u64 = files.iter().map(|file| file.size).sum();
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
    // M4a accepts local user/group/directory/alternative declarations, but
    // profile-bound service/tmpfiles/sysctl checks must fail closed until M4d
    // provides target facts.
    if !lifecycle.services.is_empty()
        || !lifecycle.tmpfiles.is_empty()
        || !lifecycle.sysctl.is_empty()
    {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::LifecycleUnsupported,
            "v2 lifecycle services, tmpfiles, and sysctl declarations require target profile facts",
            Some("lifecycle".to_string()),
            "defer profile-bound lifecycle declarations until M4d target profiles are available",
        ));
    }
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
        AuthorityDocumentV2, DependencyEntryV2, GroupDataV2, PackageKindTagV2, PackageKindV2,
        RedirectDataV2,
    };

    #[test]
    fn rejects_missing_package_files_as_missing_authority() {
        let authority = AuthorityDocumentV2::empty_package_for_tests("empty-package");
        let error = validate_authority(&authority).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.code == V2DiagnosticCode::MissingAuthority)
        );
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
    fn dependency_entries_need_name() {
        let mut authority = AuthorityDocumentV2::package_for_tests("bad-dep");
        authority.requires.push(DependencyEntryV2::package(""));
        let error = validate_authority(&authority).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.field.as_deref() == Some("requires.name"))
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
    fn rejects_symlink_without_signed_target() {
        let mut authority = AuthorityDocumentV2::package_for_tests("bad-link");
        let PackageKindV2::Package(data) = &mut authority.kind else {
            panic!("fixture should be package");
        };
        data.files[0].file_type = FileTypeV2::Symlink;
        data.files[0].symlink_target = None;
        let error = validate_authority(&authority).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.field.as_deref() == Some("kind.package.files.symlink_target"))
        );
    }
}
