// crates/conary-core/src/repository/catalog/parity/contract.rs

//! Versioned native parity manifest and package-row contracts.

use serde::{Deserialize, Serialize};

use super::super::contract::{validate_identity, validate_sha256};
use super::super::{
    CatalogCountsV1, CatalogPackageOriginV1, CatalogPackageRecordV1, CatalogProvideRecordV1,
    CatalogRequirementGroupV1, CatalogScopeV1, ProfileRevisionV2, ProfileSourceMemberV2,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::DebianMultiArch;
use crate::repository::versioning::VersionScheme;

pub const NATIVE_PARITY_ORACLE_SCHEMA_V1: u32 = 1;

/// The native package-manager family that independently produced one oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeParityEcosystemV1 {
    Rpm,
    Debian,
    Alpm,
}

impl NativeParityEcosystemV1 {
    #[must_use]
    pub const fn version_scheme(self) -> VersionScheme {
        match self {
            Self::Rpm => VersionScheme::Rpm,
            Self::Debian => VersionScheme::Debian,
            Self::Alpm => VersionScheme::Arch,
        }
    }
}

/// Exact pinned native implementation that emitted the normalized oracle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeParityImplementationV1 {
    pub ecosystem: NativeParityEcosystemV1,
    pub name: String,
    pub version: String,
    pub projection_schema: u32,
}

impl NativeParityImplementationV1 {
    pub(super) fn validate(&self) -> Result<()> {
        validate_identity(&self.name, "native parity implementation name")?;
        validate_identity(&self.version, "native parity implementation version")?;
        if self.projection_schema == 0 {
            return Err(Error::ConfigError(
                "native parity implementation projection schema must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

/// Counts for the complete normalized native package fact stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeParityCountsV1 {
    pub packages: u64,
    pub provides: u64,
    pub requirement_groups: u64,
    pub requirement_atoms: u64,
}

impl From<CatalogCountsV1> for NativeParityCountsV1 {
    fn from(value: CatalogCountsV1) -> Self {
        Self {
            packages: value.packages,
            provides: value.provides,
            requirement_groups: value.requirement_groups,
            requirement_atoms: value.requirement_atoms,
        }
    }
}

/// Exact content-addressed line-oriented package fact artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeParityArtifactV1 {
    pub sha256: String,
    pub size: u64,
    pub counts: NativeParityCountsV1,
}

impl NativeParityArtifactV1 {
    fn validate(&self) -> Result<()> {
        validate_sha256(&self.sha256, "native parity artifact SHA-256")
    }
}

/// Strict manifest binding one native oracle to one exact profile revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeParityOracleV1 {
    pub schema_version: u32,
    pub profile: String,
    pub profile_revision_sha256: String,
    pub profile_logical_digest_sha256: String,
    pub members: Vec<ProfileSourceMemberV2>,
    pub implementation: NativeParityImplementationV1,
    pub artifact: NativeParityArtifactV1,
}

impl NativeParityOracleV1 {
    pub fn bind(
        profile: &ProfileRevisionV2,
        implementation: NativeParityImplementationV1,
        artifact: NativeParityArtifactV1,
    ) -> Result<Self> {
        profile.validate()?;
        let manifest = Self {
            schema_version: NATIVE_PARITY_ORACLE_SCHEMA_V1,
            profile: profile.profile.clone(),
            profile_revision_sha256: profile.manifest_sha256()?,
            profile_logical_digest_sha256: profile.logical_digest_sha256.clone(),
            members: profile.members.clone(),
            implementation,
            artifact,
        };
        manifest.validate_profile(profile)?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != NATIVE_PARITY_ORACLE_SCHEMA_V1 {
            return Err(Error::ConfigError(format!(
                "native parity oracle schema {} is unsupported; expected {}",
                self.schema_version, NATIVE_PARITY_ORACLE_SCHEMA_V1
            )));
        }
        validate_identity(&self.profile, "native parity profile")?;
        validate_sha256(
            &self.profile_revision_sha256,
            "native parity profile revision SHA-256",
        )?;
        validate_sha256(
            &self.profile_logical_digest_sha256,
            "native parity profile logical digest",
        )?;
        self.implementation.validate()?;
        self.artifact.validate()?;
        validate_members(&self.members)
    }

    pub fn validate_profile(&self, profile: &ProfileRevisionV2) -> Result<()> {
        self.validate()?;
        profile.validate()?;
        let profile_sha256 = profile.manifest_sha256()?;
        if self.profile != profile.profile
            || self.profile_revision_sha256 != profile_sha256
            || self.profile_logical_digest_sha256 != profile.logical_digest_sha256
            || self.members != profile.members
        {
            return Err(Error::ConflictError(format!(
                "native parity oracle does not bind exact profile revision '{}'",
                profile.profile
            )));
        }
        Ok(())
    }

    pub fn manifest_sha256(&self) -> Result<String> {
        self.validate()?;
        let bytes = crate::json::canonical_json(self).map_err(|error| {
            Error::ParseError(format!("serialize native parity manifest: {error}"))
        })?;
        Ok(crate::hash::sha256(&bytes))
    }
}

/// One exact package and every source-authoritative resolution fact compared
/// against the immutable profile catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeParityPackageV1 {
    pub package_key_sha256: String,
    pub member_ordinal: u32,
    pub source_identity: String,
    pub repository_identity: String,
    pub source_snapshot_sha256: String,
    pub source_profile: String,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: Option<String>,
    pub debian_multi_arch: Option<DebianMultiArch>,
    pub checksum: String,
    pub size: u64,
    pub download_url: String,
    pub version_scheme: VersionScheme,
    pub provides: Vec<CatalogProvideRecordV1>,
    pub requirement_groups: Vec<CatalogRequirementGroupV1>,
}

impl NativeParityPackageV1 {
    #[cfg(test)]
    pub(super) fn from_catalog(package: &CatalogPackageRecordV1) -> Result<Self> {
        let CatalogPackageOriginV1::Profile {
            member_ordinal,
            source_identity,
            repository_identity,
            source_snapshot_sha256,
        } = &package.origin
        else {
            return Err(Error::ConfigError(format!(
                "native parity package '{}' must come from a profile catalog",
                package.name
            )));
        };
        Ok(Self {
            package_key_sha256: package.package_key_sha256.clone(),
            member_ordinal: *member_ordinal,
            source_identity: source_identity.clone(),
            repository_identity: repository_identity.clone(),
            source_snapshot_sha256: source_snapshot_sha256.clone(),
            source_profile: package.source_profile.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
            package_release: package.package_release.clone(),
            architecture: package.architecture.clone(),
            debian_multi_arch: package.debian_multi_arch,
            checksum: package.checksum.clone(),
            size: package.size,
            download_url: package.download_url.clone(),
            version_scheme: package.version_scheme,
            provides: package.provides.clone(),
            requirement_groups: package.requirement_groups.clone(),
        })
    }

    pub fn validate(&self, manifest: &NativeParityOracleV1) -> Result<()> {
        manifest.validate()?;
        self.validate_authority(
            &manifest.profile,
            &manifest.members,
            &manifest.implementation,
        )
    }

    #[cfg(any(feature = "native-alpm-oracle", feature = "native-rpm-oracle"))]
    pub(super) fn canonicalize_for_profile(&mut self, profile: &str) -> Result<()> {
        let mut record = self.as_catalog_record();
        record.canonicalize_for_scope(&CatalogScopeV1::Profile {
            profile: profile.to_string(),
        })?;
        self.package_key_sha256 = record.package_key_sha256;
        self.provides = record.provides;
        self.requirement_groups = record.requirement_groups;
        Ok(())
    }

    #[cfg(any(feature = "native-alpm-oracle", feature = "native-rpm-oracle"))]
    pub(super) fn has_same_profile_facts(&self, other: &Self) -> bool {
        self.as_catalog_record()
            .same_profile_record(&other.as_catalog_record())
    }

    pub(super) fn validate_authority(
        &self,
        profile: &str,
        members: &[ProfileSourceMemberV2],
        implementation: &NativeParityImplementationV1,
    ) -> Result<()> {
        let member = members
            .get(usize::try_from(self.member_ordinal).map_err(|_| {
                Error::ConfigError("native parity member ordinal exceeds usize".to_string())
            })?)
            .ok_or_else(|| {
                Error::ConfigError(format!(
                    "native parity package '{}' names absent member ordinal {}",
                    self.name, self.member_ordinal
                ))
            })?;
        if member.ordinal != self.member_ordinal
            || member.source_identity != self.source_identity
            || member.repository_identity != self.repository_identity
            || member.source_snapshot_sha256 != self.source_snapshot_sha256
        {
            return Err(Error::ConflictError(format!(
                "native parity package '{}' origin disagrees with member ordinal {}",
                self.name, self.member_ordinal
            )));
        }
        if self.version_scheme != implementation.ecosystem.version_scheme() {
            return Err(Error::ConfigError(format!(
                "native parity package '{}' uses {} under a {:?} oracle",
                self.name,
                self.version_scheme.as_str(),
                implementation.ecosystem
            )));
        }
        self.as_catalog_record().validate(&CatalogScopeV1::Profile {
            profile: profile.to_string(),
        })
    }

    fn as_catalog_record(&self) -> CatalogPackageRecordV1 {
        CatalogPackageRecordV1 {
            package_key_sha256: self.package_key_sha256.clone(),
            origin: CatalogPackageOriginV1::Profile {
                member_ordinal: self.member_ordinal,
                source_identity: self.source_identity.clone(),
                repository_identity: self.repository_identity.clone(),
                source_snapshot_sha256: self.source_snapshot_sha256.clone(),
            },
            source_profile: self.source_profile.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            package_release: self.package_release.clone(),
            architecture: self.architecture.clone(),
            debian_multi_arch: self.debian_multi_arch,
            description: None,
            checksum: self.checksum.clone(),
            size: self.size,
            download_url: self.download_url.clone(),
            metadata: None,
            is_security_update: false,
            severity: None,
            cve_ids: None,
            advisory_id: None,
            advisory_url: None,
            version_scheme: self.version_scheme,
            provides: self.provides.clone(),
            requirement_groups: self.requirement_groups.clone(),
        }
    }
}

pub(super) fn validate_members(members: &[ProfileSourceMemberV2]) -> Result<()> {
    if members.is_empty() {
        return Err(Error::ConfigError(
            "native parity oracle must bind at least one source member".to_string(),
        ));
    }
    for (index, member) in members.iter().enumerate() {
        let expected = u32::try_from(index).map_err(|_| {
            Error::ConfigError("native parity oracle contains too many members".to_string())
        })?;
        if member.ordinal != expected {
            return Err(Error::ConfigError(format!(
                "native parity member ordinal {} is noncanonical; expected {expected}",
                member.ordinal
            )));
        }
        validate_identity(
            &member.source_identity,
            "native parity member source identity",
        )?;
        validate_identity(
            &member.repository_identity,
            "native parity member repository identity",
        )?;
        validate_identity(
            &member.stream.identity,
            "native parity member stream identity",
        )?;
        validate_sha256(
            &member.source_snapshot_sha256,
            "native parity member source snapshot SHA-256",
        )?;
    }
    Ok(())
}
