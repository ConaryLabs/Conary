// src/commands/install/semantics.rs

use super::{PackageFormatType, prepare};
use conary_core::repository::versioning::VersionScheme;
use conary_core::scriptlet::ExecutionMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreparedSourceKind {
    NativePackage { format: PackageFormatType },
    Ccs,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum InstallIntent {
    #[default]
    PackageChange,
    Replatform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InstallSemantics {
    pub(super) source: PreparedSourceKind,
    pub(super) version_scheme: VersionScheme,
}

impl InstallSemantics {
    pub(super) fn native_package(format: PackageFormatType) -> Self {
        Self {
            source: PreparedSourceKind::NativePackage { format },
            version_scheme: prepare::version_scheme_for_format(format),
        }
    }

    pub(super) fn ccs(version_scheme: VersionScheme) -> Self {
        Self {
            source: PreparedSourceKind::Ccs,
            version_scheme,
        }
    }

    /// Exact source format that owns the capabilities this artifact publishes.
    ///
    /// A converted CCS keeps the version scheme of the format it was built
    /// from, so both prepared sources recover their format from the one scheme
    /// they already carry.
    pub(super) const fn source_package_format(
        self,
    ) -> conary_core::repository::dependency_model::SourcePackageFormat {
        conary_core::repository::dependency_model::SourcePackageFormat::for_version_scheme(
            self.version_scheme,
        )
    }

    pub(super) const fn payload_sharing_policy(self) -> conary_core::payload::PayloadSharingPolicy {
        match self.source {
            PreparedSourceKind::NativePackage {
                format: PackageFormatType::Rpm,
            } => conary_core::payload::PayloadSharingPolicy::Rpm,
            PreparedSourceKind::NativePackage {
                format: PackageFormatType::Deb | PackageFormatType::Arch,
            }
            | PreparedSourceKind::Ccs => conary_core::payload::PayloadSharingPolicy::Exclusive,
        }
    }
}

pub(super) fn build_execution_mode(old_version: Option<&str>) -> ExecutionMode {
    match old_version {
        Some(version) => ExecutionMode::Upgrade {
            old_version: version.to_string(),
        },
        None => ExecutionMode::Install,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_mode_reflects_upgrade_identity() {
        assert_eq!(build_execution_mode(None), ExecutionMode::Install);
        assert_eq!(
            build_execution_mode(Some("1.0.0")),
            ExecutionMode::Upgrade {
                old_version: "1.0.0".to_string()
            }
        );
    }

    #[test]
    fn source_semantics_own_payload_sharing_policy() {
        use conary_core::payload::PayloadSharingPolicy;

        assert_eq!(
            InstallSemantics::native_package(PackageFormatType::Rpm).payload_sharing_policy(),
            PayloadSharingPolicy::Rpm
        );
        for format in [PackageFormatType::Deb, PackageFormatType::Arch] {
            assert_eq!(
                InstallSemantics::native_package(format).payload_sharing_policy(),
                PayloadSharingPolicy::Exclusive
            );
        }
        assert_eq!(
            InstallSemantics::ccs(VersionScheme::Conary).payload_sharing_policy(),
            PayloadSharingPolicy::Exclusive
        );
    }
}
