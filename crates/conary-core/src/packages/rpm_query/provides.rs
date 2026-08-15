// conary-core/src/packages/rpm_query/provides.rs

//! Typed provider acquisition from one exact installed RPM header.

use std::process::Command;

use rpm::DependencyFlags;
use tracing::debug;

use crate::error::{Error, Result};
use crate::packages::InstalledPackageIdentity;
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, ProvideVersionRelation, ProvidedCapability,
    RepositoryCapabilityKind, SourcePackageFormat,
};
use crate::repository::versioning::VersionScheme;

pub(super) const RPM_PROVIDE_RECORD_FORMAT: &str =
    "[%{PROVIDENAME}\x1e%{PROVIDEFLAGS:hex}\x1e%{PROVIDEVERSION}\x1f]";

/// Query one installed RPM package's exact identity and declared provides.
///
/// Human-rendered `rpm --provides` output combines the capability, relation,
/// and version into one string. Adoption needs the same typed fields used by
/// RPM artifact parsing, so query the parallel header arrays directly.
pub fn query_package_provides(
    identity: &InstalledPackageIdentity,
) -> Result<Vec<ProvidedCapability>> {
    let InstalledPackageIdentity::Rpm { selector, .. } = identity else {
        return Err(Error::ConfigError(
            "RPM provides query requires an RPM installed identity".to_string(),
        ));
    };
    debug!("Querying provides for RPM package: {selector}");

    let output = Command::new("rpm")
        .args(["-q", selector, "--queryformat", RPM_PROVIDE_RECORD_FORMAT])
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| Error::InitError(format!("Failed to run rpm: {error}")))?;
    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{selector}' not found in RPM database"
        )));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("rpm provide output is not UTF-8: {error}")))?;
    let provides = parse_rpm_provide_records(identity, &stdout)?;

    debug!(
        "Package {selector} provides {} typed capabilities",
        provides.len()
    );
    Ok(provides)
}

pub(super) fn parse_rpm_provide_records(
    identity: &InstalledPackageIdentity,
    output: &str,
) -> Result<Vec<ProvidedCapability>> {
    let InstalledPackageIdentity::Rpm { name, .. } = identity else {
        return Err(Error::ConfigError(
            "RPM provides parser requires an RPM installed identity".to_string(),
        ));
    };
    let mut provides = vec![ProvidedCapability {
        kind: RepositoryCapabilityKind::PackageName,
        name: name.clone(),
        version: Some(identity.version()),
        version_relation: Some(ProvideVersionRelation::Equal),
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::ExactIdentity,
    }];
    for (record_index, record) in output
        .split('\x1f')
        .filter(|record| !record.is_empty())
        .enumerate()
    {
        let parts = record.split('\x1e').collect::<Vec<_>>();
        if parts.len() != 3 || parts[0].is_empty() {
            return Err(Error::ParseError(format!(
                "RPM provide record {} is malformed",
                record_index + 1
            )));
        }
        let flags = u32::from_str_radix(parts[1], 16).map_err(|error| {
            Error::ParseError(format!(
                "RPM provide record {} has invalid hexadecimal flags {:?}: {error}",
                record_index + 1,
                parts[1]
            ))
        })?;
        let declared = crate::packages::rpm::decode_rpm_declared_capability(
            name,
            record_index,
            parts[0],
            parts[2],
            DependencyFlags::from_bits_retain(flags),
        )?;
        provides.push(ProvidedCapability {
            kind: declared.kind,
            name: declared.name,
            version: declared.version,
            version_relation: declared.version_relation,
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier: declared.architecture_qualifier,
            provenance: CapabilityProvenance::SourceDeclared {
                format: SourcePackageFormat::Rpm,
                record_index: declared.header_index,
            },
        });
    }
    for provide in &provides {
        provide.validate()?;
    }
    Ok(provides)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_provides_preserve_exact_capability_fields() {
        let identity = InstalledPackageIdentity::rpm(
            "dracut-108-6.fc44.x86_64",
            "dracut",
            None,
            "108",
            "6.fc44",
            "x86_64",
        )
        .unwrap();
        let output = concat!(
            "dracut\x1e8\x1e108-6.fc44\x1f",
            "config(dracut)\x1e10000008\x1e108-6.fc44\x1f",
            "/usr/bin/dracut\x1e0\x1e\x1f",
        );

        let provides = parse_rpm_provide_records(&identity, output).unwrap();

        assert_eq!(provides.len(), 4);
        assert_eq!(provides[0].name, "dracut");
        assert_eq!(provides[0].version.as_deref(), Some("108-6.fc44"));
        assert_eq!(provides[0].provenance, CapabilityProvenance::ExactIdentity);
        assert_eq!(provides[2].name, "config(dracut)");
        assert_eq!(provides[2].version.as_deref(), Some("108-6.fc44"));
        assert_eq!(
            provides[2].version_relation,
            Some(ProvideVersionRelation::Equal)
        );
        assert_eq!(provides[2].kind, RepositoryCapabilityKind::Generic);
    }

    #[test]
    fn installed_provides_reject_misaligned_header_arrays() {
        let identity = InstalledPackageIdentity::rpm(
            "dracut-108-6.fc44.x86_64",
            "dracut",
            None,
            "108",
            "6.fc44",
            "x86_64",
        )
        .unwrap();

        assert!(parse_rpm_provide_records(&identity, "dracut\x1e8\x1f").is_err());
        assert!(parse_rpm_provide_records(&identity, "\x1e8\x1e108\x1f").is_err());
        assert!(parse_rpm_provide_records(&identity, "dracut\x1enope\x1e108\x1f").is_err());
    }

    #[test]
    fn installed_provides_admit_rpm_dependency_boundary_characters() {
        let identity = InstalledPackageIdentity::rpm(
            "openSUSE-release-20260811-1.1.x86_64",
            "openSUSE-release",
            None,
            "20260811",
            "1.1",
            "x86_64",
        )
        .unwrap();
        let cpe = "cpe%3A%2Fo%3Aopensuse%3Aopensuse%3A20260811";
        let output = format!("product-cpeid()\x1e8\x1e{cpe}\x1f");

        let provides = parse_rpm_provide_records(&identity, &output).unwrap();

        assert_eq!(provides.len(), 2);
        assert_eq!(provides[1].name, "product-cpeid()");
        assert_eq!(provides[1].version.as_deref(), Some(cpe));
        assert_eq!(
            provides[1].version_relation,
            Some(ProvideVersionRelation::Equal)
        );
    }
}
