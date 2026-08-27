// crates/conary-core/src/repository/catalog/record.rs

//! Canonical package records stored in immutable repository catalogs.

mod debian;
mod digest;

pub use debian::DebianSourcePocketV1;
pub(in crate::repository) use digest::CatalogLogicalDigestV1;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::contract::{validate_identity, validate_relative_source_path, validate_sha256};
use super::{CatalogCountsV1, SourceMetadataObjectRoleV1};
use crate::db::models::{
    RepositoryPackage, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup as DbRequirementGroup,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::{
    DebianMultiArch, ProvideArchitectureQualifier, ProvideVersionRelation, ProvidedCapability,
    RepositoryCapabilityKind, RepositoryRequirementExpression, RepositoryRequirementKind,
};
use crate::repository::dependency_source::CapabilityProvenance;
use crate::repository::versioning::VersionScheme;

pub const CATALOG_CONTENT_SCHEMA_V1: u32 = 1;

/// The manifest authority whose package projection a catalog contains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogScopeV1 {
    Source {
        source_profile: String,
        source_identity: String,
        repository_identity: String,
    },
    Profile {
        profile: String,
    },
}

/// Exact source member that contributed one package record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogPackageOriginV1 {
    Source {
        source_identity: String,
        repository_identity: String,
    },
    Profile {
        member_ordinal: u32,
        source_identity: String,
        repository_identity: String,
        source_snapshot_sha256: String,
    },
}

/// Exact authenticated input bound into the catalog's logical content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogSourceEvidenceV1 {
    AuthenticatedObject {
        role: SourceMetadataObjectRoleV1,
        source_path: String,
        sha256: String,
        size: u64,
    },
    SourceSnapshot {
        member_ordinal: u32,
        source_identity: String,
        repository_identity: String,
        source_snapshot_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProvideRecordV1 {
    pub capability: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub kind: String,
    pub raw: Option<String>,
    pub version_scheme: VersionScheme,
    pub architecture_qualifier: ProvideArchitectureQualifier,
    pub provenance: CapabilityProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRequirementAtomV1 {
    pub capability: String,
    pub version_constraint: Option<String>,
    pub kind: String,
    pub dependency_type: String,
    pub raw: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRequirementGroupV1 {
    pub kind: String,
    pub behavior: String,
    pub description: Option<String>,
    pub native_text: Option<String>,
    pub expression_json: String,
    pub atoms: Vec<CatalogRequirementAtomV1>,
}

/// One repository package without any operational database identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogPackageRecordV1 {
    pub package_key_sha256: String,
    pub origin: CatalogPackageOriginV1,
    pub source_profile: String,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: Option<String>,
    pub debian_multi_arch: Option<DebianMultiArch>,
    pub description: Option<String>,
    pub checksum: String,
    pub size: u64,
    pub download_url: String,
    pub metadata: Option<String>,
    pub is_security_update: bool,
    pub severity: Option<String>,
    pub cve_ids: Option<String>,
    pub advisory_id: Option<String>,
    pub advisory_url: Option<String>,
    pub version_scheme: VersionScheme,
    pub provides: Vec<CatalogProvideRecordV1>,
    pub requirement_groups: Vec<CatalogRequirementGroupV1>,
}

/// Canonical logical content from which one immutable SQLite artifact is built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogContentV1 {
    pub schema_version: u32,
    pub scope: CatalogScopeV1,
    pub source_evidence: Vec<CatalogSourceEvidenceV1>,
    pub packages: Vec<CatalogPackageRecordV1>,
}

impl CatalogScopeV1 {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Source {
                source_profile,
                source_identity,
                repository_identity,
            } => {
                validate_identity(source_profile, "source catalog profile")?;
                validate_identity(source_identity, "source catalog source identity")?;
                validate_identity(repository_identity, "source catalog repository identity")
            }
            Self::Profile { profile } => validate_identity(profile, "profile catalog profile"),
        }
    }

    #[must_use]
    pub fn profile(&self) -> &str {
        match self {
            Self::Source { source_profile, .. } => source_profile,
            Self::Profile { profile } => profile,
        }
    }
}

impl CatalogPackageRecordV1 {
    pub fn from_repository_projection(
        package: RepositoryPackage,
        provides: Vec<RepositoryProvide>,
        requirement_groups: Vec<DbRequirementGroup>,
        requirement_group_clauses: Vec<Vec<RepositoryRequirement>>,
        origin: CatalogPackageOriginV1,
    ) -> Result<Self> {
        let RepositoryPackage {
            id: _,
            repository_id: _,
            name,
            version,
            package_release,
            architecture,
            debian_multi_arch,
            description,
            checksum,
            size,
            download_url,
            metadata,
            synced_at: _,
            is_security_update,
            severity,
            cve_ids,
            advisory_id,
            advisory_url,
            source_profile,
            version_scheme,
            canonical_id: _,
        } = package;
        let source_profile = source_profile.ok_or_else(|| {
            Error::ConfigError("catalog package is missing its exact source profile".to_string())
        })?;
        let size = u64::try_from(size).map_err(|_| {
            Error::ConfigError(format!("catalog package '{name}' has negative size {size}"))
        })?;
        if requirement_groups.len() != requirement_group_clauses.len() {
            return Err(Error::InternalError(format!(
                "catalog package '{name}' has {} requirement groups but {} clause vectors",
                requirement_groups.len(),
                requirement_group_clauses.len()
            )));
        }

        let provides = provides
            .into_iter()
            .map(CatalogProvideRecordV1::from)
            .collect();
        let requirement_groups = requirement_groups
            .into_iter()
            .zip(requirement_group_clauses)
            .map(|(group, atoms)| CatalogRequirementGroupV1::from_db(group, atoms))
            .collect();
        let mut record = Self {
            package_key_sha256: String::new(),
            origin,
            source_profile,
            name,
            version,
            package_release,
            architecture,
            debian_multi_arch,
            description,
            checksum,
            size,
            download_url,
            metadata,
            is_security_update,
            severity,
            cve_ids,
            advisory_id,
            advisory_url,
            version_scheme,
            provides,
            requirement_groups,
        };
        record.canonicalize()?;
        record.package_key_sha256 = record.calculate_package_key()?;
        Ok(record)
    }

    fn canonicalize(&mut self) -> Result<()> {
        if let Some(metadata) = &mut self.metadata {
            canonicalize_json_text(metadata, "catalog package metadata")?;
        }
        canonical_sort(&mut self.provides)?;
        for group in &mut self.requirement_groups {
            canonicalize_json_text(
                &mut group.expression_json,
                "catalog requirement group expression",
            )?;
            canonical_sort(&mut group.atoms)?;
        }
        canonical_sort(&mut self.requirement_groups)
    }

    pub(in crate::repository) fn canonicalize_for_scope(
        &mut self,
        scope: &CatalogScopeV1,
    ) -> Result<()> {
        self.canonicalize()?;
        self.package_key_sha256 = self.calculate_package_key()?;
        self.validate(scope)
    }

    pub(super) fn validate(&self, scope: &CatalogScopeV1) -> Result<()> {
        validate_sha256(&self.package_key_sha256, "catalog package key")?;
        validate_identity(&self.source_profile, "catalog package source profile")?;
        validate_identity(&self.name, "catalog package name")?;
        if self.version.is_empty() || self.checksum.is_empty() || self.download_url.is_empty() {
            return Err(Error::ConfigError(format!(
                "catalog package '{}' must have non-empty version, checksum, and download URL",
                self.name
            )));
        }
        if let Some(metadata) = &self.metadata {
            require_canonical_json_text(metadata, "catalog package metadata")?;
        }
        self.debian_source_pocket()?;
        crate::repository::versioning::validate_repo_version(self.version_scheme, &self.version)
            .map_err(|error| {
                Error::ParseError(format!(
                    "catalog package '{}' has invalid {} version: {error}",
                    self.name,
                    self.version_scheme.as_str()
                ))
            })?;
        if self.architecture.as_ref().is_some_and(String::is_empty) {
            return Err(Error::ConfigError(format!(
                "catalog package '{}' has an empty architecture",
                self.name
            )));
        }
        if (self.version_scheme == VersionScheme::Debian) != self.debian_multi_arch.is_some() {
            return Err(Error::ConfigError(format!(
                "catalog package '{}' must carry Debian Multi-Arch exactly when its version scheme is Debian",
                self.name
            )));
        }
        if self.source_profile != scope.profile() {
            return Err(Error::ConfigError(format!(
                "catalog package '{}' belongs to source profile '{}' instead of catalog profile '{}'",
                self.name,
                self.source_profile,
                scope.profile()
            )));
        }
        match (&self.origin, scope) {
            (
                CatalogPackageOriginV1::Source {
                    source_identity,
                    repository_identity,
                },
                CatalogScopeV1::Source {
                    source_identity: expected_source,
                    repository_identity: expected_repository,
                    ..
                },
            ) if source_identity == expected_source
                && repository_identity == expected_repository => {}
            (
                CatalogPackageOriginV1::Profile {
                    source_identity,
                    repository_identity,
                    source_snapshot_sha256,
                    ..
                },
                CatalogScopeV1::Profile { .. },
            ) => {
                validate_identity(source_identity, "catalog package source identity")?;
                validate_identity(repository_identity, "catalog package repository identity")?;
                validate_sha256(source_snapshot_sha256, "catalog package source snapshot")?;
            }
            _ => {
                return Err(Error::ConfigError(format!(
                    "catalog package '{}' origin does not belong to the catalog scope",
                    self.name
                )));
            }
        }
        let expected_key = self.calculate_package_key()?;
        if self.package_key_sha256 != expected_key {
            return Err(Error::ChecksumMismatch {
                expected: expected_key,
                actual: self.package_key_sha256.clone(),
            });
        }

        require_canonical_order(&self.provides, "catalog package provides")?;
        require_canonical_order(
            &self.requirement_groups,
            "catalog package requirement groups",
        )?;
        for provide in &self.provides {
            provide.validate()?;
        }
        for group in &self.requirement_groups {
            group.validate()?;
        }
        Ok(())
    }

    fn calculate_package_key(&self) -> Result<String> {
        #[derive(Serialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum PackageKeyOrigin<'a> {
            Source {
                source_identity: &'a str,
                repository_identity: &'a str,
            },
            Profile,
        }
        #[derive(Serialize)]
        struct PackageKeyInput<'a> {
            origin: PackageKeyOrigin<'a>,
            source_profile: &'a str,
            name: &'a str,
            version: &'a str,
            package_release: &'a str,
            architecture: &'a Option<String>,
            version_scheme: VersionScheme,
        }
        let origin = match &self.origin {
            CatalogPackageOriginV1::Source {
                source_identity,
                repository_identity,
            } => PackageKeyOrigin::Source {
                source_identity,
                repository_identity,
            },
            CatalogPackageOriginV1::Profile { .. } => PackageKeyOrigin::Profile,
        };
        canonical_sha256(&PackageKeyInput {
            origin,
            source_profile: &self.source_profile,
            name: &self.name,
            version: &self.version,
            package_release: &self.package_release,
            architecture: &self.architecture,
            version_scheme: self.version_scheme,
        })
    }

    /// Compare source-independent base-package truth inside one composed profile.
    ///
    /// Origin, package key, download URL, and typed Debian source-pocket
    /// placement are projections of the selected member. Every intrinsic
    /// package semantic and the authenticated payload identity must otherwise
    /// agree exactly before multiple origins may collapse.
    pub(in crate::repository) fn same_profile_base(&self, other: &Self) -> Result<bool> {
        let (_, self_metadata) = self.profile_semantic_metadata()?;
        let (_, other_metadata) = other.profile_semantic_metadata()?;
        Ok(self.source_profile == other.source_profile
            && self.name == other.name
            && self.version == other.version
            && self.package_release == other.package_release
            && self.architecture == other.architecture
            && self.debian_multi_arch == other.debian_multi_arch
            && self.description == other.description
            && self.checksum == other.checksum
            && self.size == other.size
            && self_metadata == other_metadata
            && self.is_security_update == other.is_security_update
            && self.severity == other.severity
            && self.cve_ids == other.cve_ids
            && self.advisory_id == other.advisory_id
            && self.advisory_url == other.advisory_url
            && self.version_scheme == other.version_scheme)
    }

    pub(in crate::repository) fn same_profile_record(&self, other: &Self) -> Result<bool> {
        Ok(self.same_profile_base(other)?
            && self.provides == other.provides
            && self.requirement_groups == other.requirement_groups)
    }
}

impl CatalogProvideRecordV1 {
    fn validate(&self) -> Result<()> {
        let kind = parse_capability_kind(&self.kind)?;
        validate_native_capability(&self.capability, "catalog provided capability", kind)?;
        ProvidedCapability {
            kind,
            name: self.capability.clone(),
            version: self.version.clone(),
            version_relation: self.version_relation,
            version_scheme: self.version_scheme,
            architecture_qualifier: self.architecture_qualifier.clone(),
            provenance: self.provenance.clone(),
        }
        .validate()
    }
}

impl CatalogRequirementGroupV1 {
    fn from_db(group: DbRequirementGroup, atoms: Vec<RepositoryRequirement>) -> Self {
        Self {
            kind: group.kind,
            behavior: group.behavior,
            description: group.description,
            native_text: group.native_text,
            expression_json: group.expression_json,
            atoms: atoms
                .into_iter()
                .map(CatalogRequirementAtomV1::from)
                .collect(),
        }
    }

    fn validate(&self) -> Result<()> {
        self.validate_base()?;
        if self.atoms.is_empty() {
            return Err(Error::ConfigError(
                "catalog requirement group must contain at least one atom".to_string(),
            ));
        }
        require_canonical_order(&self.atoms, "catalog requirement atoms")?;
        for atom in &self.atoms {
            atom.validate()?;
        }
        Ok(())
    }

    fn validate_base(&self) -> Result<()> {
        validate_identity(&self.kind, "catalog requirement group kind")?;
        validate_identity(&self.behavior, "catalog requirement group behavior")?;
        if RepositoryRequirementKind::from_str_exact(&self.kind).is_none() {
            return Err(Error::ConfigError(format!(
                "catalog requirement group has unknown kind '{}'",
                self.kind
            )));
        }
        if !matches!(self.behavior.as_str(), "hard" | "conditional") {
            return Err(Error::ConfigError(format!(
                "catalog requirement group has unknown behavior '{}'",
                self.behavior
            )));
        }
        require_canonical_json_text(
            &self.expression_json,
            "catalog requirement group expression",
        )?;
        serde_json::from_str::<RepositoryRequirementExpression>(&self.expression_json).map_err(
            |error| {
                Error::ParseError(format!(
                    "catalog requirement group expression is not the typed dependency grammar: {error}"
                ))
            },
        )?;
        Ok(())
    }
}

impl CatalogRequirementAtomV1 {
    fn validate(&self) -> Result<()> {
        let kind = parse_capability_kind(&self.kind)?;
        validate_native_capability(&self.capability, "catalog requirement capability", kind)?;
        if !matches!(
            self.dependency_type.as_str(),
            "runtime" | "optional" | "build"
        ) {
            return Err(Error::ConfigError(format!(
                "catalog requirement atom has unknown dependency type '{}'",
                self.dependency_type
            )));
        }
        Ok(())
    }
}

impl CatalogContentV1 {
    pub fn new(
        scope: CatalogScopeV1,
        mut source_evidence: Vec<CatalogSourceEvidenceV1>,
        mut packages: Vec<CatalogPackageRecordV1>,
    ) -> Result<Self> {
        canonicalize_evidence(&mut source_evidence);
        for package in &mut packages {
            package.canonicalize()?;
            package.package_key_sha256 = package.calculate_package_key()?;
        }
        packages.sort_by(|left, right| left.package_key_sha256.cmp(&right.package_key_sha256));
        let content = Self {
            schema_version: CATALOG_CONTENT_SCHEMA_V1,
            scope,
            source_evidence,
            packages,
        };
        content.validate()?;
        Ok(content)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_CONTENT_SCHEMA_V1 {
            return Err(Error::ConfigError(format!(
                "catalog content schema {} is unsupported; expected {}",
                self.schema_version, CATALOG_CONTENT_SCHEMA_V1
            )));
        }
        self.scope.validate()?;
        self.validate_source_evidence()?;
        let mut keys = BTreeSet::new();
        let mut previous = None;
        for package in &self.packages {
            package.validate(&self.scope)?;
            self.validate_package_origin(package)?;
            if !keys.insert(&package.package_key_sha256) {
                return Err(Error::ConflictError(format!(
                    "catalog repeats exact package key {}",
                    package.package_key_sha256
                )));
            }
            if previous.is_some_and(|key| key >= &package.package_key_sha256) {
                return Err(Error::ConfigError(
                    "catalog packages must be strictly ordered by package key".to_string(),
                ));
            }
            previous = Some(&package.package_key_sha256);
        }
        Ok(())
    }

    fn validate_source_evidence(&self) -> Result<()> {
        if self.source_evidence.is_empty() {
            return Err(Error::ConfigError(
                "catalog must bind at least one authenticated source evidence record".to_string(),
            ));
        }
        let mut canonical = self.source_evidence.clone();
        canonicalize_evidence(&mut canonical);
        if canonical != self.source_evidence {
            return Err(Error::ConfigError(
                "catalog source evidence is not in canonical order".to_string(),
            ));
        }
        let mut keys = BTreeSet::new();
        for (index, evidence) in self.source_evidence.iter().enumerate() {
            match (evidence, &self.scope) {
                (
                    CatalogSourceEvidenceV1::AuthenticatedObject {
                        role,
                        source_path,
                        sha256,
                        ..
                    },
                    CatalogScopeV1::Source { .. },
                ) => {
                    validate_relative_source_path(source_path)?;
                    validate_sha256(sha256, "catalog authenticated source object")?;
                    if !keys.insert(format!("object:{role:?}")) {
                        return Err(Error::ConflictError(format!(
                            "catalog repeats authenticated source object role {role:?}"
                        )));
                    }
                }
                (
                    CatalogSourceEvidenceV1::SourceSnapshot {
                        member_ordinal,
                        source_identity,
                        repository_identity,
                        source_snapshot_sha256,
                    },
                    CatalogScopeV1::Profile { .. },
                ) => {
                    let expected_ordinal = u32::try_from(index).map_err(|_| {
                        Error::ConfigError(
                            "profile catalog contains too many source snapshots".to_string(),
                        )
                    })?;
                    if *member_ordinal != expected_ordinal {
                        return Err(Error::ConfigError(format!(
                            "profile catalog source snapshot ordinal {member_ordinal} is noncanonical; expected {expected_ordinal}"
                        )));
                    }
                    validate_identity(source_identity, "catalog evidence source identity")?;
                    validate_identity(repository_identity, "catalog evidence repository identity")?;
                    validate_sha256(source_snapshot_sha256, "catalog evidence source snapshot")?;
                    if !keys.insert(format!("snapshot:{repository_identity}")) {
                        return Err(Error::ConflictError(format!(
                            "profile catalog repeats repository identity '{repository_identity}'"
                        )));
                    }
                }
                _ => {
                    return Err(Error::ConfigError(
                        "catalog source evidence kind does not belong to its scope".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_package_origin(&self, package: &CatalogPackageRecordV1) -> Result<()> {
        let CatalogPackageOriginV1::Profile {
            member_ordinal,
            source_identity,
            repository_identity,
            source_snapshot_sha256,
        } = &package.origin
        else {
            return Ok(());
        };
        let Some(evidence) = self.source_evidence.get(*member_ordinal as usize) else {
            return Err(Error::ConfigError(format!(
                "catalog package '{}' names missing source member ordinal {member_ordinal}",
                package.name
            )));
        };
        match evidence {
            CatalogSourceEvidenceV1::SourceSnapshot {
                member_ordinal: expected_ordinal,
                source_identity: expected_source,
                repository_identity: expected_repository,
                source_snapshot_sha256: expected_snapshot,
            } if expected_ordinal == member_ordinal
                && expected_source == source_identity
                && expected_repository == repository_identity
                && expected_snapshot == source_snapshot_sha256 =>
            {
                Ok(())
            }
            _ => Err(Error::ConfigError(format!(
                "catalog package '{}' origin does not resolve to its declared source evidence",
                package.name
            ))),
        }
    }

    pub fn logical_digest_sha256(&self) -> Result<String> {
        self.validate()?;
        let mut digest = CatalogLogicalDigestV1::new(&self.scope, &self.source_evidence)?;
        for package in &self.packages {
            digest.package(package)?;
        }
        Ok(digest.finish()?.0)
    }

    pub fn counts(&self) -> Result<CatalogCountsV1> {
        self.validate()?;
        let mut counts = CatalogCountsV1 {
            packages: u64::try_from(self.packages.len()).map_err(|_| {
                Error::InternalError("catalog package count exceeds u64".to_string())
            })?,
            source_evidence: u64::try_from(self.source_evidence.len()).map_err(|_| {
                Error::InternalError("catalog source evidence count exceeds u64".to_string())
            })?,
            ..CatalogCountsV1::default()
        };
        for package in &self.packages {
            counts.provides = counts
                .provides
                .checked_add(u64::try_from(package.provides.len()).map_err(|_| {
                    Error::InternalError("catalog provide count exceeds u64".to_string())
                })?)
                .ok_or_else(|| {
                    Error::InternalError("catalog provide count overflow".to_string())
                })?;
            counts.requirement_groups = counts
                .requirement_groups
                .checked_add(
                    u64::try_from(package.requirement_groups.len()).map_err(|_| {
                        Error::InternalError(
                            "catalog requirement group count exceeds u64".to_string(),
                        )
                    })?,
                )
                .ok_or_else(|| {
                    Error::InternalError("catalog requirement group count overflow".to_string())
                })?;
            for group in &package.requirement_groups {
                counts.requirement_atoms = counts
                    .requirement_atoms
                    .checked_add(u64::try_from(group.atoms.len()).map_err(|_| {
                        Error::InternalError(
                            "catalog requirement atom count exceeds u64".to_string(),
                        )
                    })?)
                    .ok_or_else(|| {
                        Error::InternalError("catalog requirement atom count overflow".to_string())
                    })?;
            }
        }
        Ok(counts)
    }
}

impl From<RepositoryProvide> for CatalogProvideRecordV1 {
    fn from(value: RepositoryProvide) -> Self {
        Self {
            capability: value.capability,
            version: value.version,
            version_relation: value.version_relation,
            kind: value.kind,
            raw: value.raw,
            version_scheme: value.version_scheme,
            architecture_qualifier: value.architecture_qualifier,
            provenance: value.provenance,
        }
    }
}

impl From<RepositoryRequirement> for CatalogRequirementAtomV1 {
    fn from(value: RepositoryRequirement) -> Self {
        Self {
            capability: value.capability,
            version_constraint: value.version_constraint,
            kind: value.kind,
            dependency_type: value.dependency_type,
            raw: value.raw,
        }
    }
}

fn canonical_sha256(value: &impl Serialize) -> Result<String> {
    let bytes = crate::json::canonical_json(value).map_err(|error| {
        Error::ParseError(format!(
            "failed to serialize canonical catalog record: {error}"
        ))
    })?;
    Ok(crate::hash::sha256(&bytes))
}

fn parse_capability_kind(value: &str) -> Result<RepositoryCapabilityKind> {
    match value {
        "package" => Ok(RepositoryCapabilityKind::PackageName),
        "virtual" => Ok(RepositoryCapabilityKind::Virtual),
        "soname" => Ok(RepositoryCapabilityKind::Soname),
        "file" => Ok(RepositoryCapabilityKind::File),
        "path" => Ok(RepositoryCapabilityKind::Path),
        "binary" => Ok(RepositoryCapabilityKind::Binary),
        "pkgconfig" => Ok(RepositoryCapabilityKind::PkgConfig),
        "pkgconfig32" => Ok(RepositoryCapabilityKind::PkgConfig32),
        "comar" => Ok(RepositoryCapabilityKind::Comar),
        "generic" => Ok(RepositoryCapabilityKind::Generic),
        _ => Err(Error::ConfigError(format!(
            "catalog capability has unknown kind '{value}'"
        ))),
    }
}

fn validate_native_capability(
    value: &str,
    label: &str,
    kind: RepositoryCapabilityKind,
) -> Result<()> {
    if value.is_empty() {
        return Err(Error::ConfigError(format!(
            "{label} must be non-empty source-native UTF-8"
        )));
    }
    if !matches!(
        kind,
        RepositoryCapabilityKind::File | RepositoryCapabilityKind::Path
    ) && (value.trim() != value || value.chars().any(char::is_control))
    {
        return Err(Error::ConfigError(format!(
            "{label} must not contain surrounding whitespace or control characters"
        )));
    }
    Ok(())
}

fn canonicalize_json_text(value: &mut String, label: &str) -> Result<()> {
    let parsed: serde_json::Value = serde_json::from_str(value)
        .map_err(|error| Error::ParseError(format!("{label} is not valid JSON: {error}")))?;
    let canonical = crate::json::canonical_json(&parsed)
        .map_err(|error| Error::ParseError(format!("canonicalize {label}: {error}")))?;
    *value = String::from_utf8(canonical).map_err(|error| {
        Error::InternalError(format!("canonical {label} was not UTF-8: {error}"))
    })?;
    Ok(())
}

fn require_canonical_json_text(value: &str, label: &str) -> Result<()> {
    let mut canonical = value.to_string();
    canonicalize_json_text(&mut canonical, label)?;
    if canonical != value {
        return Err(Error::ConfigError(format!(
            "{label} is valid JSON but not canonical JSON"
        )));
    }
    Ok(())
}

fn canonical_sort<T: Serialize>(values: &mut Vec<T>) -> Result<()> {
    let drained = std::mem::take(values);
    let mut keyed = drained
        .into_iter()
        .map(|value| {
            crate::json::canonical_json(&value)
                .map(|key| (key, value))
                .map_err(|error| {
                    Error::ParseError(format!(
                        "failed to serialize catalog record for canonical ordering: {error}"
                    ))
                })
        })
        .collect::<Result<Vec<_>>>()?;
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    *values = keyed.into_iter().map(|(_, value)| value).collect();
    Ok(())
}

fn canonicalize_evidence(values: &mut [CatalogSourceEvidenceV1]) {
    values.sort_by(|left, right| match (left, right) {
        (
            CatalogSourceEvidenceV1::AuthenticatedObject {
                role: left_role,
                source_path: left_path,
                ..
            },
            CatalogSourceEvidenceV1::AuthenticatedObject {
                role: right_role,
                source_path: right_path,
                ..
            },
        ) => (left_role, left_path).cmp(&(right_role, right_path)),
        (
            CatalogSourceEvidenceV1::SourceSnapshot {
                member_ordinal: left,
                ..
            },
            CatalogSourceEvidenceV1::SourceSnapshot {
                member_ordinal: right,
                ..
            },
        ) => left.cmp(right),
        (CatalogSourceEvidenceV1::AuthenticatedObject { .. }, _) => std::cmp::Ordering::Less,
        (CatalogSourceEvidenceV1::SourceSnapshot { .. }, _) => std::cmp::Ordering::Greater,
    });
}

fn require_canonical_order<T: Serialize>(values: &[T], label: &str) -> Result<()> {
    let mut previous: Option<Vec<u8>> = None;
    for value in values {
        let key = crate::json::canonical_json(value).map_err(|error| {
            Error::ParseError(format!(
                "failed to serialize {label} for canonical ordering: {error}"
            ))
        })?;
        if previous.as_ref().is_some_and(|previous| previous > &key) {
            return Err(Error::ConfigError(format!(
                "{label} are not in canonical order"
            )));
        }
        previous = Some(key);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
