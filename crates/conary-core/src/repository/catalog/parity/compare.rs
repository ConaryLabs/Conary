// crates/conary-core/src/repository/catalog/parity/compare.rs

//! Bounded merge comparison between native oracle rows and profile catalogs.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::super::{
    CatalogPackageOriginV1, CatalogPackageRecordV1, CatalogReader, CatalogScopeV1,
    CatalogSourceEvidenceV1, ProfileRevisionV2,
};
use super::contract::{NativeParityCountsV1, NativeParityPackageV1};
use super::io::NativeParityOracleReader;
use crate::error::Error as ConaryError;
use crate::repository::dependency_model::RepositoryRequirementKind;

pub const NATIVE_PARITY_COMPARISON_SCHEMA_V1: u32 = 1;

/// Safe exact package identity attached to a typed parity mismatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeParityPackageIdentityV1 {
    pub package_key_sha256: String,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: Option<String>,
}

impl NativeParityPackageIdentityV1 {
    fn from_oracle(package: &NativeParityPackageV1) -> Self {
        Self {
            package_key_sha256: package.package_key_sha256.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
            package_release: package.package_release.clone(),
            architecture: package.architecture.clone(),
        }
    }

    fn from_catalog(package: &CatalogPackageRecordV1) -> Self {
        Self {
            package_key_sha256: package.package_key_sha256.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
            package_release: package.package_release.clone(),
            architecture: package.architecture.clone(),
        }
    }
}

/// Exact authority family that differs for one common package identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeParityFactV1 {
    IdentityVariant,
    OriginPrecedence,
    PayloadAuthority,
    Providers,
    GroupedRequirements,
    NegativeRelations,
}

/// First typed divergence in the canonical full-catalog merge comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeParityMismatchV1 {
    OracleOnlyPackage {
        package: NativeParityPackageIdentityV1,
    },
    CandidateOnlyPackage {
        package: NativeParityPackageIdentityV1,
    },
    PackageFacts {
        package: NativeParityPackageIdentityV1,
        facts: Vec<NativeParityFactV1>,
    },
}

/// A complete successful comparison record suitable for later promotion
/// proof binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeParityComparisonV1 {
    pub schema_version: u32,
    pub profile: String,
    pub profile_revision_sha256: String,
    pub oracle_manifest_sha256: String,
    pub counts: NativeParityCountsV1,
}

#[derive(Debug, Error)]
pub enum NativeParityComparisonError {
    #[error("native parity oracle is invalid: {0}")]
    Oracle(#[source] ConaryError),
    #[error("native parity candidate is invalid: {0}")]
    Candidate(#[source] ConaryError),
    #[error("native parity candidate diverges from the pinned oracle: {0:?}")]
    Mismatch(Box<NativeParityMismatchV1>),
}

pub fn compare_native_parity_oracle(
    profile: &ProfileRevisionV2,
    candidate: &CatalogReader,
    oracle: &NativeParityOracleReader,
) -> std::result::Result<NativeParityComparisonV1, NativeParityComparisonError> {
    oracle
        .manifest()
        .validate_profile(profile)
        .map_err(NativeParityComparisonError::Oracle)?;
    validate_candidate_binding(profile, candidate)
        .map_err(NativeParityComparisonError::Candidate)?;

    let mut cursor = oracle
        .cursor()
        .map_err(NativeParityComparisonError::Oracle)?;
    let mut oracle_package = cursor
        .next_package()
        .map_err(NativeParityComparisonError::Oracle)?;
    let mut mismatch = None;
    let mut oracle_error = None;
    let candidate_result = candidate.for_each_package(|catalog_package| {
        let Some(expected) = oracle_package.as_ref() else {
            mismatch = Some(NativeParityMismatchV1::CandidateOnlyPackage {
                package: NativeParityPackageIdentityV1::from_catalog(&catalog_package),
            });
            return Err(comparison_sentinel());
        };
        match expected
            .package_key_sha256
            .as_str()
            .cmp(catalog_package.package_key_sha256.as_str())
        {
            std::cmp::Ordering::Less => {
                mismatch = Some(NativeParityMismatchV1::OracleOnlyPackage {
                    package: NativeParityPackageIdentityV1::from_oracle(expected),
                });
                return Err(comparison_sentinel());
            }
            std::cmp::Ordering::Greater => {
                mismatch = Some(NativeParityMismatchV1::CandidateOnlyPackage {
                    package: NativeParityPackageIdentityV1::from_catalog(&catalog_package),
                });
                return Err(comparison_sentinel());
            }
            std::cmp::Ordering::Equal => {}
        }
        let facts = differing_facts(expected, &catalog_package);
        if !facts.is_empty() {
            mismatch = Some(NativeParityMismatchV1::PackageFacts {
                package: NativeParityPackageIdentityV1::from_catalog(&catalog_package),
                facts,
            });
            return Err(comparison_sentinel());
        }
        match cursor.next_package() {
            Ok(next) => oracle_package = next,
            Err(error) => {
                oracle_error = Some(error);
                return Err(comparison_sentinel());
            }
        }
        Ok(())
    });
    if let Some(error) = oracle_error {
        return Err(NativeParityComparisonError::Oracle(error));
    }
    if let Some(mismatch) = mismatch {
        return Err(NativeParityComparisonError::Mismatch(Box::new(mismatch)));
    }
    candidate_result.map_err(NativeParityComparisonError::Candidate)?;
    if let Some(package) = oracle_package {
        return Err(NativeParityComparisonError::Mismatch(Box::new(
            NativeParityMismatchV1::OracleOnlyPackage {
                package: NativeParityPackageIdentityV1::from_oracle(&package),
            },
        )));
    }

    Ok(NativeParityComparisonV1 {
        schema_version: NATIVE_PARITY_COMPARISON_SCHEMA_V1,
        profile: profile.profile.clone(),
        profile_revision_sha256: profile
            .manifest_sha256()
            .map_err(NativeParityComparisonError::Candidate)?,
        oracle_manifest_sha256: oracle
            .manifest()
            .manifest_sha256()
            .map_err(NativeParityComparisonError::Oracle)?,
        counts: oracle.manifest().artifact.counts,
    })
}

fn validate_candidate_binding(
    profile: &ProfileRevisionV2,
    candidate: &CatalogReader,
) -> crate::error::Result<()> {
    profile.validate()?;
    let binding = candidate.binding();
    if binding.scope
        != (CatalogScopeV1::Profile {
            profile: profile.profile.clone(),
        })
        || binding.artifact != profile.catalog
        || binding.logical_digest_sha256 != profile.logical_digest_sha256
        || binding.counts != profile.counts
    {
        return Err(ConaryError::ConflictError(format!(
            "native parity candidate does not match profile revision '{}'",
            profile.profile
        )));
    }
    let evidence = profile
        .members
        .iter()
        .map(|member| CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: member.ordinal,
            source_identity: member.source_identity.clone(),
            repository_identity: member.repository_identity.clone(),
            source_snapshot_sha256: member.source_snapshot_sha256.clone(),
        })
        .collect::<Vec<_>>();
    if candidate.source_evidence()? != evidence {
        return Err(ConaryError::ConflictError(format!(
            "native parity candidate member evidence does not match profile revision '{}'",
            profile.profile
        )));
    }
    Ok(())
}

fn differing_facts(
    oracle: &NativeParityPackageV1,
    candidate: &CatalogPackageRecordV1,
) -> Vec<NativeParityFactV1> {
    let mut facts = Vec::new();
    let CatalogPackageOriginV1::Profile {
        member_ordinal,
        source_identity,
        repository_identity,
        source_snapshot_sha256,
    } = &candidate.origin
    else {
        facts.push(NativeParityFactV1::OriginPrecedence);
        return facts;
    };
    if oracle.member_ordinal != *member_ordinal
        || oracle.source_identity != *source_identity
        || oracle.repository_identity != *repository_identity
        || oracle.source_snapshot_sha256 != *source_snapshot_sha256
    {
        facts.push(NativeParityFactV1::OriginPrecedence);
    }
    if oracle.source_profile != candidate.source_profile
        || oracle.name != candidate.name
        || oracle.version != candidate.version
        || oracle.package_release != candidate.package_release
        || oracle.architecture != candidate.architecture
        || oracle.debian_multi_arch != candidate.debian_multi_arch
        || oracle.version_scheme != candidate.version_scheme
    {
        facts.push(NativeParityFactV1::IdentityVariant);
    }
    if oracle.checksum != candidate.checksum
        || oracle.size != candidate.size
        || oracle.download_url != candidate.download_url
    {
        facts.push(NativeParityFactV1::PayloadAuthority);
    }
    if oracle.provides != candidate.provides {
        facts.push(NativeParityFactV1::Providers);
    }
    if !requirements_equal(oracle, candidate, false) {
        facts.push(NativeParityFactV1::GroupedRequirements);
    }
    if !requirements_equal(oracle, candidate, true) {
        facts.push(NativeParityFactV1::NegativeRelations);
    }
    facts
}

fn requirements_equal(
    oracle: &NativeParityPackageV1,
    candidate: &CatalogPackageRecordV1,
    negative: bool,
) -> bool {
    oracle
        .requirement_groups
        .iter()
        .filter(|group| requirement_is_negative(&group.kind) == Some(negative))
        .eq(candidate
            .requirement_groups
            .iter()
            .filter(|group| requirement_is_negative(&group.kind) == Some(negative)))
}

fn requirement_is_negative(kind: &str) -> Option<bool> {
    RepositoryRequirementKind::from_str_exact(kind)
        .map(RepositoryRequirementKind::is_negative_relation)
}

fn comparison_sentinel() -> ConaryError {
    ConaryError::ConflictError("typed native parity comparison stopped".to_string())
}
