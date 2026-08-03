// conary-core/src/repository/parsers/fedora/provides.rs

//! Exact provider-role projection from authenticated RPM primary metadata.

use super::relation::RpmProvideConstraint;
use crate::error::{Error, Result};
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, RepositoryCapabilityKind,
    RepositoryProvide, SourcePackageFormat,
};

pub(super) fn project_repository_provides(
    package_name: &str,
    package_version: &str,
    declared: &[(String, RpmProvideConstraint)],
    primary_files: &[String],
) -> Result<Vec<RepositoryProvide>> {
    let mut projected = vec![RepositoryProvide::package_name(
        package_name.to_string(),
        Some(package_version.to_string()),
    )];

    for (record_index, (name, provide)) in declared.iter().enumerate() {
        if crate::repository::rpm_runtime::RpmRuntimeFeature::parse_capability(name)
            .map_err(|error| Error::ParseError(error.to_string()))?
            .is_some()
        {
            return Err(Error::ParseError(format!(
                "RPM repository package cannot provide package-manager runtime capability '{name}'; Conary's typed runtime ledger is the sole authority"
            )));
        }
        let kind = if name == package_name {
            RepositoryCapabilityKind::PackageName
        } else {
            // RPM primary metadata does not type paths or sonames separately.
            RepositoryCapabilityKind::Generic
        };
        let native_text = if provide.native_constraint.is_empty() {
            name.clone()
        } else {
            format!("{name} {}", provide.native_constraint)
        };
        projected.push(RepositoryProvide {
            name: name.clone(),
            kind,
            version: provide.version.clone(),
            version_relation: provide.version_relation,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            native_text: Some(native_text),
            provenance: CapabilityProvenance::SourceDeclared {
                format: SourcePackageFormat::Rpm,
                record_index: u32::try_from(record_index).map_err(|_| {
                    Error::ParseError("RPM repository provide index exceeds u32".to_string())
                })?,
            },
        });
    }

    projected.extend(primary_files.iter().cloned().map(|path| RepositoryProvide {
        provenance: CapabilityProvenance::SourceDerivedFile {
            format: SourcePackageFormat::Rpm,
            source_path: path.clone(),
        },
        ..RepositoryProvide::file(path)
    }));
    Ok(projected)
}
