// conary-core/src/repository/catalog/capacity.rs

//! Typed scratch-space contracts for immutable catalog construction.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::Result;

use super::SourceMetadataObjectRoleV1;
use super::contract::validate_relative_source_path;

/// Current schema for exact SQLite catalog-finalization scratch evidence.
pub const CATALOG_FINALIZATION_SCRATCH_SCHEMA_V2: u32 = 2;

/// Current schema for exact immutable catalog-copy scratch evidence.
pub const CATALOG_COPY_SCRATCH_SCHEMA_V1: u32 = 1;

/// Current schema for exact authenticated-metadata staging evidence.
pub const CATALOG_METADATA_SCRATCH_SCHEMA_V1: u32 = 1;

/// Current schema for metadata whose exact size is admitted while streaming.
pub const CATALOG_METADATA_STREAM_SCRATCH_SCHEMA_V1: u32 = 1;

/// Current schema for a normalized parser-projection spool admitted while streaming.
pub const CATALOG_PROJECTION_SPOOL_SCRATCH_SCHEMA_V1: u32 = 1;

/// Current schema for profile-candidate construction admission.
pub const CATALOG_PROFILE_CANDIDATE_SCRATCH_SCHEMA_V1: u32 = 1;

/// Current schema for native source-candidate construction admission.
pub const CATALOG_SOURCE_CANDIDATE_SCRATCH_SCHEMA_V1: u32 = 1;

/// SQLite page size fixed by the immutable catalog schema.
pub const CATALOG_SQLITE_PAGE_SIZE_V1: u64 = 4096;

/// Fixed table and index roots created by catalog schema 1 before package rows.
pub const CATALOG_SQLITE_SCHEMA_PAGE_COUNT_V1: u64 = 16;

/// One private normalized-projection stream whose exact size is learned while parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProjectionSpoolScratchV1 {
    /// Contract schema version.
    pub schema_version: u32,
    /// Run-local relative path used only until the admitted candidate is populated.
    pub source_path: String,
}

impl CatalogProjectionSpoolScratchV1 {
    /// Bind the private spool path before any normalized projection bytes are staged.
    pub fn new(source_path: impl Into<String>) -> Result<Self> {
        let requirement = Self {
            schema_version: CATALOG_PROJECTION_SPOOL_SCRATCH_SCHEMA_V1,
            source_path: source_path.into(),
        };
        requirement.validate()?;
        Ok(requirement)
    }

    /// Reject superseded schemas and paths outside the private work directory.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_PROJECTION_SPOOL_SCRATCH_SCHEMA_V1 {
            return Err(crate::Error::ConfigError(
                "catalog projection spool scratch evidence has an invalid schema".to_string(),
            ));
        }
        validate_relative_source_path(&self.source_path)
    }
}

/// One authenticated metadata stream whose exact served size is learned only
/// as bytes arrive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogMetadataStreamScratchV1 {
    /// Contract schema version.
    pub schema_version: u32,
    /// Typed metadata role authenticated after the complete stream arrives.
    pub role: SourceMetadataObjectRoleV1,
    /// Repository-relative source path bound into the resulting evidence.
    pub source_path: String,
}

impl CatalogMetadataStreamScratchV1 {
    /// Bind one exact role and path before any response bytes are staged.
    pub fn new(role: SourceMetadataObjectRoleV1, source_path: impl Into<String>) -> Result<Self> {
        let requirement = Self {
            schema_version: CATALOG_METADATA_STREAM_SCRATCH_SCHEMA_V1,
            role,
            source_path: source_path.into(),
        };
        requirement.validate()?;
        Ok(requirement)
    }

    /// Keep streaming admission confined to the two metadata authorities that
    /// do not publish a signed byte length before download.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_METADATA_STREAM_SCRATCH_SCHEMA_V1
            || !matches!(
                self.role,
                SourceMetadataObjectRoleV1::ArchDatabase | SourceMetadataObjectRoleV1::EopkgIndex
            )
        {
            return Err(crate::Error::ConfigError(
                "catalog metadata stream scratch evidence has invalid schema or role".to_string(),
            ));
        }
        validate_relative_source_path(&self.source_path)
    }
}

/// Exact immutable source-catalog facts consumed by one profile member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProfileMemberScratchV1 {
    /// Canonical profile member ordinal.
    pub ordinal: u32,
    /// Exact independently verified source-catalog artifact bytes.
    pub catalog_bytes: u64,
    /// Exact package count bound by the source snapshot.
    pub package_count: u64,
}

/// Conservative pre-write allocation for one profile-catalog candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProfileCandidateScratchV1 {
    /// Contract schema version.
    pub schema_version: u32,
    /// Page size fixed by the immutable catalog schema.
    pub page_size: u64,
    /// Exact ordered source members consumed by the profile.
    pub members: Vec<CatalogProfileMemberScratchV1>,
    /// Sum of exact verified source-catalog artifact bytes.
    pub input_catalog_bytes: u64,
    /// Sum of exact input package counts before profile deduplication.
    pub input_package_count: u64,
    /// Baseline destination payload allocation supplied by the input catalogs.
    pub destination_payload_bytes: u64,
    /// Separate full source-byte budget for arbitrary destination B-tree repacking.
    pub btree_rewrite_bytes: u64,
    /// One page per source package for the expanded profile-origin row.
    pub profile_origin_bytes: u64,
    /// Complete database-file ceiling before final compaction.
    pub candidate_database_bytes: u64,
    /// Full source-byte ceiling for the new database's rollback journal.
    pub rollback_journal_bytes: u64,
    /// Sum of every candidate-construction allocation above.
    pub required_additional_bytes: u64,
}

impl CatalogProfileCandidateScratchV1 {
    /// Derive the complete profile construction bound from exact source artifacts.
    pub fn from_members(mut members: Vec<CatalogProfileMemberScratchV1>) -> Result<Self> {
        members.sort_by_key(|member| member.ordinal);
        let input_catalog_bytes = members.iter().try_fold(0_u64, |total, member| {
            total.checked_add(member.catalog_bytes).ok_or_else(|| {
                crate::Error::IoError(
                    "profile candidate scratch-space arithmetic overflow".to_string(),
                )
            })
        })?;
        let input_package_count = members.iter().try_fold(0_u64, |total, member| {
            total.checked_add(member.package_count).ok_or_else(|| {
                crate::Error::IoError(
                    "profile candidate package-count arithmetic overflow".to_string(),
                )
            })
        })?;
        let profile_origin_bytes = input_package_count
            .checked_mul(CATALOG_SQLITE_PAGE_SIZE_V1)
            .ok_or_else(|| {
                crate::Error::IoError(
                    "profile candidate scratch-space arithmetic overflow".to_string(),
                )
            })?;
        let candidate_database_bytes = input_catalog_bytes
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(profile_origin_bytes))
            .ok_or_else(|| {
                crate::Error::IoError(
                    "profile candidate scratch-space arithmetic overflow".to_string(),
                )
            })?;
        let required_additional_bytes = candidate_database_bytes
            .checked_add(input_catalog_bytes)
            .ok_or_else(|| {
            crate::Error::IoError("profile candidate scratch-space arithmetic overflow".to_string())
        })?;
        let requirement = Self {
            schema_version: CATALOG_PROFILE_CANDIDATE_SCRATCH_SCHEMA_V1,
            page_size: CATALOG_SQLITE_PAGE_SIZE_V1,
            members,
            input_catalog_bytes,
            input_package_count,
            destination_payload_bytes: input_catalog_bytes,
            btree_rewrite_bytes: input_catalog_bytes,
            profile_origin_bytes,
            candidate_database_bytes,
            rollback_journal_bytes: input_catalog_bytes,
            required_additional_bytes,
        };
        requirement.validate()?;
        Ok(requirement)
    }

    /// Reject incomplete, noncanonical, non-page-aligned, or contradictory facts.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_PROFILE_CANDIDATE_SCRATCH_SCHEMA_V1
            || self.page_size != CATALOG_SQLITE_PAGE_SIZE_V1
            || self.members.is_empty()
        {
            return Err(crate::Error::ConfigError(
                "profile candidate scratch evidence has invalid schema or no members".to_string(),
            ));
        }
        for (index, member) in self.members.iter().enumerate() {
            let expected_ordinal = u32::try_from(index).map_err(|_| {
                crate::Error::ConfigError(
                    "profile candidate scratch evidence has too many members".to_string(),
                )
            })?;
            if member.ordinal != expected_ordinal
                || member.catalog_bytes == 0
                || member.catalog_bytes % self.page_size != 0
            {
                return Err(crate::Error::ConfigError(
                    "profile candidate scratch member facts are noncanonical".to_string(),
                ));
            }
        }
        let canonical = Self::from_members_unchecked(self.members.clone())?;
        if canonical.input_catalog_bytes != self.input_catalog_bytes
            || canonical.input_package_count != self.input_package_count
            || canonical.destination_payload_bytes != self.destination_payload_bytes
            || canonical.btree_rewrite_bytes != self.btree_rewrite_bytes
            || canonical.profile_origin_bytes != self.profile_origin_bytes
            || canonical.candidate_database_bytes != self.candidate_database_bytes
            || canonical.rollback_journal_bytes != self.rollback_journal_bytes
            || canonical.required_additional_bytes != self.required_additional_bytes
        {
            return Err(crate::Error::ConfigError(
                "profile candidate scratch evidence contradicts its source catalog facts"
                    .to_string(),
            ));
        }
        Ok(())
    }

    fn from_members_unchecked(members: Vec<CatalogProfileMemberScratchV1>) -> Result<Self> {
        let input_catalog_bytes = members.iter().try_fold(0_u64, |total, member| {
            total.checked_add(member.catalog_bytes).ok_or_else(|| {
                crate::Error::IoError(
                    "profile candidate scratch-space arithmetic overflow".to_string(),
                )
            })
        })?;
        let input_package_count = members.iter().try_fold(0_u64, |total, member| {
            total.checked_add(member.package_count).ok_or_else(|| {
                crate::Error::IoError(
                    "profile candidate package-count arithmetic overflow".to_string(),
                )
            })
        })?;
        let profile_origin_bytes = input_package_count
            .checked_mul(CATALOG_SQLITE_PAGE_SIZE_V1)
            .ok_or_else(|| {
                crate::Error::IoError(
                    "profile candidate scratch-space arithmetic overflow".to_string(),
                )
            })?;
        let candidate_database_bytes = input_catalog_bytes
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(profile_origin_bytes))
            .ok_or_else(|| {
                crate::Error::IoError(
                    "profile candidate scratch-space arithmetic overflow".to_string(),
                )
            })?;
        let required_additional_bytes = candidate_database_bytes
            .checked_add(input_catalog_bytes)
            .ok_or_else(|| {
            crate::Error::IoError("profile candidate scratch-space arithmetic overflow".to_string())
        })?;
        Ok(Self {
            schema_version: CATALOG_PROFILE_CANDIDATE_SCRATCH_SCHEMA_V1,
            page_size: CATALOG_SQLITE_PAGE_SIZE_V1,
            members,
            input_catalog_bytes,
            input_package_count,
            destination_payload_bytes: input_catalog_bytes,
            btree_rewrite_bytes: input_catalog_bytes,
            profile_origin_bytes,
            candidate_database_bytes,
            rollback_journal_bytes: input_catalog_bytes,
            required_additional_bytes,
        })
    }
}

/// Conservative pre-write allocation for one native source-catalog candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSourceCandidateScratchV1 {
    /// Contract schema version.
    pub schema_version: u32,
    /// Page size fixed by the immutable catalog schema.
    pub page_size: u64,
    /// Exact canonical bytes of normalized package and supplemental relation facts.
    pub canonical_projection_bytes: u64,
    /// Exact package count observed by the authenticated preflight.
    pub package_count: u64,
    /// Baseline destination payload allocation supplied by the projection.
    pub destination_payload_bytes: u64,
    /// Separate full projection-byte budget for arbitrary B-tree repacking.
    pub btree_rewrite_bytes: u64,
    /// Fixed schema roots plus one page per source package for row structure.
    pub package_structure_bytes: u64,
    /// Complete database-file ceiling before final compaction.
    pub candidate_database_bytes: u64,
    /// Full candidate-database ceiling for the rollback journal.
    pub rollback_journal_bytes: u64,
    /// Sum of candidate and journal allocations.
    pub required_additional_bytes: u64,
}

impl CatalogSourceCandidateScratchV1 {
    /// Derive a complete native-candidate bound from exact preflight facts.
    pub fn from_projection_facts(
        canonical_projection_bytes: u64,
        package_count: u64,
    ) -> Result<Self> {
        if canonical_projection_bytes == 0 {
            return Err(crate::Error::ConfigError(
                "source candidate scratch admission requires positive projection bytes".to_string(),
            ));
        }
        let package_structure_bytes = package_count
            .checked_add(CATALOG_SQLITE_SCHEMA_PAGE_COUNT_V1)
            .and_then(|pages| pages.checked_mul(CATALOG_SQLITE_PAGE_SIZE_V1))
            .ok_or_else(source_candidate_overflow)?;
        let candidate_database_bytes = canonical_projection_bytes
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(package_structure_bytes))
            .ok_or_else(source_candidate_overflow)?;
        let required_additional_bytes = candidate_database_bytes
            .checked_mul(2)
            .ok_or_else(source_candidate_overflow)?;
        let requirement = Self {
            schema_version: CATALOG_SOURCE_CANDIDATE_SCRATCH_SCHEMA_V1,
            page_size: CATALOG_SQLITE_PAGE_SIZE_V1,
            canonical_projection_bytes,
            package_count,
            destination_payload_bytes: canonical_projection_bytes,
            btree_rewrite_bytes: canonical_projection_bytes,
            package_structure_bytes,
            candidate_database_bytes,
            rollback_journal_bytes: candidate_database_bytes,
            required_additional_bytes,
        };
        requirement.validate()?;
        Ok(requirement)
    }

    /// Reuse one independently reopened source catalog as exact materialization evidence.
    pub fn from_cached_catalog(catalog_bytes: u64, package_count: u64) -> Result<Self> {
        if catalog_bytes == 0 || !catalog_bytes.is_multiple_of(CATALOG_SQLITE_PAGE_SIZE_V1) {
            return Err(crate::Error::ConfigError(
                "cached source candidate scratch admission requires positive page-aligned catalog bytes"
                    .to_string(),
            ));
        }
        Self::from_projection_facts(catalog_bytes, package_count)
    }

    /// Reject contradictory or superseded source-candidate evidence.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_SOURCE_CANDIDATE_SCRATCH_SCHEMA_V1
            || self.page_size != CATALOG_SQLITE_PAGE_SIZE_V1
            || self.canonical_projection_bytes == 0
        {
            return Err(crate::Error::ConfigError(
                "source candidate scratch evidence has invalid schema or facts".to_string(),
            ));
        }
        let canonical = Self::from_projection_facts_unchecked(
            self.canonical_projection_bytes,
            self.package_count,
        )?;
        if canonical.destination_payload_bytes != self.destination_payload_bytes
            || canonical.btree_rewrite_bytes != self.btree_rewrite_bytes
            || canonical.package_structure_bytes != self.package_structure_bytes
            || canonical.candidate_database_bytes != self.candidate_database_bytes
            || canonical.rollback_journal_bytes != self.rollback_journal_bytes
            || canonical.required_additional_bytes != self.required_additional_bytes
        {
            return Err(crate::Error::ConfigError(
                "source candidate scratch evidence contradicts its projection facts".to_string(),
            ));
        }
        Ok(())
    }

    fn from_projection_facts_unchecked(
        canonical_projection_bytes: u64,
        package_count: u64,
    ) -> Result<Self> {
        let package_structure_bytes = package_count
            .checked_add(CATALOG_SQLITE_SCHEMA_PAGE_COUNT_V1)
            .and_then(|pages| pages.checked_mul(CATALOG_SQLITE_PAGE_SIZE_V1))
            .ok_or_else(source_candidate_overflow)?;
        let candidate_database_bytes = canonical_projection_bytes
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(package_structure_bytes))
            .ok_or_else(source_candidate_overflow)?;
        let required_additional_bytes = candidate_database_bytes
            .checked_mul(2)
            .ok_or_else(source_candidate_overflow)?;
        Ok(Self {
            schema_version: CATALOG_SOURCE_CANDIDATE_SCRATCH_SCHEMA_V1,
            page_size: CATALOG_SQLITE_PAGE_SIZE_V1,
            canonical_projection_bytes,
            package_count,
            destination_payload_bytes: canonical_projection_bytes,
            btree_rewrite_bytes: canonical_projection_bytes,
            package_structure_bytes,
            candidate_database_bytes,
            rollback_journal_bytes: candidate_database_bytes,
            required_additional_bytes,
        })
    }
}

fn source_candidate_overflow() -> crate::Error {
    crate::Error::IoError("source candidate scratch-space arithmetic overflow".to_string())
}

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
pub struct CatalogFinalizationScratchV2 {
    /// Contract schema version.
    pub schema_version: u32,
    /// Positive SQLite page size read after committing catalog metadata.
    pub database_page_size: u64,
    /// Positive SQLite page count read from the same committed candidate.
    pub database_page_count: u64,
    /// Exact committed database bytes before compaction.
    pub database_bytes: u64,
    /// Conservative compacted-output allocation used by `VACUUM INTO`.
    pub compacted_copy_bytes: u64,
    /// Exact transient allocation required by `VACUUM INTO`.
    pub required_additional_bytes: u64,
}

impl CatalogFinalizationScratchV2 {
    /// Derive the compacted-output free-space requirement from positive page
    /// facts read after committing the private candidate.
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
        let requirement = Self {
            schema_version: CATALOG_FINALIZATION_SCRATCH_SCHEMA_V2,
            database_page_size: page_size,
            database_page_count: page_count,
            database_bytes,
            compacted_copy_bytes: database_bytes,
            required_additional_bytes: database_bytes,
        };
        requirement.validate()?;
        Ok(requirement)
    }

    /// Reject contradictory or superseded scratch evidence.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_FINALIZATION_SCRATCH_SCHEMA_V2
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
        if expected_database != Some(self.database_bytes)
            || self.compacted_copy_bytes != self.database_bytes
            || self.required_additional_bytes != self.database_bytes
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

/// Filesystem admission for one metadata stream. Each returned permit covers
/// one exact response chunk until that chunk has been written to run-local
/// storage and the filesystem reports the materialized allocation itself.
pub trait CatalogMetadataStreamAdmission: Send + Sync {
    /// Reserve the next positive response-chunk allocation before its write.
    fn reserve_next(&self, additional_bytes: u64) -> Result<Box<dyn Send>>;
}

/// Process owner that admits an exact catalog scratch requirement. A returned
/// lease retains its reservation until the owning operation completes or aborts.
pub trait CatalogScratchAdmission: Send + Sync {
    /// Reserve complete native source-candidate growth before its private file exists.
    fn reserve_source_candidate(
        &self,
        candidate_path: &Path,
        requirement: CatalogSourceCandidateScratchV1,
    ) -> Result<Box<dyn Send>>;

    /// Reserve complete profile-candidate growth before its private file exists.
    fn reserve_profile_candidate(
        &self,
        candidate_path: &Path,
        requirement: CatalogProfileCandidateScratchV1,
    ) -> Result<Box<dyn Send>>;

    /// Reserve authenticated native metadata bytes at the existing run-local
    /// work directory until the sink removes those files.
    fn reserve_metadata(
        &self,
        work_directory: &Path,
        requirement: CatalogMetadataScratchV1,
    ) -> Result<Box<dyn Send>>;

    /// Create a chunk admission owner for metadata without a signed length.
    fn stream_metadata(
        &self,
        work_directory: &Path,
        requirement: CatalogMetadataStreamScratchV1,
    ) -> Result<Box<dyn CatalogMetadataStreamAdmission>>;

    /// Admit private normalized projection chunks before each spool write.
    fn stream_projection_spool(
        &self,
        work_directory: &Path,
        requirement: CatalogProjectionSpoolScratchV1,
    ) -> Result<Box<dyn CatalogMetadataStreamAdmission>>;

    /// Reserve the exact additional bytes and retain them in the returned lease.
    fn reserve_finalization(
        &self,
        candidate_path: &Path,
        requirement: CatalogFinalizationScratchV2,
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

    fn profile_member(
        ordinal: u32,
        catalog_pages: u64,
        package_count: u64,
    ) -> CatalogProfileMemberScratchV1 {
        CatalogProfileMemberScratchV1 {
            ordinal,
            catalog_bytes: catalog_pages * CATALOG_SQLITE_PAGE_SIZE_V1,
            package_count,
        }
    }

    #[test]
    fn finalization_requirement_admits_one_direct_compaction_output() {
        let requirement = CatalogFinalizationScratchV2::from_page_facts(4096, 23).unwrap();

        assert_eq!(
            requirement.schema_version,
            CATALOG_FINALIZATION_SCRATCH_SCHEMA_V2
        );
        assert_eq!(requirement.database_bytes, 23 * 4096);
        assert_eq!(requirement.compacted_copy_bytes, 23 * 4096);
        assert_eq!(requirement.required_additional_bytes, 23 * 4096);

        let mut contradictory = requirement;
        contradictory.compacted_copy_bytes += 1;
        assert!(contradictory.validate().is_err());
    }

    #[test]
    fn profile_candidate_requirement_is_structural_canonical_and_exact() {
        let requirement = CatalogProfileCandidateScratchV1::from_members(vec![
            profile_member(1, 2, 3),
            profile_member(0, 1, 2),
        ])
        .unwrap();

        assert_eq!(requirement.members[0].ordinal, 0);
        assert_eq!(requirement.input_catalog_bytes, 3 * 4096);
        assert_eq!(requirement.input_package_count, 5);
        assert_eq!(requirement.destination_payload_bytes, 3 * 4096);
        assert_eq!(requirement.btree_rewrite_bytes, 3 * 4096);
        assert_eq!(requirement.profile_origin_bytes, 5 * 4096);
        assert_eq!(requirement.candidate_database_bytes, 11 * 4096);
        assert_eq!(requirement.rollback_journal_bytes, 3 * 4096);
        assert_eq!(requirement.required_additional_bytes, 14 * 4096);

        let mut contradictory = requirement.clone();
        contradictory.profile_origin_bytes += 1;
        assert!(contradictory.validate().is_err());
        assert!(
            CatalogProfileCandidateScratchV1::from_members(vec![profile_member(1, 1, 1)]).is_err()
        );
        assert!(
            CatalogProfileCandidateScratchV1::from_members(vec![CatalogProfileMemberScratchV1 {
                ordinal: 0,
                catalog_bytes: 4095,
                package_count: 1,
            },])
            .is_err()
        );
    }

    #[test]
    fn source_candidate_requirement_is_structural_and_rejects_contradiction() {
        let requirement = CatalogSourceCandidateScratchV1::from_projection_facts(8192, 3).unwrap();

        assert_eq!(requirement.destination_payload_bytes, 8192);
        assert_eq!(requirement.btree_rewrite_bytes, 8192);
        assert_eq!(requirement.package_structure_bytes, 19 * 4096);
        assert_eq!(requirement.candidate_database_bytes, 23 * 4096);
        assert_eq!(requirement.rollback_journal_bytes, 23 * 4096);
        assert_eq!(requirement.required_additional_bytes, 46 * 4096);

        let cached = CatalogSourceCandidateScratchV1::from_cached_catalog(4096, 0).unwrap();
        cached.validate().unwrap();
        assert!(CatalogSourceCandidateScratchV1::from_cached_catalog(4095, 1).is_err());
        assert!(CatalogSourceCandidateScratchV1::from_projection_facts(0, 1).is_err());

        let mut contradictory = requirement;
        contradictory.candidate_database_bytes += 1;
        assert!(contradictory.validate().is_err());
    }

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
    fn stream_requirement_accepts_only_unknown_length_metadata_roles() {
        let arch = CatalogMetadataStreamScratchV1::new(
            SourceMetadataObjectRoleV1::ArchDatabase,
            "core.db",
        )
        .unwrap();
        arch.validate().unwrap();
        CatalogMetadataStreamScratchV1::new(
            SourceMetadataObjectRoleV1::EopkgIndex,
            "eopkg-index.xml.xz",
        )
        .unwrap();

        assert!(
            CatalogMetadataStreamScratchV1::new(
                SourceMetadataObjectRoleV1::RpmPrimary,
                "repodata/primary.xml.zst",
            )
            .is_err()
        );
        assert!(
            CatalogMetadataStreamScratchV1::new(
                SourceMetadataObjectRoleV1::ArchDatabase,
                "../core.db",
            )
            .is_err()
        );
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
