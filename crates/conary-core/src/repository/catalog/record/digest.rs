// crates/conary-core/src/repository/catalog/record/digest.rs

//! Incremental logical digest ownership for canonical catalog records.

use super::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogContentV1, CatalogPackageRecordV1, CatalogScopeV1,
    CatalogSourceEvidenceV1,
};
use crate::error::{Error, Result};
use crate::repository::catalog::CatalogCountsV1;

/// Incremental form of the canonical `CatalogContentV1` digest.
///
/// Top-level object keys are emitted in the same lexical order as
/// `crate::json::canonical_json`, while package records arrive one at a time in
/// package-key order. This keeps the persisted V1 logical identity byte-for-byte
/// stable without constructing a repository-sized JSON value.
pub(in crate::repository) struct CatalogLogicalDigestV1 {
    hasher: crate::hash::Hasher,
    scope: CatalogScopeV1,
    counts: CatalogCountsV1,
    previous_package_key: Option<String>,
    first_package: bool,
    source_evidence: Vec<CatalogSourceEvidenceV1>,
}

impl CatalogLogicalDigestV1 {
    pub(in crate::repository) fn new(
        scope: &CatalogScopeV1,
        source_evidence: &[CatalogSourceEvidenceV1],
    ) -> Result<Self> {
        scope.validate()?;
        let content = CatalogContentV1 {
            schema_version: CATALOG_CONTENT_SCHEMA_V1,
            scope: scope.clone(),
            source_evidence: source_evidence.to_vec(),
            packages: Vec::new(),
        };
        content.validate_source_evidence()?;
        let mut hasher = crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256);
        hasher.update(b"{\"packages\":[");
        Ok(Self {
            hasher,
            scope: scope.clone(),
            counts: CatalogCountsV1 {
                source_evidence: u64::try_from(source_evidence.len()).map_err(|_| {
                    Error::InternalError("catalog source evidence count exceeds u64".to_string())
                })?,
                ..CatalogCountsV1::default()
            },
            previous_package_key: None,
            first_package: true,
            source_evidence: source_evidence.to_vec(),
        })
    }

    pub(in crate::repository) fn package(
        &mut self,
        package: &CatalogPackageRecordV1,
    ) -> Result<()> {
        package.validate(&self.scope)?;
        if self
            .previous_package_key
            .as_deref()
            .is_some_and(|previous| previous >= package.package_key_sha256.as_str())
        {
            return Err(Error::ConfigError(
                "catalog digest packages must be strictly ordered by package key".to_string(),
            ));
        }
        if !self.first_package {
            self.hasher.update(b",");
        }
        self.first_package = false;
        self.hasher
            .update(&crate::json::canonical_json(package).map_err(|error| {
                Error::ParseError(format!(
                    "serialize catalog package for logical digest: {error}"
                ))
            })?);
        self.previous_package_key = Some(package.package_key_sha256.clone());
        self.counts.packages = checked_add(self.counts.packages, 1, "package")?;
        self.counts.provides = checked_add(
            self.counts.provides,
            u64::try_from(package.provides.len()).map_err(|_| {
                Error::InternalError("catalog provide count exceeds u64".to_string())
            })?,
            "provide",
        )?;
        self.counts.requirement_groups = checked_add(
            self.counts.requirement_groups,
            u64::try_from(package.requirement_groups.len()).map_err(|_| {
                Error::InternalError("catalog requirement group count exceeds u64".to_string())
            })?,
            "requirement group",
        )?;
        for group in &package.requirement_groups {
            self.counts.requirement_atoms = checked_add(
                self.counts.requirement_atoms,
                u64::try_from(group.atoms.len()).map_err(|_| {
                    Error::InternalError("catalog requirement atom count exceeds u64".to_string())
                })?,
                "requirement atom",
            )?;
        }
        Ok(())
    }

    pub(in crate::repository) fn finish(mut self) -> Result<(String, CatalogCountsV1)> {
        self.hasher.update(b"],\"schema_version\":1,\"scope\":");
        self.hasher
            .update(&crate::json::canonical_json(&self.scope).map_err(|error| {
                Error::ParseError(format!(
                    "serialize catalog scope for logical digest: {error}"
                ))
            })?);
        self.hasher.update(b",\"source_evidence\":[");
        for (index, evidence) in self.source_evidence.iter().enumerate() {
            if index > 0 {
                self.hasher.update(b",");
            }
            self.hasher
                .update(&crate::json::canonical_json(evidence).map_err(|error| {
                    Error::ParseError(format!(
                        "serialize catalog source evidence for logical digest: {error}"
                    ))
                })?);
        }
        self.hasher.update(b"]}");
        Ok((self.hasher.finalize().to_string(), self.counts))
    }
}

fn checked_add(current: u64, increment: u64, label: &str) -> Result<u64> {
    current
        .checked_add(increment)
        .ok_or_else(|| Error::InternalError(format!("catalog {label} count overflow")))
}
