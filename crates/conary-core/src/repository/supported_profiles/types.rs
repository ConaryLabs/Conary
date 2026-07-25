// conary-core/src/repository/supported_profiles/types.rs

use serde::Deserialize;

use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::versioning::VersionScheme;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(super) struct CatalogDocument {
    pub profiles: Vec<ProfileDocument>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(super) struct ProfileDocument {
    pub id: String,
    pub display_name: String,
    pub release: String,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub eol: Option<String>,
    pub identity: ProfileIdentityDocument,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(super) struct ProfileIdentityDocument {
    pub family_slug: String,
    pub remi_route_slug: String,
    pub repology_repo: String,
    pub package_format: ProfilePackageFormat,
    pub dependency_flavor: DependencyFlavorValue,
    pub version_scheme: VersionSchemeValue,
    #[serde(default)]
    pub scriptlet_shell: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProfilePackageFormat {
    Rpm,
    Deb,
    Arch,
}

impl ProfilePackageFormat {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
            Self::Arch => "arch",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum DependencyFlavorValue {
    Rpm,
    Deb,
    Arch,
}

impl From<DependencyFlavorValue> for RepositoryDependencyFlavor {
    fn from(value: DependencyFlavorValue) -> Self {
        match value {
            DependencyFlavorValue::Rpm => RepositoryDependencyFlavor::Rpm,
            DependencyFlavorValue::Deb => RepositoryDependencyFlavor::Deb,
            DependencyFlavorValue::Arch => RepositoryDependencyFlavor::Arch,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum VersionSchemeValue {
    Rpm,
    Debian,
    Arch,
}

impl From<VersionSchemeValue> for VersionScheme {
    fn from(value: VersionSchemeValue) -> Self {
        match value {
            VersionSchemeValue::Rpm => VersionScheme::Rpm,
            VersionSchemeValue::Debian => VersionScheme::Debian,
            VersionSchemeValue::Arch => VersionScheme::Arch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedProfile {
    document: ProfileDocument,
}

impl SupportedProfile {
    pub(super) fn new(document: ProfileDocument) -> Self {
        Self { document }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.document.id
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.document.display_name
    }

    #[must_use]
    pub fn family_slug(&self) -> &str {
        &self.document.identity.family_slug
    }

    #[must_use]
    pub fn remi_route_slug(&self) -> &str {
        &self.document.identity.remi_route_slug
    }

    #[must_use]
    pub fn repology_repo(&self) -> &str {
        &self.document.identity.repology_repo
    }

    #[must_use]
    pub fn package_format(&self) -> ProfilePackageFormat {
        self.document.identity.package_format
    }

    #[must_use]
    pub fn dependency_flavor(&self) -> RepositoryDependencyFlavor {
        self.document.identity.dependency_flavor.into()
    }

    #[must_use]
    pub fn version_scheme(&self) -> VersionScheme {
        self.document.identity.version_scheme.into()
    }

    /// Distribution-build-time shell for ALPM `.INSTALL` function libraries.
    ///
    /// ALPM packages do not carry this in package metadata, so it belongs to
    /// the exact source profile rather than the package parser or target host.
    #[must_use]
    pub fn scriptlet_shell(&self) -> Option<&str> {
        self.document.identity.scriptlet_shell.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedRoute {
    slug: String,
    public_profile_ids: Vec<String>,
}

impl SupportedRoute {
    pub(super) fn new(slug: String, public_profile_ids: Vec<String>) -> Self {
        Self {
            slug,
            public_profile_ids,
        }
    }

    #[must_use]
    pub fn slug(&self) -> &str {
        &self.slug
    }

    #[must_use]
    pub fn public_profile_ids(&self) -> &[String] {
        &self.public_profile_ids
    }
}
