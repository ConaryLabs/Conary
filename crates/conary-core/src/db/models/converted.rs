// conary-core/src/db/models/converted.rs

//! Converted package tracking model
//!
//! Tracks packages converted from native formats (RPM/DEB/Arch) to CCS.
//! This enables:
//! - Skip re-conversion of same package artifact (checksum-based dedup)
//! - Persist typed lifecycle evidence
//! - Re-convert when conversion algorithm is upgraded

use crate::ccs::convert::ScriptletBundleSummary;
use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::BTreeSet;
use strum_macros::{AsRefStr, Display, EnumString};

/// Current conversion algorithm version
/// Bump this when making changes that require re-conversion of existing packages.
///
/// Revision 12 binds repository conversions to the exact normalized repository
/// capability projection that influenced their signed CCS authority.
pub const CONVERSION_VERSION: i32 = 12;
/// Canonical digest of an empty repository-provide projection.
pub const EMPTY_REPOSITORY_PROVIDES_DIGEST: &str =
    "sha256:4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945";

/// Storage and ownership boundary for a converted package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum ConvertedArtifactKind {
    /// A conversion attached to an installed Conary trove.
    Installed,
    /// A Remi repository artifact with complete serving identity and storage.
    Repository,
}

/// Validated repository-serving view of a converted package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryConvertedArtifact<'a> {
    pub package_name: &'a str,
    pub package_version: &'a str,
    pub source_profile: &'a str,
    pub package_architecture: &'a str,
    pub repository_provides_digest: &'a str,
    pub chunk_hashes: Vec<String>,
    pub total_size: u64,
    pub content_hash: &'a str,
    pub ccs_path: &'a str,
}

/// A converted package record
#[derive(Debug, Clone)]
pub struct ConvertedPackage {
    pub id: Option<i64>,
    pub artifact_kind: ConvertedArtifactKind,
    /// Reference to the converted trove (CCS package that was installed)
    pub trove_id: Option<i64>,
    /// Original package format (rpm, deb, arch)
    pub original_format: String,
    /// Checksum of original package file (skip if already converted)
    pub original_checksum: String,
    /// Digest of the exact repository-provide projection merged into a
    /// repository conversion. Installed conversions carry no repository view.
    pub repository_provides_digest: Option<String>,
    /// Conversion algorithm version (re-convert if upgraded)
    pub conversion_version: i32,
    /// When the conversion occurred
    pub converted_at: Option<String>,

    /// Enhancement algorithm version (0 = not enhanced yet)
    pub enhancement_version: i32,
    /// Extracted provenance JSON (before DB insertion)
    pub extracted_provenance_json: Option<String>,
    /// Enhancement status: pending, in_progress, complete, failed, skipped
    pub enhancement_status: String,
    /// Error message if enhancement failed
    pub enhancement_error: Option<String>,
    /// When enhancement was last attempted
    pub enhancement_attempted_at: Option<String>,

    // Server-side conversion identity and storage fields.
    /// Package name (for server-side lookups)
    pub package_name: Option<String>,
    /// Package version (for server-side lookups)
    pub package_version: Option<String>,
    /// Exact public source profile for this repository conversion.
    pub source_profile: Option<String>,
    /// Native package architecture for server-side conversion cache identity.
    pub package_architecture: Option<String>,
    /// JSON array of chunk hashes
    pub chunk_hashes_json: Option<String>,
    /// Total size of the CCS package
    pub total_size: Option<i64>,
    /// Content hash of the CCS package
    pub content_hash: Option<String>,
    /// Path to the CCS package file
    pub ccs_path: Option<String>,

    // Foreign-package scriptlet evidence fields.
    /// Aggregate scriptlet fidelity from passive bundle construction.
    pub scriptlet_fidelity: String,
    /// Digest of normalized scriptlet evidence.
    pub evidence_digest: Option<String>,
    /// JSON-encoded typed scriptlet summary for API/index projection.
    pub scriptlet_summary_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkConversionState {
    NoConvertedReference,
    CurrentConversion,
    StaleConversionOnly,
}

impl ConvertedPackage {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, artifact_kind, trove_id, original_format, original_checksum, \
         repository_provides_digest, conversion_version, converted_at, \
         enhancement_version, extracted_provenance_json, \
         enhancement_status, enhancement_error, enhancement_attempted_at, \
         package_name, package_version, source_profile, chunk_hashes_json, total_size, \
         content_hash, ccs_path, package_architecture, scriptlet_fidelity, \
         evidence_digest, scriptlet_summary_json";

    /// Create a conversion attached to an installed trove.
    pub fn new_installed(
        trove_id: i64,
        original_format: String,
        original_checksum: String,
    ) -> Self {
        Self {
            id: None,
            artifact_kind: ConvertedArtifactKind::Installed,
            trove_id: Some(trove_id),
            original_format,
            original_checksum,
            repository_provides_digest: None,
            conversion_version: CONVERSION_VERSION,
            converted_at: None,
            // Enhancement starts as pending with version 0
            enhancement_version: 0,
            extracted_provenance_json: None,
            enhancement_status: "pending".to_string(),
            enhancement_error: None,
            enhancement_attempted_at: None,
            // Server-side fields start as None
            package_name: None,
            package_version: None,
            source_profile: None,
            package_architecture: None,
            chunk_hashes_json: None,
            total_size: None,
            content_hash: None,
            ccs_path: None,
            scriptlet_fidelity: "native-free".to_string(),
            evidence_digest: None,
            scriptlet_summary_json: default_scriptlet_summary_json(),
        }
    }

    /// Create a repository-serving converted package record.
    #[allow(clippy::too_many_arguments)]
    pub fn new_repository(
        source_profile: String,
        package_name: String,
        package_version: String,
        package_architecture: String,
        original_format: String,
        original_checksum: String,
        chunk_hashes: &[String],
        total_size: i64,
        content_hash: String,
        ccs_path: String,
        repository_provides_digest: String,
    ) -> Self {
        let chunk_hashes_json =
            serde_json::to_string(chunk_hashes).expect("a string list always serializes to JSON");
        Self {
            id: None,
            artifact_kind: ConvertedArtifactKind::Repository,
            trove_id: None,
            original_format,
            original_checksum,
            repository_provides_digest: Some(repository_provides_digest),
            conversion_version: CONVERSION_VERSION,
            converted_at: None,
            enhancement_version: 0,
            extracted_provenance_json: None,
            enhancement_status: "pending".to_string(),
            enhancement_error: None,
            enhancement_attempted_at: None,
            package_name: Some(package_name),
            package_version: Some(package_version),
            source_profile: Some(source_profile),
            package_architecture: Some(package_architecture),
            chunk_hashes_json: Some(chunk_hashes_json),
            total_size: Some(total_size),
            content_hash: Some(content_hash),
            ccs_path: Some(ccs_path),
            scriptlet_fidelity: "native-free".to_string(),
            evidence_digest: None,
            scriptlet_summary_json: default_scriptlet_summary_json(),
        }
    }

    /// Create from a database row
    ///
    /// The current schema epoch guarantees all columns exist.
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let artifact_kind_raw: String = row.get(1)?;
        let artifact_kind =
            artifact_kind_raw
                .parse::<ConvertedArtifactKind>()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
                    )
                })?;

        Ok(Self {
            id: row.get(0)?,
            artifact_kind,
            trove_id: row.get(2)?,
            original_format: row.get(3)?,
            original_checksum: row.get(4)?,
            repository_provides_digest: row.get(5)?,
            conversion_version: row.get(6)?,
            converted_at: row.get(7)?,
            enhancement_version: row.get(8)?,
            extracted_provenance_json: row.get(9)?,
            enhancement_status: row.get(10)?,
            enhancement_error: row.get(11)?,
            enhancement_attempted_at: row.get(12)?,
            package_name: row.get(13)?,
            package_version: row.get(14)?,
            source_profile: row.get(15)?,
            chunk_hashes_json: row.get(16)?,
            total_size: row.get(17)?,
            content_hash: row.get(18)?,
            ccs_path: row.get(19)?,
            package_architecture: row.get(20)?,
            scriptlet_fidelity: row.get(21)?,
            evidence_digest: row.get(22)?,
            scriptlet_summary_json: row.get(23)?,
        })
    }

    /// Insert this converted package into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        self.validate_artifact_contract()?;
        self.scriptlet_summary()?;
        conn.execute(
            "INSERT INTO converted_packages (artifact_kind, trove_id, original_format, original_checksum, repository_provides_digest, conversion_version,
                enhancement_version, extracted_provenance_json, enhancement_status,
                package_name, package_version, source_profile, chunk_hashes_json, total_size, content_hash, ccs_path, package_architecture,
                scriptlet_fidelity, evidence_digest,
                scriptlet_summary_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            params![
                self.artifact_kind.as_ref(),
                self.trove_id,
                &self.original_format,
                &self.original_checksum,
                &self.repository_provides_digest,
                self.conversion_version,
                self.enhancement_version,
                &self.extracted_provenance_json,
                &self.enhancement_status,
                &self.package_name,
                &self.package_version,
                &self.source_profile,
                &self.chunk_hashes_json,
                self.total_size,
                &self.content_hash,
                &self.ccs_path,
                &self.package_architecture,
                &self.scriptlet_fidelity,
                &self.evidence_digest,
                &self.scriptlet_summary_json,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find an installed conversion by its original checksum.
    pub fn find_installed_by_checksum(conn: &Connection, checksum: &str) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM converted_packages
             WHERE artifact_kind = 'installed' AND original_checksum = ?1",
            Self::COLUMNS
        );
        let result = conn
            .query_row(&sql, [checksum], Self::from_row)
            .optional()?;
        Ok(result)
    }

    /// Find a repository conversion by exact profile and original checksum.
    pub fn find_repository_by_checksum(
        conn: &Connection,
        source_profile: &str,
        checksum: &str,
    ) -> Result<Option<Self>> {
        Self::require_public_source_profile(source_profile)?;
        let sql = format!(
            "SELECT {} FROM converted_packages
             WHERE artifact_kind = 'repository'
               AND source_profile = ?1 AND original_checksum = ?2",
            Self::COLUMNS
        );
        conn.query_row(&sql, params![source_profile, checksum], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    /// Find a converted package by trove_id
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM converted_packages WHERE trove_id = ?1",
            Self::COLUMNS
        );
        let result = conn
            .query_row(&sql, [trove_id], Self::from_row)
            .optional()?;
        Ok(result)
    }

    /// Check if a package needs re-conversion (algorithm upgraded)
    pub fn needs_reconversion(&self) -> bool {
        self.conversion_version != CONVERSION_VERSION
    }

    /// Check whether this repository artifact still matches the exact current
    /// source package and normalized provide projection.
    pub fn repository_metadata_is_current(&self, conn: &Connection) -> Result<bool> {
        if self.needs_reconversion() {
            return Ok(false);
        }
        let artifact = self.repository_artifact()?;
        let current_inputs = super::RepositoryProvide::conversion_inputs_for_source_identity(
            conn,
            artifact.source_profile,
            artifact.package_name,
            artifact.package_version,
            artifact.package_architecture,
        )?
        .into_iter()
        .map(|(checksum, digest)| (bare_sha256(&checksum).to_string(), digest))
        .collect::<BTreeSet<_>>();
        if current_inputs.len() > 1 {
            return Ok(false);
        }

        Ok(current_inputs.contains(&(
            bare_sha256(&self.original_checksum).to_string(),
            artifact.repository_provides_digest.to_string(),
        )))
    }

    /// Remove repository conversion rows invalidated by one exact source
    /// profile refresh. The caller owns the surrounding repository-sync
    /// transaction.
    pub fn reconcile_repository_metadata(conn: &Connection, source_profile: &str) -> Result<usize> {
        Self::require_public_source_profile(source_profile)?;
        let mut removed = 0usize;
        for converted in Self::list_repository_conversions(conn)? {
            let artifact = converted.repository_artifact()?;
            if artifact.source_profile != source_profile {
                continue;
            }
            if converted.repository_metadata_is_current(conn)? {
                continue;
            }
            let id = converted.id.ok_or_else(|| {
                crate::Error::InternalError(
                    "persisted repository conversion has no row identity".to_string(),
                )
            })?;
            removed += conn.execute(
                "DELETE FROM converted_packages
                 WHERE artifact_kind = 'repository' AND id = ?1",
                [id],
            )?;
        }
        Ok(removed)
    }

    /// Return the exact installed trove identity for an installed conversion.
    pub fn installed_trove_id(&self) -> Result<i64> {
        if self.artifact_kind != ConvertedArtifactKind::Installed {
            return Err(crate::Error::InternalError(format!(
                "converted package {} is a repository artifact, not an installed conversion",
                self.record_identity()
            )));
        }
        self.trove_id.ok_or_else(|| {
            crate::Error::InternalError(format!(
                "installed converted package {} has no trove identity",
                self.record_identity()
            ))
        })
    }

    /// Return a validated repository-serving artifact.
    ///
    /// Missing identity or storage fields and malformed chunk JSON are
    /// persisted-state corruption. Serving callers must propagate this error;
    /// they must not synthesize names, versions, sizes, hashes, or paths.
    pub fn repository_artifact(&self) -> Result<RepositoryConvertedArtifact<'_>> {
        if self.artifact_kind != ConvertedArtifactKind::Repository {
            return Err(crate::Error::InternalError(format!(
                "converted package {} is an installed conversion, not a repository artifact",
                self.record_identity()
            )));
        }

        let package_name = self.required_repository_text("package_name", &self.package_name)?;
        let package_version =
            self.required_repository_text("package_version", &self.package_version)?;
        let source_profile =
            self.required_repository_text("source_profile", &self.source_profile)?;
        if crate::repository::supported_profiles::profile_by_public_id(source_profile).is_none() {
            return Err(crate::Error::InternalError(format!(
                "repository converted package {} carries unsupported source profile '{source_profile}'",
                self.record_identity()
            )));
        }
        let package_architecture =
            self.required_repository_text("package_architecture", &self.package_architecture)?;
        let repository_provides_digest = self.required_repository_text(
            "repository_provides_digest",
            &self.repository_provides_digest,
        )?;
        let repository_provides_digest = crate::hash::Hash::parse_prefixed(
            repository_provides_digest,
        )
        .map_err(|error| {
            crate::Error::InternalError(format!(
                "repository converted package {} has invalid repository_provides_digest: {error}",
                self.record_identity()
            ))
        })?;
        if repository_provides_digest.algorithm != crate::hash::HashAlgorithm::Sha256 {
            return Err(crate::Error::InternalError(format!(
                "repository converted package {} repository_provides_digest must use sha256",
                self.record_identity()
            )));
        }
        let content_hash = self.required_repository_text("content_hash", &self.content_hash)?;
        let ccs_path = self.required_repository_text("ccs_path", &self.ccs_path)?;
        let chunk_hashes_json =
            self.required_repository_text("chunk_hashes_json", &self.chunk_hashes_json)?;
        let chunk_hashes =
            serde_json::from_str::<Vec<String>>(chunk_hashes_json).map_err(|error| {
                crate::Error::InternalError(format!(
                    "repository converted package {} has malformed chunk_hashes_json: {error}",
                    self.record_identity()
                ))
            })?;
        if chunk_hashes.iter().any(|hash| hash.is_empty()) {
            return Err(crate::Error::InternalError(format!(
                "repository converted package {} has an empty chunk identity",
                self.record_identity()
            )));
        }
        let total_size = self.total_size.ok_or_else(|| {
            crate::Error::InternalError(format!(
                "repository converted package {} is missing total_size",
                self.record_identity()
            ))
        })?;
        let total_size = u64::try_from(total_size).map_err(|_| {
            crate::Error::InternalError(format!(
                "repository converted package {} has negative total_size {total_size}",
                self.record_identity()
            ))
        })?;

        Ok(RepositoryConvertedArtifact {
            package_name,
            package_version,
            source_profile,
            package_architecture,
            repository_provides_digest: self
                .repository_provides_digest
                .as_deref()
                .expect("validated repository digest is present"),
            chunk_hashes,
            total_size,
            content_hash,
            ccs_path,
        })
    }

    fn required_repository_text<'a>(
        &self,
        field: &str,
        value: &'a Option<String>,
    ) -> Result<&'a str> {
        value
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                crate::Error::InternalError(format!(
                    "repository converted package {} is missing {field}",
                    self.record_identity()
                ))
            })
    }

    fn validate_artifact_contract(&self) -> Result<()> {
        match self.artifact_kind {
            ConvertedArtifactKind::Installed => {
                self.installed_trove_id()?;
                if self.package_name.is_some()
                    || self.package_version.is_some()
                    || self.source_profile.is_some()
                    || self.package_architecture.is_some()
                    || self.repository_provides_digest.is_some()
                    || self.chunk_hashes_json.is_some()
                    || self.total_size.is_some()
                    || self.content_hash.is_some()
                    || self.ccs_path.is_some()
                {
                    return Err(crate::Error::InternalError(format!(
                        "installed converted package {} carries repository-serving fields",
                        self.record_identity()
                    )));
                }
                Ok(())
            }
            ConvertedArtifactKind::Repository => {
                if self.trove_id.is_some() {
                    return Err(crate::Error::InternalError(format!(
                        "repository converted package {} carries an installed trove identity",
                        self.record_identity()
                    )));
                }
                self.repository_artifact().map(|_| ())
            }
        }
    }

    /// Store passive scriptlet metadata generated during conversion.
    pub fn set_scriptlet_metadata(&mut self, summary: &ScriptletBundleSummary) -> Result<()> {
        validate_current_scriptlet_summary(summary)?;
        self.scriptlet_fidelity = summary.scriptlet_fidelity.clone();
        self.evidence_digest = summary.evidence_digest.clone();
        self.scriptlet_summary_json = serde_json::to_string(summary)?;
        Ok(())
    }

    /// Parse and validate the persisted current-epoch lifecycle summary.
    ///
    /// Malformed or internally inconsistent rows are database corruption, not
    /// a publication state and not an operator-review workflow.
    pub fn scriptlet_summary(&self) -> Result<ScriptletBundleSummary> {
        let summary = serde_json::from_str::<ScriptletBundleSummary>(&self.scriptlet_summary_json)
            .map_err(|error| {
                crate::Error::InternalError(format!(
                    "converted package {} has malformed lifecycle summary JSON: {error}",
                    self.record_identity()
                ))
            })?;
        validate_current_scriptlet_summary(&summary)?;
        self.validate_summary_projection(&summary)?;
        Ok(summary)
    }

    pub fn parsed_chunk_hashes(&self) -> Result<Vec<String>> {
        self.repository_artifact()
            .map(|artifact| artifact.chunk_hashes)
    }

    /// Classify whether a CAS chunk is reachable from a current conversion,
    /// stale conversions only, or no conversions.
    pub fn chunk_conversion_state(conn: &Connection, hash: &str) -> Result<ChunkConversionState> {
        let bare_hash = hash.strip_prefix("sha256:").unwrap_or(hash);
        let prefixed_hash = format!("sha256:{bare_hash}");
        let bare_pattern = format!("%\"{bare_hash}\"%");
        let prefixed_pattern = format!("%\"{prefixed_hash}\"%");

        let sql = format!(
            "SELECT {} FROM converted_packages
             WHERE artifact_kind = 'repository'
               AND chunk_hashes_json IS NOT NULL
               AND (chunk_hashes_json LIKE ?1 OR chunk_hashes_json LIKE ?2)",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![bare_pattern, prefixed_pattern], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);

        let mut saw_converted_reference = false;
        for candidate in rows {
            let references_hash = candidate
                .parsed_chunk_hashes()?
                .into_iter()
                .any(|chunk_hash| chunk_hash == bare_hash || chunk_hash == prefixed_hash);
            if !references_hash {
                continue;
            }

            saw_converted_reference = true;
            if candidate.repository_metadata_is_current(conn)? {
                candidate.scriptlet_summary()?;
                return Ok(ChunkConversionState::CurrentConversion);
            }
        }

        Ok(if saw_converted_reference {
            ChunkConversionState::StaleConversionOnly
        } else {
            ChunkConversionState::NoConvertedReference
        })
    }

    fn validate_summary_projection(&self, summary: &ScriptletBundleSummary) -> Result<()> {
        let projections_match = self.scriptlet_fidelity == summary.scriptlet_fidelity
            && self.evidence_digest == summary.evidence_digest;
        if projections_match {
            return Ok(());
        }

        Err(crate::Error::InternalError(format!(
            "converted package {} lifecycle summary disagrees with indexed projection columns",
            self.record_identity()
        )))
    }

    fn record_identity(&self) -> String {
        self.id
            .map(|id| id.to_string())
            .unwrap_or_else(|| self.original_checksum.clone())
    }

    /// List all converted packages
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM converted_packages ORDER BY converted_at DESC",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let results = stmt
            .query_map([], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
    }

    /// List repository-serving conversions only.
    pub fn list_repository_conversions(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM converted_packages
             WHERE artifact_kind = 'repository'
             ORDER BY converted_at DESC",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let results = stmt
            .query_map([], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
    }

    /// List converted packages after a checkpoint id, bounded in SQL.
    pub fn list_after_id(conn: &Connection, after_id: i64, limit: usize) -> Result<Vec<Self>> {
        let sql_limit = i64::try_from(limit).unwrap_or(i64::MAX).max(1);
        let sql = format!(
            "SELECT {} FROM converted_packages
             WHERE id > ?1
             ORDER BY id ASC
             LIMIT ?2",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let results = stmt
            .query_map(params![after_id, sql_limit], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
    }

    /// Find converted packages whose algorithm, exact source package, and
    /// normalized repository-provide projection are all current.
    ///
    /// Callers that expose lifecycle metadata must parse `scriptlet_summary()`
    /// so malformed current rows surface as data corruption.
    pub fn find_current_conversions(
        conn: &Connection,
        source_profile: &str,
        package_name: Option<&str>,
    ) -> Result<Vec<Self>> {
        Self::require_public_source_profile(source_profile)?;
        let sql = if package_name.is_some() {
            format!(
                "SELECT {} FROM converted_packages
                 WHERE artifact_kind = 'repository'
                   AND source_profile = ?1 AND package_name = ?2
                   AND conversion_version = ?3",
                Self::COLUMNS
            )
        } else {
            format!(
                "SELECT {} FROM converted_packages
                 WHERE artifact_kind = 'repository'
                   AND source_profile = ?1 AND conversion_version = ?2",
                Self::COLUMNS
            )
        };

        let rows = if let Some(package_name) = package_name {
            let mut stmt = conn.prepare(&sql)?;
            stmt.query_map(
                params![source_profile, package_name, CONVERSION_VERSION],
                Self::from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            let mut stmt = conn.prepare(&sql)?;
            stmt.query_map(params![source_profile, CONVERSION_VERSION], Self::from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        rows.into_iter()
            .filter_map(
                |converted| match converted.repository_metadata_is_current(conn) {
                    Ok(true) => Some(Ok(converted)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                },
            )
            .collect()
    }

    /// Find a converted package by source_profile, name, and version (server-side lookup)
    pub fn find_by_package_identity(
        conn: &Connection,
        source_profile: &str,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Self>> {
        Self::find_by_package_identity_with_arch(conn, source_profile, name, version, None)
    }

    /// Find a converted package by source_profile, name, version, and architecture.
    pub fn find_by_package_identity_with_arch(
        conn: &Connection,
        source_profile: &str,
        name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<Option<Self>> {
        Self::require_public_source_profile(source_profile)?;
        let result = if let Some(ver) = version {
            if let Some(arch) = architecture {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE artifact_kind = 'repository' \
                     AND source_profile = ?1 AND package_name = ?2 AND package_version = ?3 \
                     AND package_architecture = ?4 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(
                    &sql,
                    params![source_profile, name, ver, arch],
                    Self::from_row,
                )
                .optional()?
            } else {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE artifact_kind = 'repository' \
                     AND source_profile = ?1 AND package_name = ?2 AND package_version = ?3 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(&sql, params![source_profile, name, ver], Self::from_row)
                    .optional()?
            }
        } else {
            if let Some(arch) = architecture {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE artifact_kind = 'repository' \
                     AND source_profile = ?1 AND package_name = ?2 AND package_architecture = ?3 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(&sql, params![source_profile, name, arch], Self::from_row)
                    .optional()?
            } else {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE artifact_kind = 'repository' \
                     AND source_profile = ?1 AND package_name = ?2 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(&sql, params![source_profile, name], Self::from_row)
                    .optional()?
            }
        };
        Ok(result)
    }

    /// Find a server-side conversion by content hash, accepting both raw and
    /// OCI-style `sha256:` references.
    pub fn find_by_content_hash_identity(
        conn: &Connection,
        source_profile: &str,
        package: &str,
        content_hash: &str,
    ) -> Result<Option<Self>> {
        Self::require_public_source_profile(source_profile)?;
        let normalized_hash = content_hash.strip_prefix("sha256:").unwrap_or(content_hash);
        let prefixed_hash = format!("sha256:{normalized_hash}");
        let sql = format!(
            "SELECT {} FROM converted_packages \
             WHERE artifact_kind = 'repository' \
             AND source_profile = ?1 AND package_name = ?2 \
             AND (content_hash = ?3 OR content_hash = ?4) \
             ORDER BY converted_at DESC LIMIT 1",
            Self::COLUMNS
        );
        let result = conn
            .query_row(
                &sql,
                params![source_profile, package, normalized_hash, prefixed_hash],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    fn require_public_source_profile(source_profile: &str) -> Result<()> {
        if crate::repository::supported_profiles::profile_by_public_id(source_profile).is_none() {
            return Err(crate::Error::ConfigError(format!(
                "unsupported public source profile '{source_profile}' for converted-package lookup"
            )));
        }
        Ok(())
    }

    /// Delete an installed conversion record by checksum.
    pub fn delete_installed_by_checksum(conn: &Connection, checksum: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM converted_packages
             WHERE artifact_kind = 'installed' AND original_checksum = ?1",
            [checksum],
        )?;
        Ok(())
    }

    /// Delete a repository conversion by exact profile and checksum.
    pub fn delete_repository_by_checksum(
        conn: &Connection,
        source_profile: &str,
        checksum: &str,
    ) -> Result<()> {
        Self::require_public_source_profile(source_profile)?;
        conn.execute(
            "DELETE FROM converted_packages
             WHERE artifact_kind = 'repository'
               AND source_profile = ?1 AND original_checksum = ?2",
            params![source_profile, checksum],
        )?;
        Ok(())
    }

    /// Count converted packages by format
    pub fn count_by_format(conn: &Connection) -> Result<Vec<(String, i64)>> {
        let mut stmt = conn.prepare(
            "SELECT original_format, COUNT(*) FROM converted_packages GROUP BY original_format ORDER BY COUNT(*) DESC",
        )?;

        let results = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    // Provenance-enhancement methods.

    /// Update enhancement status for this package
    pub fn update_enhancement_status(
        &mut self,
        conn: &Connection,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::Error::NotFound(
                "Cannot update enhancement status on unsaved package".to_string(),
            )
        })?;

        conn.execute(
            "UPDATE converted_packages SET enhancement_status = ?1, enhancement_error = ?2, enhancement_attempted_at = datetime('now') WHERE id = ?3",
            rusqlite::params![status, error, id],
        )?;

        self.enhancement_status = status.to_string();
        self.enhancement_error = error.map(|s| s.to_string());
        Ok(())
    }

    /// Mark enhancement as complete with results
    pub fn set_enhancement_complete(
        &mut self,
        conn: &Connection,
        version: i32,
        extracted_provenance: Option<&str>,
    ) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::Error::NotFound("Cannot update enhancement on unsaved package".to_string())
        })?;

        conn.execute(
            "UPDATE converted_packages SET
                enhancement_version = ?1,
                extracted_provenance_json = ?2,
                enhancement_status = 'complete',
                enhancement_error = NULL,
                enhancement_attempted_at = datetime('now')
             WHERE id = ?3",
            rusqlite::params![version, extracted_provenance, id],
        )?;

        self.enhancement_version = version;
        self.extracted_provenance_json = extracted_provenance.map(|s| s.to_string());
        self.enhancement_status = "complete".to_string();
        self.enhancement_error = None;
        Ok(())
    }

    /// Mark enhancement as failed with error message
    pub fn set_enhancement_failed(&mut self, conn: &Connection, error: &str) -> Result<()> {
        self.update_enhancement_status(conn, "failed", Some(error))
    }

    /// Check if this package needs enhancement
    pub fn needs_enhancement(&self, current_version: i32) -> bool {
        self.enhancement_status == "pending"
            || (self.enhancement_status == "complete" && self.enhancement_version < current_version)
    }
}

fn bare_sha256(value: &str) -> &str {
    value.strip_prefix("sha256:").unwrap_or(value)
}

fn default_scriptlet_summary_json() -> String {
    serde_json::to_string(&ScriptletBundleSummary::default())
        .expect("default lifecycle summary must serialize")
}

fn validate_current_scriptlet_summary(summary: &ScriptletBundleSummary) -> Result<()> {
    if !matches!(
        summary.scriptlet_fidelity.as_str(),
        "native-free" | "native-lifecycle"
    ) {
        return Err(crate::Error::InternalError(format!(
            "unsupported current lifecycle fidelity {:?}",
            summary.scriptlet_fidelity
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
