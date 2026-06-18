// conary-core/src/repository/supported_profiles/types.rs

use serde::Deserialize;

use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::distro::ReplayTargetOwned;
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
    pub replay_target: ReplayTargetDocument,
    pub repository: RepositoryHintsDocument,
    pub lifecycle: LifecycleDocument,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(super) struct ProfileIdentityDocument {
    pub family_slug: String,
    pub remi_route_slug: String,
    pub package_format: ProfilePackageFormat,
    pub dependency_flavor: DependencyFlavorValue,
    pub version_scheme: VersionSchemeValue,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ReplayTargetDocument {
    pub format: ReplayFormat,
    pub distro: String,
    pub release: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(super) struct RepositoryHintsDocument {
    pub name_patterns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(super) struct LifecycleDocument {
    pub service_manager: String,
    pub default_shell: String,
    #[serde(default)]
    pub path_dirs: Vec<String>,
    pub services: LifecyclePolicyDocument,
    pub tmpfiles: LifecyclePolicyDocument,
    pub sysctl: LifecyclePolicyDocument,
    pub users: LifecyclePolicyDocument,
    pub groups: LifecyclePolicyDocument,
    pub directories: LifecyclePolicyDocument,
    pub alternatives: LifecyclePolicyDocument,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) struct LifecyclePolicyDocument {
    pub mode: LifecyclePolicyMode,
    #[serde(default)]
    pub entries: Vec<String>,
    #[serde(default)]
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProfilePackageFormat {
    Rpm,
    Deb,
    Arch,
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

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ReplayFormat {
    Rpm,
    Deb,
    Arch,
}

impl ReplayFormat {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            ReplayFormat::Rpm => "rpm",
            ReplayFormat::Deb => "deb",
            ReplayFormat::Arch => "arch",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LifecyclePolicyMode {
    AllowList,
    Unsupported,
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

    #[must_use]
    pub fn repository_name_patterns(&self) -> &[String] {
        &self.document.repository.name_patterns
    }

    #[must_use]
    pub fn replay_target_for_arch(&self, arch: &str) -> ReplayTargetOwned {
        ReplayTargetOwned {
            format: self.document.replay_target.format.as_str().to_string(),
            distro: self.document.replay_target.distro.clone(),
            release: self.document.replay_target.release.clone(),
            arch: arch.trim().to_string(),
        }
    }

    #[must_use]
    pub(super) fn lifecycle(&self) -> &LifecycleDocument {
        &self.document.lifecycle
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
