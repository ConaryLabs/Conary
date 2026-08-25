// conary-core/src/repository/catalog/capacity.rs

//! Typed scratch-space contracts for immutable catalog construction.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::Result;

use super::SourceMetadataObjectRoleV1;
use super::contract::validate_relative_source_path;

/// Current schema for exact SQLite catalog-finalization scratch evidence.
pub const CATALOG_FINALIZATION_SCRATCH_SCHEMA_V1: u32 = 1;

/// Current schema for exact immutable catalog-copy scratch evidence.
pub const CATALOG_COPY_SCRATCH_SCHEMA_V1: u32 = 1;

/// Current schema for exact authenticated-metadata staging evidence.
pub const CATALOG_METADATA_SCRATCH_SCHEMA_V1: u32 = 1;

/// One authenticated child object whose signed bytes coexist in run-local
/// storage while a native repository projection is built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogMetadataObjectScratchV1 {
    pub role: SourceMetadataObjectRoleV1,
    pub source_path: String,
    pub size: u64,
}

/// Exact additional file bytes required to stage authenticated native
/// metadata before parser or catalog mutation begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogMetadataScratchV1 {
    /// Contract schema version.
    pub schema_version: u32,
    /// Canonically ordered authenticated child allocations.
    pub objects: Vec<CatalogMetadataObjectScratchV1>,
    /// Sum of every staged child object's signed byte length.
    pub required_additional_bytes: u64,
}

impl CatalogMetadataScratchV1 {
    /// Derive a complete staging allocation from signed child-object facts.
    pub fn from_signed_objects(mut objects: Vec<CatalogMetadataObjectScratchV1>) -> Result<Self> {
        objects.sort_by(|left, right| {
            left.role
                .cmp(&right.role)
                .then_with(|| left.source_path.cmp(&right.source_path))
        });
        let required_additional_bytes = objects.iter().try_fold(0_u64, |total, object| {
            total.checked_add(object.size).ok_or_else(|| {
                crate::Error::IoError(
                    "catalog metadata scratch-space arithmetic overflow".to_string(),
                )
            })
        })?;
        let requirement = Self {
            schema_version: CATALOG_METADATA_SCRATCH_SCHEMA_V1,
            objects,
            required_additional_bytes,
        };
        requirement.validate()?;
        Ok(requirement)
    }

    /// Reject missing, repeated, noncanonical, or contradictory object facts.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_METADATA_SCRATCH_SCHEMA_V1 || self.objects.is_empty() {
            return Err(crate::Error::ConfigError(
                "catalog metadata scratch evidence has invalid schema or no objects".to_string(),
            ));
        }
        let mut previous: Option<&CatalogMetadataObjectScratchV1> = None;
        let mut required_additional_bytes = 0_u64;
        for object in &self.objects {
            validate_relative_source_path(&object.source_path)?;
            if object.size == 0 {
                return Err(crate::Error::ConfigError(
                    "catalog metadata scratch evidence has a zero-sized object".to_string(),
                ));
            }
            if let Some(previous) = previous
                && previous.role >= object.role
            {
                return Err(crate::Error::ConfigError(
                    "catalog metadata scratch objects are repeated or noncanonical".to_string(),
                ));
            }
            required_additional_bytes = required_additional_bytes
                .checked_add(object.size)
                .ok_or_else(|| {
                    crate::Error::IoError(
                        "catalog metadata scratch-space arithmetic overflow".to_string(),
                    )
                })?;
            previous = Some(object);
        }
        if required_additional_bytes != self.required_additional_bytes {
            return Err(crate::Error::ConfigError(
                "catalog metadata scratch evidence contradicts its signed object byte facts"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// Exact additional filesystem allocation required to copy one verified
/// catalog and its canonical manifest into a private publication stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogCopyScratchV1 {
    /// Contract schema version.
    pub schema_version: u32,
    /// Exact verified catalog artifact bytes copied into the stage.
    pub catalog_bytes: u64,
    /// Exact canonical manifest bytes written beside the catalog.
    pub manifest_bytes: u64,
    /// Sum of all additional file bytes created by the copy.
    pub required_additional_bytes: u64,
}

impl CatalogCopyScratchV1 {
    /// Derive the complete copy allocation from exact artifact bytes.
    pub fn from_exact_bytes(catalog_bytes: u64, manifest_bytes: u64) -> Result<Self> {
        if catalog_bytes == 0 || manifest_bytes == 0 {
            return Err(crate::Error::ConfigError(
                "catalog copy admission requires positive artifact byte facts".to_string(),
            ));
        }
        let required_additional_bytes =
            catalog_bytes.checked_add(manifest_bytes).ok_or_else(|| {
                crate::Error::IoError("catalog copy scratch-space arithmetic overflow".to_string())
            })?;
        let requirement = Self {
            schema_version: CATALOG_COPY_SCRATCH_SCHEMA_V1,
            catalog_bytes,
            manifest_bytes,
            required_additional_bytes,
        };
        requirement.validate()?;
        Ok(requirement)
    }

    /// Reject contradictory or superseded copy evidence.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_COPY_SCRATCH_SCHEMA_V1
            || self.catalog_bytes == 0
            || self.manifest_bytes == 0
            || self.catalog_bytes.checked_add(self.manifest_bytes)
                != Some(self.required_additional_bytes)
        {
            return Err(crate::Error::ConfigError(
                "catalog copy scratch evidence contradicts its artifact byte facts".to_string(),
            ));
        }
        Ok(())
    }
}

/// Exact additional filesystem allocation required while SQLite compacts a
/// private catalog alongside its current database file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogFinalizationScratchV1 {
    /// Contract schema version.
    pub schema_version: u32,
    /// Positive SQLite page size read after committing catalog metadata.
    pub database_page_size: u64,
    /// Positive SQLite page count read from the same committed candidate.
    pub database_page_count: u64,
    /// Exact committed database bytes before compaction.
    pub database_bytes: u64,
    /// Conservative temporary-database allocation used by `VACUUM`.
    pub temporary_copy_bytes: u64,
    /// Conservative rollback-journal allocation while copying back.
    pub rollback_journal_bytes: u64,
    /// Sum of the two transient allocations required by `VACUUM`.
    pub required_additional_bytes: u64,
}

impl CatalogFinalizationScratchV1 {
    /// Derive SQLite's documented worst-case free-space requirement from
    /// positive page facts read after committing the private candidate.
    ///
    /// <https://www.sqlite.org/lang_vacuum.html#how_vacuum_works>
    pub fn from_page_facts(page_size: u64, page_count: u64) -> Result<Self> {
        if page_size == 0 || page_count == 0 {
            return Err(crate::Error::ConfigError(
                "catalog finalization requires positive SQLite page facts".to_string(),
            ));
        }
        let database_bytes = page_size.checked_mul(page_count).ok_or_else(|| {
            crate::Error::IoError(
                "catalog finalization scratch-space arithmetic overflow".to_string(),
            )
        })?;
        let required_additional_bytes = database_bytes.checked_mul(2).ok_or_else(|| {
            crate::Error::IoError(
                "catalog finalization scratch-space arithmetic overflow".to_string(),
            )
        })?;
        let requirement = Self {
            schema_version: CATALOG_FINALIZATION_SCRATCH_SCHEMA_V1,
            database_page_size: page_size,
            database_page_count: page_count,
            database_bytes,
            temporary_copy_bytes: database_bytes,
            rollback_journal_bytes: database_bytes,
            required_additional_bytes,
        };
        requirement.validate()?;
        Ok(requirement)
    }

    /// Reject contradictory or superseded scratch evidence.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_FINALIZATION_SCRATCH_SCHEMA_V1
            || self.database_page_size == 0
            || self.database_page_count == 0
        {
            return Err(crate::Error::ConfigError(
                "catalog finalization scratch evidence has invalid schema or page facts"
                    .to_string(),
            ));
        }
        let expected_database = self
            .database_page_size
            .checked_mul(self.database_page_count);
        let expected_required = self
            .temporary_copy_bytes
            .checked_add(self.rollback_journal_bytes);
        if expected_database != Some(self.database_bytes)
            || self.temporary_copy_bytes != self.database_bytes
            || self.rollback_journal_bytes != self.database_bytes
            || expected_required != Some(self.required_additional_bytes)
        {
            return Err(crate::Error::ConfigError(
                "catalog finalization scratch evidence contradicts its SQLite page facts"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// Stable capacity evidence returned before a large catalog allocation begins.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "catalog storage requires {required_bytes} additional bytes, but the filesystem has \
     {available_bytes} available with {reserved_bytes} bytes reserved by concurrent catalog work"
)]
pub struct CatalogScratchCapacityError {
    /// Additional bytes required by this catalog operation.
    pub required_bytes: u64,
    /// Bytes reported available on the owning filesystem at admission.
    pub available_bytes: u64,
    /// Bytes already promised to other catalog work in this process.
    pub reserved_bytes: u64,
}

/// Process owner that admits an exact catalog scratch requirement. A returned
/// lease retains its reservation until the owning operation completes or aborts.
pub trait CatalogScratchAdmission: Send + Sync {
    /// Reserve authenticated native metadata bytes at the existing run-local
    /// work directory until the sink removes those files.
    fn reserve_metadata(
        &self,
        work_directory: &Path,
        requirement: CatalogMetadataScratchV1,
    ) -> Result<Box<dyn Send>>;

    /// Reserve the exact additional bytes and retain them in the returned lease.
    fn reserve_finalization(
        &self,
        candidate_path: &Path,
        requirement: CatalogFinalizationScratchV1,
    ) -> Result<Box<dyn Send>>;

    /// Reserve an exact verified catalog and canonical manifest copy at the
    /// existing destination root, retaining it through durable reopen.
    fn reserve_copy(
        &self,
        destination_root: &Path,
        requirement: CatalogCopyScratchV1,
    ) -> Result<Box<dyn Send>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(
        role: SourceMetadataObjectRoleV1,
        source_path: &str,
        size: u64,
    ) -> CatalogMetadataObjectScratchV1 {
        CatalogMetadataObjectScratchV1 {
            role,
            source_path: source_path.to_string(),
            size,
        }
    }

    #[test]
    fn metadata_requirement_is_canonical_and_exact() {
        let requirement = CatalogMetadataScratchV1::from_signed_objects(vec![
            object(
                SourceMetadataObjectRoleV1::RpmFilelists,
                "repodata/filelists.xml.zst",
                3072,
            ),
            object(
                SourceMetadataObjectRoleV1::RpmPrimary,
                "repodata/primary.xml.zst",
                1024,
            ),
        ])
        .unwrap();

        assert_eq!(requirement.required_additional_bytes, 4096);
        assert_eq!(
            requirement.objects[0].role,
            SourceMetadataObjectRoleV1::RpmPrimary
        );
        requirement.validate().unwrap();
    }

    #[test]
    fn metadata_requirement_rejects_missing_repeated_and_contradictory_facts() {
        assert!(CatalogMetadataScratchV1::from_signed_objects(Vec::new()).is_err());
        assert!(
            CatalogMetadataScratchV1::from_signed_objects(vec![object(
                SourceMetadataObjectRoleV1::DebianPackages,
                "Packages.gz",
                0,
            )])
            .is_err()
        );
        assert!(
            CatalogMetadataScratchV1::from_signed_objects(vec![object(
                SourceMetadataObjectRoleV1::DebianPackages,
                "../Packages.gz",
                1,
            )])
            .is_err()
        );
        assert!(
            CatalogMetadataScratchV1::from_signed_objects(vec![
                object(SourceMetadataObjectRoleV1::RpmPrimary, "primary.xml.gz", 1,),
                object(
                    SourceMetadataObjectRoleV1::RpmPrimary,
                    "other-primary.xml.gz",
                    1,
                ),
            ])
            .is_err()
        );

        let mut contradictory = CatalogMetadataScratchV1::from_signed_objects(vec![object(
            SourceMetadataObjectRoleV1::DebianPackages,
            "Packages.gz",
            7,
        )])
        .unwrap();
        contradictory.required_additional_bytes += 1;
        assert!(contradictory.validate().is_err());
    }

    #[test]
    fn copy_requirement_is_exact_and_rejects_contradiction() {
        let requirement = CatalogCopyScratchV1::from_exact_bytes(4096, 257).unwrap();
        assert_eq!(requirement.required_additional_bytes, 4353);

        let mut contradictory = requirement;
        contradictory.required_additional_bytes += 1;
        assert!(contradictory.validate().is_err());
    }

    #[test]
    fn copy_requirement_rejects_zero_and_overflow() {
        assert!(CatalogCopyScratchV1::from_exact_bytes(0, 1).is_err());
        assert!(CatalogCopyScratchV1::from_exact_bytes(1, 0).is_err());
        assert!(CatalogCopyScratchV1::from_exact_bytes(u64::MAX, 1).is_err());
    }
}
