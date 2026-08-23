// conary-core/src/repository/supported_profiles/types.rs

use serde::{Deserialize, Serialize};

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
    pub support_tier: SupportTier,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub eol: Option<String>,
    pub members: Vec<ProfileSourceMemberContract>,
    pub identity: ProfileIdentityDocument,
}

/// Product support state for one exact source profile.
///
/// Only `Public` profiles may be enrolled automatically or exposed through
/// the canonical Remi public authority. The other tiers remain explicit so
/// implementation and test evidence cannot accidentally promote a profile.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SupportTier {
    Public,
    Candidate,
    Internal,
    Retired,
}

/// Declared function of one repository inside a complete profile universe.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileSourceRole {
    Base,
    Updates,
    Security,
    Backports,
    Overlay,
    Optional,
    Debug,
    Source,
}

/// Exact repository membership required to compose one supported profile.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileSourceMemberContract {
    pub repository_identity: String,
    pub role: ProfileSourceRole,
    pub precedence: i32,
}

impl ProfileSourceRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Updates => "updates",
            Self::Security => "security",
            Self::Backports => "backports",
            Self::Overlay => "overlay",
            Self::Optional => "optional",
            Self::Debug => "debug",
            Self::Source => "source",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "base" => Ok(Self::Base),
            "updates" => Ok(Self::Updates),
            "security" => Ok(Self::Security),
            "backports" => Ok(Self::Backports),
            "overlay" => Ok(Self::Overlay),
            "optional" => Ok(Self::Optional),
            "debug" => Ok(Self::Debug),
            "source" => Ok(Self::Source),
            other => Err(format!("unknown profile source role '{other}'")),
        }
    }
}

impl SupportTier {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Candidate => "candidate",
            Self::Internal => "internal",
            Self::Retired => "retired",
        }
    }

    #[must_use]
    pub const fn is_public(self) -> bool {
        matches!(self, Self::Public)
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(super) struct ProfileIdentityDocument {
    pub family_slug: String,
    pub remi_route_slug: String,
    pub repology_repo: String,
    pub package_format: ProfilePackageFormat,
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
    Eopkg,
}

impl ProfilePackageFormat {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
            Self::Arch => "arch",
            Self::Eopkg => "eopkg",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum VersionSchemeValue {
    Rpm,
    Debian,
    Arch,
    Eopkg,
}

impl From<VersionSchemeValue> for VersionScheme {
    fn from(value: VersionSchemeValue) -> Self {
        match value {
            VersionSchemeValue::Rpm => VersionScheme::Rpm,
            VersionSchemeValue::Debian => VersionScheme::Debian,
            VersionSchemeValue::Arch => VersionScheme::Arch,
            VersionSchemeValue::Eopkg => VersionScheme::Eopkg,
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
    pub fn support_tier(&self) -> SupportTier {
        self.document.support_tier
    }

    #[must_use]
    pub fn members(&self) -> &[ProfileSourceMemberContract] {
        &self.document.members
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
