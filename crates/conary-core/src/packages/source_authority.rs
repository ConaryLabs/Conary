// conary-core/src/packages/source_authority.rs

//! Closed source-package identity and declared-capability authority.

use crate::error::{Error, Result};
use crate::packages::arch::authority::AlpmPackageAuthority;
use crate::packages::config_authority::SourceConfigDeclaration;
use crate::packages::deb::authority::DebianPackageAuthority;
use crate::packages::rpm::authority::RpmPackageAuthority;
use crate::repository::dependency_model::{
    CapabilityProvenance, DebianMultiArch, ProvideArchitectureQualifier, ProvideVersionRelation,
    ProvidedCapability, RepositoryCapabilityKind, SourcePackageFormat,
};
use crate::repository::versioning::VersionScheme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcsPackageAuthority {
    pub name: String,
    pub version: String,
    pub version_scheme: VersionScheme,
    pub architecture: Option<String>,
    pub debian_multi_arch: Option<DebianMultiArch>,
    pub capabilities: Vec<ProvidedCapability>,
    pub config: Vec<SourceConfigDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourcePackageAuthority {
    Rpm(RpmPackageAuthority),
    Debian(DebianPackageAuthority),
    Alpm(AlpmPackageAuthority),
    Ccs(CcsPackageAuthority),
}

impl SourcePackageAuthority {
    #[must_use]
    pub fn to_trove(&self, description: Option<String>) -> crate::db::models::Trove {
        let mut trove = crate::db::models::Trove::new(
            self.name().to_string(),
            self.version().to_string(),
            crate::db::models::TroveType::Package,
            self.version_scheme(),
        );
        trove.architecture = self.architecture().map(str::to_string);
        trove.debian_multi_arch = self.debian_multi_arch();
        trove.description = description;
        trove
    }

    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Rpm(value) => &value.name,
            Self::Debian(value) => &value.name,
            Self::Alpm(value) => &value.name,
            Self::Ccs(value) => &value.name,
        }
    }

    #[must_use]
    pub fn version(&self) -> &str {
        match self {
            Self::Rpm(value) => &value.evr,
            Self::Debian(value) => &value.version,
            Self::Alpm(value) => &value.version,
            Self::Ccs(value) => &value.version,
        }
    }

    #[must_use]
    pub const fn version_scheme(&self) -> VersionScheme {
        match self {
            Self::Rpm(_) => VersionScheme::Rpm,
            Self::Debian(_) => VersionScheme::Debian,
            Self::Alpm(_) => VersionScheme::Arch,
            Self::Ccs(value) => value.version_scheme,
        }
    }

    #[must_use]
    pub fn architecture(&self) -> Option<&str> {
        match self {
            Self::Rpm(value) => Some(&value.architecture),
            Self::Debian(value) => Some(&value.architecture),
            Self::Alpm(value) => Some(&value.architecture),
            Self::Ccs(value) => value.architecture.as_deref(),
        }
    }

    #[must_use]
    pub const fn debian_multi_arch(&self) -> Option<DebianMultiArch> {
        match self {
            Self::Debian(value) => Some(value.multi_arch),
            Self::Ccs(value) => value.debian_multi_arch,
            Self::Rpm(_) | Self::Alpm(_) => None,
        }
    }

    #[must_use]
    pub const fn source_format(&self) -> SourcePackageFormat {
        match self {
            Self::Rpm(_) => SourcePackageFormat::Rpm,
            Self::Debian(_) => SourcePackageFormat::Debian,
            Self::Alpm(_) => SourcePackageFormat::Alpm,
            Self::Ccs(_) => SourcePackageFormat::Ccs,
        }
    }

    pub fn declared_capabilities(&self) -> Result<Vec<ProvidedCapability>> {
        if let Self::Ccs(value) = self {
            for capability in &value.capabilities {
                if capability.provenance == CapabilityProvenance::ExactIdentity {
                    return Err(Error::ParseError(
                        "CCS capability list duplicates exact package identity".to_string(),
                    ));
                }
                capability.validate()?;
            }
            return Ok(value.capabilities.clone());
        }

        let project =
            |format,
             scheme,
             record_index,
             kind,
             name: &String,
             version: &Option<String>,
             version_relation,
             architecture_qualifier: &ProvideArchitectureQualifier| {
                let capability = ProvidedCapability {
                    kind,
                    name: name.clone(),
                    version: version.clone(),
                    version_relation,
                    version_scheme: scheme,
                    architecture_qualifier: architecture_qualifier.clone(),
                    provenance: CapabilityProvenance::SourceDeclared {
                        format,
                        record_index,
                    },
                };
                capability.validate()?;
                Ok(capability)
            };
        match self {
            Self::Rpm(value) => {
                let mut capabilities = value
                    .provides
                    .iter()
                    .map(|entry| {
                        project(
                            SourcePackageFormat::Rpm,
                            VersionScheme::Rpm,
                            entry.header_index,
                            entry.kind,
                            &entry.name,
                            &entry.version,
                            entry.version_relation,
                            &entry.architecture_qualifier,
                        )
                    })
                    .collect::<Result<Vec<_>>>()?;
                // RPM satisfies a file dependency from its header file table,
                // which includes paths the package only promises to create.
                // Debian and Arch have no equivalent declaration.
                crate::repository::dependency_model::extend_promised_path_provides(
                    &mut capabilities,
                    SourcePackageFormat::Rpm,
                    value.promised_paths.iter().map(|entry| entry.path.as_str()),
                )?;
                Ok(capabilities)
            }
            Self::Debian(value) => value
                .provides
                .iter()
                .map(|entry| {
                    project(
                        SourcePackageFormat::Debian,
                        VersionScheme::Debian,
                        entry.control_index,
                        entry.kind,
                        &entry.name,
                        &entry.version,
                        entry.version_relation,
                        &entry.architecture_qualifier,
                    )
                })
                .collect(),
            Self::Alpm(value) => value
                .provides
                .iter()
                .map(|entry| {
                    project(
                        SourcePackageFormat::Alpm,
                        VersionScheme::Arch,
                        entry.pkginfo_index,
                        entry.kind,
                        &entry.name,
                        &entry.version,
                        entry.version_relation,
                        &entry.architecture_qualifier,
                    )
                })
                .collect(),
            Self::Ccs(_) => unreachable!(),
        }
    }

    pub fn resolution_capabilities(&self) -> Result<Vec<ProvidedCapability>> {
        let mut capabilities = vec![ProvidedCapability {
            kind: RepositoryCapabilityKind::PackageName,
            name: self.name().to_string(),
            version: Some(self.version().to_string()),
            version_relation: Some(ProvideVersionRelation::Equal),
            version_scheme: self.version_scheme(),
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::ExactIdentity,
        }];
        capabilities.extend(self.declared_capabilities()?);
        Ok(capabilities)
    }

    /// Project native declaration authority into the closed transaction input.
    pub fn config_declarations(&self) -> Result<Vec<SourceConfigDeclaration>> {
        let declarations = match self {
            Self::Rpm(value) => value
                .config
                .iter()
                .cloned()
                .map(SourceConfigDeclaration::Rpm)
                .collect(),
            Self::Debian(value) => value
                .config
                .iter()
                .cloned()
                .map(SourceConfigDeclaration::Debian)
                .collect(),
            Self::Alpm(value) => value
                .config
                .iter()
                .cloned()
                .map(SourceConfigDeclaration::Alpm)
                .collect(),
            Self::Ccs(value) => value.config.clone(),
        };
        for declaration in &declarations {
            declaration.validate()?;
        }
        let mut paths = std::collections::BTreeSet::new();
        for declaration in &declarations {
            if !paths.insert(declaration.path()) {
                return Err(Error::ConfigError(format!(
                    "config path {} is declared more than once",
                    declaration.path()
                )));
            }
        }
        Ok(declarations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpm_identity_and_same_name_compatibility_provide_remain_distinct() {
        let authority = SourcePackageAuthority::Alpm(AlpmPackageAuthority {
            name: "aspnet-runtime".to_string(),
            version: "10.0.10.sdk110-1".to_string(),
            architecture: "x86_64".to_string(),
            package_type: Some("pkg".to_string()),
            provides: vec![crate::packages::arch::authority::AlpmDeclaredCapability {
                pkginfo_index: 0,
                kind: RepositoryCapabilityKind::PackageName,
                name: "aspnet-runtime".to_string(),
                version: Some("10.0".to_string()),
                version_relation: Some(ProvideVersionRelation::Equal),
                architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            }],
            config: Vec::new(),
        });

        let projected = authority.resolution_capabilities().unwrap();
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].version.as_deref(), Some("10.0.10.sdk110-1"));
        assert_eq!(projected[0].provenance, CapabilityProvenance::ExactIdentity);
        assert_eq!(projected[1].version.as_deref(), Some("10.0"));
        assert_eq!(
            projected[1].provenance,
            CapabilityProvenance::SourceDeclared {
                format: SourcePackageFormat::Alpm,
                record_index: 0,
            }
        );
    }

    #[test]
    fn cross_format_capabilities_preserve_role_scheme_alias_and_architecture() {
        let cases = [
            SourcePackageAuthority::Rpm(RpmPackageAuthority {
                name: "runtime".to_string(),
                epoch: Some(1),
                version_component: "10.0.10".to_string(),
                release: "2.fc44".to_string(),
                evr: "1:10.0.10-2.fc44".to_string(),
                architecture: "x86_64".to_string(),
                provides: vec![crate::packages::rpm::authority::RpmDeclaredCapability {
                    header_index: 7,
                    kind: RepositoryCapabilityKind::Virtual,
                    name: "dotnet-runtime".to_string(),
                    version: Some("10.0".to_string()),
                    version_relation: Some(ProvideVersionRelation::GreaterOrEqual),
                    architecture_qualifier: ProvideArchitectureQualifier::Exact(
                        "x86_64".to_string(),
                    ),
                }],
                config: Vec::new(),
                promised_paths: Vec::new(),
            }),
            SourcePackageAuthority::Debian(DebianPackageAuthority {
                name: "runtime".to_string(),
                epoch: Some(2),
                source_version: "2:10.0.10-1".to_string(),
                version: "2:10.0.10-1".to_string(),
                architecture: "amd64".to_string(),
                multi_arch: DebianMultiArch::Same,
                provides: vec![crate::packages::deb::authority::DebianDeclaredCapability {
                    control_index: 3,
                    kind: RepositoryCapabilityKind::PackageName,
                    name: "runtime".to_string(),
                    version: Some("10.0".to_string()),
                    version_relation: Some(ProvideVersionRelation::Equal),
                    architecture_qualifier: ProvideArchitectureQualifier::Exact(
                        "amd64".to_string(),
                    ),
                }],
                config: Vec::new(),
            }),
            SourcePackageAuthority::Alpm(AlpmPackageAuthority {
                name: "runtime".to_string(),
                version: "10.0.10-1".to_string(),
                architecture: "x86_64".to_string(),
                package_type: Some("pkg".to_string()),
                provides: vec![crate::packages::arch::authority::AlpmDeclaredCapability {
                    pkginfo_index: 5,
                    kind: RepositoryCapabilityKind::Virtual,
                    name: "dotnet-runtime".to_string(),
                    version: None,
                    version_relation: None,
                    architecture_qualifier: ProvideArchitectureQualifier::Implicit,
                }],
                config: Vec::new(),
            }),
        ];

        let expected = [
            (
                VersionScheme::Rpm,
                SourcePackageFormat::Rpm,
                7,
                RepositoryCapabilityKind::Virtual,
                Some(ProvideVersionRelation::GreaterOrEqual),
                ProvideArchitectureQualifier::Exact("x86_64".to_string()),
            ),
            (
                VersionScheme::Debian,
                SourcePackageFormat::Debian,
                3,
                RepositoryCapabilityKind::PackageName,
                Some(ProvideVersionRelation::Equal),
                ProvideArchitectureQualifier::Exact("amd64".to_string()),
            ),
            (
                VersionScheme::Arch,
                SourcePackageFormat::Alpm,
                5,
                RepositoryCapabilityKind::Virtual,
                None,
                ProvideArchitectureQualifier::Implicit,
            ),
        ];

        for (authority, expected) in cases.into_iter().zip(expected) {
            let projected = authority.resolution_capabilities().unwrap();
            assert_eq!(projected.len(), 2);
            assert_eq!(projected[0].provenance, CapabilityProvenance::ExactIdentity);
            assert_eq!(projected[0].version_scheme, expected.0);
            assert_eq!(projected[1].kind, expected.3);
            assert_eq!(projected[1].version_relation, expected.4);
            assert_eq!(projected[1].architecture_qualifier, expected.5);
            assert_eq!(
                projected[1].provenance,
                CapabilityProvenance::SourceDeclared {
                    format: expected.1,
                    record_index: expected.2,
                }
            );
        }
    }

    fn rpm_authority_with_promised_paths(paths: &[&str]) -> SourcePackageAuthority {
        SourcePackageAuthority::Rpm(RpmPackageAuthority {
            name: "crypto-policies".to_string(),
            epoch: None,
            version_component: "20251128".to_string(),
            release: "3".to_string(),
            evr: "20251128-3".to_string(),
            architecture: "noarch".to_string(),
            provides: Vec::new(),
            config: Vec::new(),
            promised_paths: paths
                .iter()
                .enumerate()
                .map(
                    |(index, path)| crate::packages::rpm::authority::RpmPromisedPath {
                        header_index: index as u32,
                        path: (*path).to_string(),
                    },
                )
                .collect(),
        })
    }

    #[test]
    fn rpm_publishes_every_ghost_header_path_as_a_promised_capability() {
        let authority =
            rpm_authority_with_promised_paths(&["/etc/crypto-policies/back-ends/krb5.config"]);

        let capabilities = authority.declared_capabilities().unwrap();

        assert_eq!(capabilities.len(), 1);
        let promised = &capabilities[0];
        assert!(promised.is_promised_path());
        assert!(!promised.witnesses_installed_content());
        assert_eq!(promised.kind, RepositoryCapabilityKind::File);
        assert_eq!(promised.name, "/etc/crypto-policies/back-ends/krb5.config");
        assert_eq!(promised.version, None);
        assert_eq!(promised.version_scheme, VersionScheme::Rpm);
        promised.validate().unwrap();
    }

    #[test]
    fn a_promised_path_never_displaces_the_exact_package_identity() {
        let authority =
            rpm_authority_with_promised_paths(&["/etc/crypto-policies/back-ends/krb5.config"]);

        let capabilities = authority.resolution_capabilities().unwrap();

        assert_eq!(
            capabilities
                .iter()
                .filter(|capability| capability.provenance == CapabilityProvenance::ExactIdentity)
                .count(),
            1
        );
        assert!(
            capabilities
                .iter()
                .any(ProvidedCapability::is_promised_path)
        );
    }

    #[test]
    fn only_rpm_declares_promised_paths() {
        // Debian and Arch have no %ghost equivalent, so no source authority
        // other than RPM may publish a promise.
        for authority in [
            SourcePackageAuthority::Debian(DebianPackageAuthority {
                name: "tool".to_string(),
                epoch: None,
                source_version: "1.0-1".to_string(),
                version: "1.0-1".to_string(),
                architecture: "amd64".to_string(),
                multi_arch: DebianMultiArch::No,
                provides: Vec::new(),
                config: Vec::new(),
            }),
            SourcePackageAuthority::Alpm(AlpmPackageAuthority {
                name: "tool".to_string(),
                version: "1.0-1".to_string(),
                architecture: "x86_64".to_string(),
                package_type: None,
                provides: Vec::new(),
                config: Vec::new(),
            }),
        ] {
            assert!(
                !authority
                    .declared_capabilities()
                    .unwrap()
                    .iter()
                    .any(ProvidedCapability::is_promised_path)
            );
        }
    }
}
