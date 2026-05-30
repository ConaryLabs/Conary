// conary-core/src/db/models/converted.rs

//! Converted package tracking model
//!
//! Tracks packages converted from legacy formats (RPM/DEB/Arch) to CCS format.
//! This enables:
//! - Skip re-conversion of same package artifact (checksum-based dedup)
//! - Track conversion fidelity for debugging and user warnings
//! - Store detected hooks extracted from scriptlets
//! - Re-convert when conversion algorithm is upgraded

use crate::ccs::convert::ScriptletBundleSummary;
use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// Current conversion algorithm version
/// Bump this when making changes that require re-conversion of existing packages.
///
/// v4 invalidates Remi artifacts produced before passive legacy scriptlet
/// bundles and scriptlet metadata were embedded in converted CCS manifests.
pub const CONVERSION_VERSION: i32 = 4;

/// A converted package record
#[derive(Debug, Clone)]
pub struct ConvertedPackage {
    pub id: Option<i64>,
    /// Reference to the converted trove (CCS package that was installed)
    pub trove_id: Option<i64>,
    /// Original package format (rpm, deb, arch)
    pub original_format: String,
    /// Checksum of original package file (skip if already converted)
    pub original_checksum: String,
    /// Conversion algorithm version (re-convert if upgraded)
    pub conversion_version: i32,
    /// Fidelity level achieved (full, high, partial, low)
    pub conversion_fidelity: String,
    /// JSON of extracted hooks and fidelity details
    pub detected_hooks: Option<String>,
    /// When the conversion occurred
    pub converted_at: Option<String>,

    // Enhancement fields (v36)
    /// Enhancement algorithm version (0 = not enhanced yet)
    pub enhancement_version: i32,
    /// Raw inferred capabilities JSON (for audit trail)
    pub inferred_caps_json: Option<String>,
    /// Extracted provenance JSON (before DB insertion)
    pub extracted_provenance_json: Option<String>,
    /// Enhancement status: pending, in_progress, complete, failed, skipped
    pub enhancement_status: String,
    /// Error message if enhancement failed
    pub enhancement_error: Option<String>,
    /// When enhancement was last attempted
    pub enhancement_attempted_at: Option<String>,

    // Server-side conversion tracking fields (v38)
    /// Package name (for server-side lookups)
    pub package_name: Option<String>,
    /// Package version (for server-side lookups)
    pub package_version: Option<String>,
    /// Distribution (fedora, arch, ubuntu, debian)
    pub distro: Option<String>,
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

    // Passive legacy scriptlet metadata fields (v70)
    /// Aggregate scriptlet fidelity from passive bundle construction.
    pub scriptlet_fidelity: String,
    /// Aggregate target compatibility from passive bundle construction.
    pub target_compatibility: String,
    /// Passive publication status. Goal 4 stores this only; it is not enforced.
    pub publication_status: String,
    /// Digest of normalized scriptlet evidence.
    pub evidence_digest: Option<String>,
    /// Digest of curated review evidence, when available.
    pub curation_evidence_digest: Option<String>,
    /// JSON array of blocked scriptlet reason codes for cheap filtering.
    pub blocked_reason_codes_json: String,
    /// JSON-encoded internal scriptlet summary for API/index projection.
    pub scriptlet_summary_json: String,
    /// Local review artifact path, never exposed directly by public APIs.
    pub review_artifact_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletSummaryForPublication {
    pub summary: ScriptletBundleSummary,
    pub valid: bool,
}

impl ConvertedPackage {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, trove_id, original_format, original_checksum, \
         conversion_version, conversion_fidelity, detected_hooks, converted_at, \
         enhancement_version, inferred_caps_json, extracted_provenance_json, \
         enhancement_status, enhancement_error, enhancement_attempted_at, \
         package_name, package_version, distro, chunk_hashes_json, total_size, \
         content_hash, ccs_path, package_architecture, scriptlet_fidelity, \
         target_compatibility, publication_status, evidence_digest, \
         curation_evidence_digest, blocked_reason_codes_json, \
         scriptlet_summary_json, review_artifact_path";

    /// Create a new converted package record
    pub fn new(
        original_format: String,
        original_checksum: String,
        conversion_fidelity: String,
    ) -> Self {
        Self {
            id: None,
            trove_id: None,
            original_format,
            original_checksum,
            conversion_version: CONVERSION_VERSION,
            conversion_fidelity,
            detected_hooks: None,
            converted_at: None,
            // Enhancement starts as pending with version 0
            enhancement_version: 0,
            inferred_caps_json: None,
            extracted_provenance_json: None,
            enhancement_status: "pending".to_string(),
            enhancement_error: None,
            enhancement_attempted_at: None,
            // Server-side fields start as None
            package_name: None,
            package_version: None,
            distro: None,
            package_architecture: None,
            chunk_hashes_json: None,
            total_size: None,
            content_hash: None,
            ccs_path: None,
            scriptlet_fidelity: "unknown".to_string(),
            target_compatibility: "unknown".to_string(),
            publication_status: "public".to_string(),
            evidence_digest: None,
            curation_evidence_digest: None,
            blocked_reason_codes_json: "[]".to_string(),
            scriptlet_summary_json: "{}".to_string(),
            review_artifact_path: None,
        }
    }

    /// Create a new server-side converted package record (for Remi)
    #[allow(clippy::too_many_arguments)]
    pub fn new_server(
        distro: String,
        package_name: String,
        package_version: String,
        original_format: String,
        original_checksum: String,
        conversion_fidelity: String,
        chunk_hashes: &[String],
        total_size: i64,
        content_hash: String,
        ccs_path: String,
    ) -> Self {
        Self {
            id: None,
            trove_id: None,
            original_format,
            original_checksum,
            conversion_version: CONVERSION_VERSION,
            conversion_fidelity,
            detected_hooks: None,
            converted_at: None,
            enhancement_version: 0,
            inferred_caps_json: None,
            extracted_provenance_json: None,
            enhancement_status: "pending".to_string(),
            enhancement_error: None,
            enhancement_attempted_at: None,
            package_name: Some(package_name),
            package_version: Some(package_version),
            distro: Some(distro),
            package_architecture: None,
            chunk_hashes_json: Some(
                serde_json::to_string(chunk_hashes).unwrap_or_else(|_| "[]".to_string()),
            ),
            total_size: Some(total_size),
            content_hash: Some(content_hash),
            ccs_path: Some(ccs_path),
            scriptlet_fidelity: "unknown".to_string(),
            target_compatibility: "unknown".to_string(),
            publication_status: "public".to_string(),
            evidence_digest: None,
            curation_evidence_digest: None,
            blocked_reason_codes_json: "[]".to_string(),
            scriptlet_summary_json: "{}".to_string(),
            review_artifact_path: None,
        }
    }

    /// Create from a database row
    ///
    /// Schema v52 guarantees all columns exist -- no compat fallbacks needed.
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            trove_id: row.get(1)?,
            original_format: row.get(2)?,
            original_checksum: row.get(3)?,
            conversion_version: row.get(4)?,
            conversion_fidelity: row.get(5)?,
            detected_hooks: row.get(6)?,
            converted_at: row.get(7)?,
            enhancement_version: row.get(8)?,
            inferred_caps_json: row.get(9)?,
            extracted_provenance_json: row.get(10)?,
            enhancement_status: row.get(11)?,
            enhancement_error: row.get(12)?,
            enhancement_attempted_at: row.get(13)?,
            package_name: row.get(14)?,
            package_version: row.get(15)?,
            distro: row.get(16)?,
            chunk_hashes_json: row.get(17)?,
            total_size: row.get(18)?,
            content_hash: row.get(19)?,
            ccs_path: row.get(20)?,
            package_architecture: row.get(21)?,
            scriptlet_fidelity: row.get(22)?,
            target_compatibility: row.get(23)?,
            publication_status: row.get(24)?,
            evidence_digest: row.get(25)?,
            curation_evidence_digest: row.get(26)?,
            blocked_reason_codes_json: row.get(27)?,
            scriptlet_summary_json: row.get(28)?,
            review_artifact_path: row.get(29)?,
        })
    }

    /// Insert this converted package into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks,
                enhancement_version, inferred_caps_json, extracted_provenance_json, enhancement_status,
                package_name, package_version, distro, chunk_hashes_json, total_size, content_hash, ccs_path, package_architecture,
                scriptlet_fidelity, target_compatibility, publication_status, evidence_digest, curation_evidence_digest,
                blocked_reason_codes_json, scriptlet_summary_json, review_artifact_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
            params![
                self.trove_id,
                &self.original_format,
                &self.original_checksum,
                self.conversion_version,
                &self.conversion_fidelity,
                &self.detected_hooks,
                self.enhancement_version,
                &self.inferred_caps_json,
                &self.extracted_provenance_json,
                &self.enhancement_status,
                &self.package_name,
                &self.package_version,
                &self.distro,
                &self.chunk_hashes_json,
                self.total_size,
                &self.content_hash,
                &self.ccs_path,
                &self.package_architecture,
                &self.scriptlet_fidelity,
                &self.target_compatibility,
                &self.publication_status,
                &self.evidence_digest,
                &self.curation_evidence_digest,
                &self.blocked_reason_codes_json,
                &self.scriptlet_summary_json,
                &self.review_artifact_path,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Update the trove_id after the converted package is installed
    pub fn set_trove_id(&mut self, conn: &Connection, trove_id: i64) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::Error::NotFound("Cannot update trove_id on unconverted package".to_string())
        })?;

        conn.execute(
            "UPDATE converted_packages SET trove_id = ?1 WHERE id = ?2",
            params![trove_id, id],
        )?;

        self.trove_id = Some(trove_id);
        Ok(())
    }

    /// Find a converted package by its original checksum
    pub fn find_by_checksum(conn: &Connection, checksum: &str) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM converted_packages WHERE original_checksum = ?1",
            Self::COLUMNS
        );
        let result = conn
            .query_row(&sql, [checksum], Self::from_row)
            .optional()?;
        Ok(result)
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
        self.conversion_version < CONVERSION_VERSION
    }

    /// Store passive scriptlet metadata generated during conversion.
    pub fn set_scriptlet_metadata(
        &mut self,
        summary: &ScriptletBundleSummary,
    ) -> serde_json::Result<()> {
        self.scriptlet_fidelity = summary.scriptlet_fidelity.clone();
        self.target_compatibility = summary.target_compatibility.clone();
        self.publication_status = summary.publication_status.clone();
        self.evidence_digest = summary.evidence_digest.clone();
        self.curation_evidence_digest = summary.curation_evidence_digest.clone();
        self.blocked_reason_codes_json = serde_json::to_string(&summary.blocked_reason_codes)?;
        self.scriptlet_summary_json = serde_json::to_string(summary)?;
        self.review_artifact_path = summary.review_artifact_path.clone();
        Ok(())
    }

    /// Recover the passive scriptlet summary, using scalar columns when the
    /// JSON blob is missing or malformed.
    pub fn scriptlet_summary(&self) -> ScriptletBundleSummary {
        match serde_json::from_str::<ScriptletBundleSummary>(&self.scriptlet_summary_json) {
            Ok(mut summary) => {
                summary.scriptlet_fidelity = self.scriptlet_fidelity.clone();
                summary.target_compatibility = self.target_compatibility.clone();
                summary.publication_status = self.publication_status.clone();
                summary.evidence_digest = self.evidence_digest.clone();
                summary.curation_evidence_digest = self.curation_evidence_digest.clone();
                summary.review_artifact_path = self.review_artifact_path.clone();
                summary
            }
            Err(error) => {
                tracing::warn!(
                    "failed to parse converted package scriptlet summary JSON: {}",
                    error
                );
                let mut summary = ScriptletBundleSummary {
                    scriptlet_fidelity: self.scriptlet_fidelity.clone(),
                    target_compatibility: self.target_compatibility.clone(),
                    publication_status: self.publication_status.clone(),
                    evidence_digest: self.evidence_digest.clone(),
                    curation_evidence_digest: self.curation_evidence_digest.clone(),
                    review_artifact_path: self.review_artifact_path.clone(),
                    ..ScriptletBundleSummary::default()
                };
                summary.blocked_reason_codes =
                    serde_json::from_str(&self.blocked_reason_codes_json).unwrap_or_default();
                summary
            }
        }
    }

    pub fn scriptlet_publication_status(&self) -> &str {
        self.publication_status.as_str()
    }

    pub fn scriptlet_summary_for_publication(&self) -> ScriptletSummaryForPublication {
        let value = match serde_json::from_str::<serde_json::Value>(&self.scriptlet_summary_json) {
            Ok(value) => value,
            Err(_) => {
                return ScriptletSummaryForPublication {
                    summary: self.scriptlet_summary(),
                    valid: false,
                };
            }
        };

        let shape_valid = self.summary_json_shape_valid_for_publication(&value);
        let summary = self.scriptlet_summary();
        let status_matches = value
            .get("publication_status")
            .and_then(|value| value.as_str())
            .map(|status| status == self.publication_status)
            .unwrap_or_else(|| self.is_default_scriptlet_publication_shape(&value));

        ScriptletSummaryForPublication {
            summary,
            valid: shape_valid && status_matches,
        }
    }

    pub fn is_scriptlet_public_ready(&self) -> bool {
        let publication = self.scriptlet_summary_for_publication();
        publication.valid && publication.summary.publication_status == "public"
    }

    pub fn parsed_chunk_hashes(&self) -> Vec<String> {
        self.chunk_hashes_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
            .unwrap_or_default()
    }

    fn summary_json_shape_valid_for_publication(&self, value: &serde_json::Value) -> bool {
        if self.is_default_scriptlet_publication_shape(value) {
            return true;
        }

        let Some(object) = value.as_object() else {
            return false;
        };

        [
            "scriptlet_fidelity",
            "target_compatibility",
            "publication_status",
            "decision_counts",
            "blocked_reason_codes",
            "review_reason_codes",
            "unknown_commands",
            "blocked_classes",
        ]
        .iter()
        .all(|key| object.contains_key(*key))
    }

    fn is_default_scriptlet_publication_shape(&self, value: &serde_json::Value) -> bool {
        value.as_object().is_some_and(|object| object.is_empty())
            && self.scriptlet_fidelity == "unknown"
            && self.target_compatibility == "unknown"
            && self.publication_status == "public"
            && self.evidence_digest.is_none()
            && self.curation_evidence_digest.is_none()
            && json_string_array_is_empty(&self.blocked_reason_codes_json)
            && self.review_artifact_path.is_none()
    }

    /// List all converted packages with a specific fidelity level
    pub fn find_by_fidelity(conn: &Connection, fidelity: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM converted_packages WHERE conversion_fidelity = ?1 \
             ORDER BY converted_at DESC",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let results = stmt
            .query_map([fidelity], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
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

    /// Find a converted package by distro, name, and version (server-side lookup)
    pub fn find_by_package_identity(
        conn: &Connection,
        distro: &str,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Self>> {
        Self::find_by_package_identity_with_arch(conn, distro, name, version, None)
    }

    /// Find a converted package by distro, name, version, and architecture.
    pub fn find_by_package_identity_with_arch(
        conn: &Connection,
        distro: &str,
        name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<Option<Self>> {
        let result = if let Some(ver) = version {
            if let Some(arch) = architecture {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE distro = ?1 AND package_name = ?2 AND package_version = ?3 \
                     AND package_architecture = ?4 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(&sql, params![distro, name, ver, arch], Self::from_row)
                    .optional()?
            } else {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE distro = ?1 AND package_name = ?2 AND package_version = ?3 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(&sql, params![distro, name, ver], Self::from_row)
                    .optional()?
            }
        } else {
            if let Some(arch) = architecture {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE distro = ?1 AND package_name = ?2 AND package_architecture = ?3 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(&sql, params![distro, name, arch], Self::from_row)
                    .optional()?
            } else {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE distro = ?1 AND package_name = ?2 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(&sql, params![distro, name], Self::from_row)
                    .optional()?
            }
        };
        Ok(result)
    }

    /// Find a server-side conversion by content hash, accepting both raw and
    /// OCI-style `sha256:` references.
    pub fn find_by_content_hash_identity(
        conn: &Connection,
        distro: &str,
        package: &str,
        content_hash: &str,
    ) -> Result<Option<Self>> {
        let normalized_hash = content_hash.strip_prefix("sha256:").unwrap_or(content_hash);
        let prefixed_hash = format!("sha256:{normalized_hash}");
        let sql = format!(
            "SELECT {} FROM converted_packages \
             WHERE distro = ?1 AND package_name = ?2 \
             AND (content_hash = ?3 OR content_hash = ?4) \
             ORDER BY converted_at DESC LIMIT 1",
            Self::COLUMNS
        );
        let result = conn
            .query_row(
                &sql,
                params![distro, package, normalized_hash, prefixed_hash],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Delete a converted package record by checksum
    pub fn delete_by_checksum(conn: &Connection, checksum: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM converted_packages WHERE original_checksum = ?1",
            [checksum],
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

    // Enhancement-related methods (v36)

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
        inferred_caps: Option<&str>,
        extracted_provenance: Option<&str>,
    ) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::Error::NotFound("Cannot update enhancement on unsaved package".to_string())
        })?;

        conn.execute(
            "UPDATE converted_packages SET
                enhancement_version = ?1,
                inferred_caps_json = ?2,
                extracted_provenance_json = ?3,
                enhancement_status = 'complete',
                enhancement_error = NULL,
                enhancement_attempted_at = datetime('now')
             WHERE id = ?4",
            rusqlite::params![version, inferred_caps, extracted_provenance, id],
        )?;

        self.enhancement_version = version;
        self.inferred_caps_json = inferred_caps.map(|s| s.to_string());
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

fn json_string_array_is_empty(value: &str) -> bool {
    match serde_json::from_str::<Vec<String>>(value) {
        Ok(values) => values.is_empty(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::convert::ScriptletBundleSummary;
    use crate::db::testing::create_test_db;

    #[test]
    fn converted_package_defaults_scriptlet_metadata() {
        let converted = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:source".to_string(),
            "high".to_string(),
        );

        assert_eq!(converted.scriptlet_fidelity, "unknown");
        assert_eq!(converted.target_compatibility, "unknown");
        assert_eq!(converted.publication_status, "public");
        assert_eq!(converted.blocked_reason_codes_json, "[]");
        assert_eq!(converted.scriptlet_summary_json, "{}");
        assert_eq!(converted.review_artifact_path, None);
    }

    #[test]
    fn converted_package_round_trips_scriptlet_metadata() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::migrate(&conn).unwrap();
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "gtk3".to_string(),
            "3.24.0-1.fc44".to_string(),
            "rpm".to_string(),
            "sha256:source".to_string(),
            "high".to_string(),
            &["sha256:chunk".to_string()],
            42,
            "sha256:content".to_string(),
            "/tmp/gtk3.ccs".to_string(),
        );
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "review-required".to_string(),
            publication_status: "private-review".to_string(),
            evidence_digest: Some(crate::hash::sha256_prefixed(b"evidence")),
            blocked_reason_codes: vec!["blocked-class-network".to_string()],
            review_reason_codes: vec!["review-class-debconf".to_string()],
            unknown_commands: vec!["custom-helper".to_string()],
            blocked_classes: vec!["network".to_string()],
            ..ScriptletBundleSummary::default()
        };
        converted.set_scriptlet_metadata(&summary).unwrap();
        converted.insert(&conn).unwrap();

        let found = ConvertedPackage::find_by_package_identity_with_arch(
            &conn,
            "fedora",
            "gtk3",
            Some("3.24.0-1.fc44"),
            None,
        )
        .unwrap()
        .unwrap();

        assert_eq!(found.scriptlet_fidelity, "review-required");
        assert_eq!(found.target_compatibility, "review-required");
        assert_eq!(found.publication_status, "private-review");
        assert_eq!(
            found.blocked_reason_codes_json,
            "[\"blocked-class-network\"]"
        );
        assert!(found.scriptlet_summary_json.contains("custom-helper"));
    }

    #[test]
    fn scriptlet_summary_recovers_from_malformed_json_with_scalar_fields() {
        let mut converted = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:source".to_string(),
            "high".to_string(),
        );
        converted.scriptlet_fidelity = "blocked".to_string();
        converted.target_compatibility = "blocked".to_string();
        converted.publication_status = "blocked".to_string();
        converted.evidence_digest = Some(crate::hash::sha256_prefixed(b"fallback-evidence"));
        converted.blocked_reason_codes_json = "[\"blocked-class-network\"]".to_string();
        converted.scriptlet_summary_json = "{not valid json".to_string();

        let summary = converted.scriptlet_summary();

        assert_eq!(summary.scriptlet_fidelity, "blocked");
        assert_eq!(summary.target_compatibility, "blocked");
        assert_eq!(summary.publication_status, "blocked");
        assert_eq!(
            summary.evidence_digest,
            Some(crate::hash::sha256_prefixed(b"fallback-evidence"))
        );
        assert_eq!(summary.blocked_reason_codes, vec!["blocked-class-network"]);
        assert!(summary.review_reason_codes.is_empty());
        assert!(summary.unknown_commands.is_empty());
    }

    #[test]
    fn scriptlet_summary_for_publication_accepts_constructor_default_shape() {
        let converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "plain".to_string(),
            "1.0".to_string(),
            "ccs".to_string(),
            "upload:fedora:abc".to_string(),
            "full".to_string(),
            &["abc".to_string()],
            3,
            "abc".to_string(),
            "/tmp/plain.ccs".to_string(),
        );

        let publication = converted.scriptlet_summary_for_publication();

        assert!(publication.valid);
        assert_eq!(publication.summary.publication_status, "public");
        assert!(converted.is_scriptlet_public_ready());
    }

    #[test]
    fn scriptlet_summary_for_publication_rejects_default_json_with_scriptlet_evidence() {
        let mut converted = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:source".to_string(),
            "high".to_string(),
        );
        converted.scriptlet_fidelity = "blocked".to_string();
        converted.target_compatibility = "blocked".to_string();
        converted.publication_status = "public".to_string();
        converted.evidence_digest = Some(crate::hash::sha256_prefixed(b"evidence"));
        converted.scriptlet_summary_json = "{}".to_string();

        let publication = converted.scriptlet_summary_for_publication();

        assert!(!publication.valid);
        assert!(!converted.is_scriptlet_public_ready());
    }

    #[test]
    fn scriptlet_summary_for_publication_rejects_partial_and_malformed_json() {
        let mut converted = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:source".to_string(),
            "high".to_string(),
        );
        converted.scriptlet_summary_json = r#"{"publication_status":"public"}"#.to_string();
        assert!(!converted.scriptlet_summary_for_publication().valid);

        converted.scriptlet_summary_json = "{not valid json".to_string();
        assert!(!converted.scriptlet_summary_for_publication().valid);
    }

    #[test]
    fn scriptlet_public_ready_requires_valid_summary_and_public_status() {
        let mut converted = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:source".to_string(),
            "high".to_string(),
        );
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "review-required".to_string(),
            publication_status: "private-review".to_string(),
            review_reason_codes: vec!["review-class-debconf".to_string()],
            ..ScriptletBundleSummary::default()
        };
        converted.set_scriptlet_metadata(&summary).unwrap();

        assert!(converted.scriptlet_summary_for_publication().valid);
        assert!(!converted.is_scriptlet_public_ready());
    }

    #[test]
    fn test_converted_package_crud() {
        let (_temp, conn) = create_test_db();

        // Create a converted package
        let mut converted = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:abc123def456".to_string(),
            "high".to_string(),
        );
        converted.detected_hooks = Some(r#"{"users": [{"name": "nginx"}]}"#.to_string());

        let id = converted.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by checksum
        let found = ConvertedPackage::find_by_checksum(&conn, "sha256:abc123def456")
            .unwrap()
            .unwrap();
        assert_eq!(found.original_format, "rpm");
        assert_eq!(found.conversion_fidelity, "high");
        assert!(found.detected_hooks.is_some());

        // List all
        let all = ConvertedPackage::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);

        // Delete
        ConvertedPackage::delete_by_checksum(&conn, "sha256:abc123def456").unwrap();
        let deleted = ConvertedPackage::find_by_checksum(&conn, "sha256:abc123def456").unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_needs_reconversion() {
        let mut converted = ConvertedPackage::new(
            "deb".to_string(),
            "sha256:test".to_string(),
            "full".to_string(),
        );
        converted.conversion_version = CONVERSION_VERSION;

        assert!(!converted.needs_reconversion());

        converted.conversion_version = CONVERSION_VERSION - 1;
        assert!(converted.needs_reconversion());
    }

    #[test]
    fn test_find_by_fidelity() {
        let (_temp, conn) = create_test_db();

        // Create multiple converted packages
        let mut high1 = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:111".to_string(),
            "high".to_string(),
        );
        high1.insert(&conn).unwrap();

        let mut high2 = ConvertedPackage::new(
            "deb".to_string(),
            "sha256:222".to_string(),
            "high".to_string(),
        );
        high2.insert(&conn).unwrap();

        let mut low1 = ConvertedPackage::new(
            "arch".to_string(),
            "sha256:333".to_string(),
            "low".to_string(),
        );
        low1.insert(&conn).unwrap();

        // Find by fidelity
        let high = ConvertedPackage::find_by_fidelity(&conn, "high").unwrap();
        assert_eq!(high.len(), 2);

        let low = ConvertedPackage::find_by_fidelity(&conn, "low").unwrap();
        assert_eq!(low.len(), 1);
    }

    #[test]
    fn test_count_by_format() {
        let (_temp, conn) = create_test_db();

        // Create converted packages with different formats
        let mut rpm1 = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:r1".to_string(),
            "high".to_string(),
        );
        rpm1.insert(&conn).unwrap();

        let mut rpm2 = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:r2".to_string(),
            "high".to_string(),
        );
        rpm2.insert(&conn).unwrap();

        let mut deb1 = ConvertedPackage::new(
            "deb".to_string(),
            "sha256:d1".to_string(),
            "high".to_string(),
        );
        deb1.insert(&conn).unwrap();

        // Count by format
        let counts = ConvertedPackage::count_by_format(&conn).unwrap();
        assert_eq!(counts.len(), 2);

        // RPM should be first (most common)
        assert_eq!(counts[0].0, "rpm");
        assert_eq!(counts[0].1, 2);
        assert_eq!(counts[1].0, "deb");
        assert_eq!(counts[1].1, 1);
    }

    #[test]
    fn test_unique_checksum_constraint() {
        let (_temp, conn) = create_test_db();

        let mut converted1 = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:same_checksum".to_string(),
            "high".to_string(),
        );
        converted1.insert(&conn).unwrap();

        // Try to insert with same checksum - should fail
        let mut converted2 = ConvertedPackage::new(
            "deb".to_string(),
            "sha256:same_checksum".to_string(),
            "full".to_string(),
        );
        let result = converted2.insert(&conn);
        assert!(result.is_err());
    }

    #[test]
    fn test_enhancement_methods() {
        let (_temp, conn) = create_test_db();

        // Create and insert a converted package
        let mut converted = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:enhance_test".to_string(),
            "high".to_string(),
        );
        converted.insert(&conn).unwrap();

        // Check initial enhancement state
        assert_eq!(converted.enhancement_status, "pending");
        assert_eq!(converted.enhancement_version, 0);
        assert!(converted.needs_enhancement(1));

        // Mark as complete
        converted
            .set_enhancement_complete(&conn, 1, Some(r#"{"network": true}"#), None)
            .unwrap();
        assert_eq!(converted.enhancement_status, "complete");
        assert_eq!(converted.enhancement_version, 1);
        assert!(!converted.needs_enhancement(1));
        assert!(converted.needs_enhancement(2)); // outdated

        // Verify persisted in database
        let found = ConvertedPackage::find_by_checksum(&conn, "sha256:enhance_test")
            .unwrap()
            .unwrap();
        assert_eq!(found.enhancement_status, "complete");
        assert_eq!(found.enhancement_version, 1);
        assert!(found.inferred_caps_json.is_some());
    }

    #[test]
    fn test_enhancement_failure() {
        let (_temp, conn) = create_test_db();

        let mut converted = ConvertedPackage::new(
            "deb".to_string(),
            "sha256:fail_test".to_string(),
            "partial".to_string(),
        );
        converted.insert(&conn).unwrap();

        // Mark as failed
        converted
            .set_enhancement_failed(&conn, "Test error message")
            .unwrap();
        assert_eq!(converted.enhancement_status, "failed");
        assert_eq!(
            converted.enhancement_error.as_deref(),
            Some("Test error message")
        );

        // Verify persisted
        let found = ConvertedPackage::find_by_checksum(&conn, "sha256:fail_test")
            .unwrap()
            .unwrap();
        assert_eq!(found.enhancement_status, "failed");
        assert!(found.enhancement_error.is_some());
    }
}
