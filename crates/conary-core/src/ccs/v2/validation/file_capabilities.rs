// conary-core/src/ccs/v2/validation/file_capabilities.rs

//! Signed Linux file-capability validation against exact package payload authority.

use super::super::diagnostics::{V2Diagnostic, V2DiagnosticCode};
use super::super::schema::PackageDataV2;
use crate::ccs::manifest::FileCapability;
use crate::payload::PayloadNodeKind;

pub(super) fn validate_file_capabilities(
    data: &PackageDataV2,
    capabilities: &[FileCapability],
    diagnostics: &mut Vec<V2Diagnostic>,
) {
    match super::super::file_capabilities::canonicalize_file_capabilities(capabilities) {
        Ok(canonical) if canonical != capabilities => diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            "signed file capability authority is not in canonical path and capability-name order",
            Some("file_capabilities".to_string()),
            "sort declarations by path and each declaration's Linux capability names",
        )),
        Err(error) => diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            format!("invalid signed file capability authority: {error}"),
            Some("file_capabilities".to_string()),
            "declare each valid Linux file capability path exactly once",
        )),
        Ok(_) => {}
    }

    for (index, capability) in capabilities.iter().enumerate() {
        let field = format!("file_capabilities[{index}].path");
        let Some(file) = data.files.iter().find(|file| file.path == capability.path) else {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                format!(
                    "file capability target {} has no matching signed package file",
                    capability.path
                ),
                Some(field),
                "bind file capabilities only to exact signed package file paths",
            ));
            continue;
        };
        if !matches!(file.node.kind, PayloadNodeKind::Regular { .. }) {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                format!(
                    "file capability target {} is not a regular signed package file",
                    capability.path
                ),
                Some(field),
                "bind Linux file capabilities only to regular signed payload files",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::v2::diagnostics::V2DiagnosticCode;
    use crate::ccs::v2::schema::{AuthorityDocumentV2, PackageKindV2};
    use crate::ccs::v2::validation::validate_authority;

    fn file_capability(path: &str, names: &[&str]) -> FileCapability {
        FileCapability {
            path: path.to_string(),
            capabilities: names.iter().map(|name| (*name).to_string()).collect(),
            permitted: true,
            effective: true,
            inheritable: false,
        }
    }

    #[test]
    fn accepts_canonical_capability_bound_to_regular_signed_file() {
        let mut authority = AuthorityDocumentV2::package_for_tests("capable");
        authority.file_capabilities =
            vec![file_capability("/usr/bin/hello", &["cap_net_bind_service"])];

        validate_authority(&authority).unwrap();
    }

    #[test]
    fn rejects_missing_and_non_regular_signed_targets() {
        let mut missing = AuthorityDocumentV2::package_for_tests("missing-target");
        missing.file_capabilities = vec![file_capability(
            "/usr/bin/missing",
            &["cap_net_bind_service"],
        )];
        let error = validate_authority(&missing).unwrap_err();
        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.field.as_deref() == Some("file_capabilities[0].path")
                && diagnostic
                    .message
                    .contains("no matching signed package file")
        }));

        let mut symlink = AuthorityDocumentV2::package_for_tests("symlink-target");
        let PackageKindV2::Package(data) = &mut symlink.kind else {
            panic!("fixture should be a package");
        };
        data.files[0].node.kind = PayloadNodeKind::Symlink {
            target: "hello-real".to_string(),
        };
        data.files[0].node.mode = libc::S_IFLNK | 0o777;
        data.files[0].content = None;
        symlink.file_capabilities =
            vec![file_capability("/usr/bin/hello", &["cap_net_bind_service"])];
        let error = validate_authority(&symlink).unwrap_err();
        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.field.as_deref() == Some("file_capabilities[0].path")
                && diagnostic
                    .message
                    .contains("not a regular signed package file")
        }));
    }

    #[test]
    fn rejects_duplicate_paths_and_noncanonical_capability_names() {
        let declaration = file_capability("/usr/bin/hello", &["cap_net_bind_service"]);
        let mut duplicate = AuthorityDocumentV2::package_for_tests("duplicate");
        duplicate.file_capabilities = vec![declaration.clone(), declaration];
        let error = validate_authority(&duplicate).unwrap_err();
        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == V2DiagnosticCode::KindContractViolation
                && diagnostic.message.contains("declared more than once")
        }));

        let mut unordered = AuthorityDocumentV2::package_for_tests("unordered");
        unordered.file_capabilities = vec![file_capability(
            "/usr/bin/hello",
            &["cap_net_raw", "cap_net_bind_service"],
        )];
        let error = validate_authority(&unordered).unwrap_err();
        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.field.as_deref() == Some("file_capabilities")
                && diagnostic.message.contains("canonical")
        }));
    }
}
