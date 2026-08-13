// conary-core/src/repository/dependency_source.rs

//! Source ecosystem identity for repository dependencies and capabilities.
//!
//! This module owns the closed relationship between a package format and its
//! version grammar. Keeping that relationship separate from the dependency
//! expression model prevents each consumer from inventing its own mapping.

use super::versioning::VersionScheme;
use serde::{Deserialize, Serialize};

/// Source package format that established a signed or persisted capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourcePackageFormat {
    Rpm,
    Debian,
    Alpm,
    Eopkg,
    Ccs,
}

impl SourcePackageFormat {
    #[must_use]
    pub const fn version_scheme(self) -> VersionScheme {
        match self {
            Self::Rpm => VersionScheme::Rpm,
            Self::Debian => VersionScheme::Debian,
            Self::Alpm => VersionScheme::Arch,
            Self::Eopkg => VersionScheme::Eopkg,
            Self::Ccs => VersionScheme::Conary,
        }
    }

    /// Exact source format that owns package identity under `scheme`.
    #[must_use]
    pub const fn for_version_scheme(scheme: VersionScheme) -> Self {
        match scheme {
            VersionScheme::Rpm => Self::Rpm,
            VersionScheme::Debian => Self::Debian,
            VersionScheme::Arch => Self::Alpm,
            VersionScheme::Eopkg => Self::Eopkg,
            VersionScheme::Conary => Self::Ccs,
        }
    }
}

/// Why a capability exists. Exact identity remains distinct from declarations
/// and payload-derived capabilities.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "kebab-case")]
pub enum CapabilityProvenance {
    ExactIdentity,
    AuthorDeclared,
    SourceDeclared {
        format: SourcePackageFormat,
        record_index: u32,
    },
    /// A path the source package ships, derived from one exact source record.
    SourceDerivedFile {
        format: SourcePackageFormat,
    },
    /// A path the source package owns but ships no content for.
    SourcePromisedPath {
        format: SourcePackageFormat,
    },
}

/// Which native ecosystem a dependency originates from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RepositoryDependencyFlavor {
    Conary,
    Rpm,
    Deb,
    Arch,
    Eopkg,
}

impl RepositoryDependencyFlavor {
    #[must_use]
    pub const fn version_scheme(self) -> VersionScheme {
        match self {
            Self::Conary => VersionScheme::Conary,
            Self::Rpm => VersionScheme::Rpm,
            Self::Deb => VersionScheme::Debian,
            Self::Arch => VersionScheme::Arch,
            Self::Eopkg => VersionScheme::Eopkg,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_format_has_one_version_scheme_inverse() {
        for format in [
            SourcePackageFormat::Rpm,
            SourcePackageFormat::Debian,
            SourcePackageFormat::Alpm,
            SourcePackageFormat::Eopkg,
            SourcePackageFormat::Ccs,
        ] {
            assert_eq!(
                SourcePackageFormat::for_version_scheme(format.version_scheme()),
                format
            );
        }
    }
}
