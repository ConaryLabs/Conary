// crates/conary-core/src/repository/catalog/record/digest.rs

//! Incremental logical digest ownership for canonical catalog records.

use super::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogContentV1, CatalogPackageRecordV1, CatalogProvideRecordV1,
    CatalogRequirementGroupV1, CatalogScopeV1, CatalogSourceEvidenceV1,
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

    /// Begin one package whose relations will arrive in canonical order.
    ///
    /// The package must carry no in-memory provides or requirement groups. The
    /// returned writer emits those arrays one row at a time while preserving
    /// the exact canonical JSON identity used by [`Self::package`].
    pub(in crate::repository) fn begin_package(
        &mut self,
        package: &CatalogPackageRecordV1,
    ) -> Result<CatalogPackageLogicalDigestV1<'_>> {
        if !package.provides.is_empty() || !package.requirement_groups.is_empty() {
            return Err(Error::InternalError(
                "streamed catalog digest package must not retain relation arrays".to_string(),
            ));
        }
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
        let package_json = crate::json::canonical_json(package).map_err(|error| {
            Error::ParseError(format!(
                "serialize catalog package base for logical digest: {error}"
            ))
        })?;
        let (prefix, middle, suffix) = split_package_relation_arrays(&package_json)?;
        if !self.first_package {
            self.hasher.update(b",");
        }
        self.first_package = false;
        self.hasher.update(&prefix);
        self.previous_package_key = Some(package.package_key_sha256.clone());
        Ok(CatalogPackageLogicalDigestV1 {
            digest: self,
            middle,
            suffix,
            previous_provide: None,
            previous_requirement_group: None,
            first_provide: true,
            first_requirement_group: true,
            requirements_started: false,
        })
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

/// Streaming relation writer for one canonical package digest entry.
pub(in crate::repository) struct CatalogPackageLogicalDigestV1<'a> {
    digest: &'a mut CatalogLogicalDigestV1,
    middle: Vec<u8>,
    suffix: Vec<u8>,
    previous_provide: Option<Vec<u8>>,
    previous_requirement_group: Option<Vec<u8>>,
    first_provide: bool,
    first_requirement_group: bool,
    requirements_started: bool,
}

impl CatalogPackageLogicalDigestV1<'_> {
    pub(in crate::repository) fn provide(
        &mut self,
        provide: &CatalogProvideRecordV1,
    ) -> Result<()> {
        if self.requirements_started {
            return Err(Error::InternalError(
                "catalog digest provide arrived after requirement groups".to_string(),
            ));
        }
        provide.validate()?;
        let canonical = canonical_relation(provide, "catalog provide")?;
        require_next_canonical(
            &mut self.previous_provide,
            &canonical,
            "catalog package provides",
        )?;
        if !self.first_provide {
            self.digest.hasher.update(b",");
        }
        self.first_provide = false;
        self.digest.hasher.update(&canonical);
        self.digest.counts.provides = checked_add(self.digest.counts.provides, 1, "provide")?;
        Ok(())
    }

    pub(in crate::repository) fn requirement_group(
        &mut self,
        group: &CatalogRequirementGroupV1,
    ) -> Result<()> {
        self.start_requirements();
        group.validate()?;
        let canonical = canonical_relation(group, "catalog requirement group")?;
        require_next_canonical(
            &mut self.previous_requirement_group,
            &canonical,
            "catalog package requirement groups",
        )?;
        if !self.first_requirement_group {
            self.digest.hasher.update(b",");
        }
        self.first_requirement_group = false;
        self.digest.hasher.update(&canonical);
        self.digest.counts.requirement_groups = checked_add(
            self.digest.counts.requirement_groups,
            1,
            "requirement group",
        )?;
        self.digest.counts.requirement_atoms = checked_add(
            self.digest.counts.requirement_atoms,
            u64::try_from(group.atoms.len()).map_err(|_| {
                Error::InternalError("catalog requirement atom count exceeds u64".to_string())
            })?,
            "requirement atom",
        )?;
        Ok(())
    }

    pub(in crate::repository) fn finish(mut self) -> Result<()> {
        self.start_requirements();
        self.digest.hasher.update(&self.suffix);
        self.digest.counts.packages = checked_add(self.digest.counts.packages, 1, "package")?;
        Ok(())
    }

    fn start_requirements(&mut self) {
        if !self.requirements_started {
            self.digest.hasher.update(&self.middle);
            self.requirements_started = true;
        }
    }
}

fn split_package_relation_arrays(bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let provides = relation_array_open(bytes, b"\"provides\":[]", "provides")?;
    let requirements =
        relation_array_open(bytes, b"\"requirement_groups\":[]", "requirement groups")?;
    if provides >= requirements {
        return Err(Error::InternalError(
            "canonical catalog package relation fields are out of order".to_string(),
        ));
    }
    Ok((
        bytes[..=provides].to_vec(),
        bytes[provides + 1..=requirements].to_vec(),
        bytes[requirements + 1..].to_vec(),
    ))
}

fn relation_array_open(bytes: &[u8], marker: &[u8], label: &str) -> Result<usize> {
    let marker_start = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .ok_or_else(|| {
            Error::InternalError(format!(
                "canonical catalog package is missing its empty {label} array"
            ))
        })?;
    Ok(marker_start + marker.len() - 2)
}

fn canonical_relation<T: serde::Serialize>(value: &T, label: &str) -> Result<Vec<u8>> {
    crate::json::canonical_json(value).map_err(|error| {
        Error::ParseError(format!("serialize {label} for logical digest: {error}"))
    })
}

fn require_next_canonical(
    previous: &mut Option<Vec<u8>>,
    current: &[u8],
    label: &str,
) -> Result<()> {
    if previous
        .as_ref()
        .is_some_and(|previous| previous.as_slice() > current)
    {
        return Err(Error::ConfigError(format!(
            "{label} are not in canonical order"
        )));
    }
    *previous = Some(current.to_vec());
    Ok(())
}

fn checked_add(current: u64, increment: u64, label: &str) -> Result<u64> {
    current
        .checked_add(increment)
        .ok_or_else(|| Error::InternalError(format!("catalog {label} count overflow")))
}
