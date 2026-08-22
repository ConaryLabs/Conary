// crates/conary-core/src/db/models/converted/enhancement.rs

//! Lifecycle evidence and provenance enhancement state.

use super::validation::validate_current_scriptlet_summary;
use super::{CONVERSION_VERSION, ChunkConversionState, ConvertedPackage};
use crate::ccs::convert::ScriptletBundleSummary;
use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

impl ConvertedPackage {
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

    pub fn transport(&self) -> Result<crate::ccs::transport::CcsTransportEnvelopeV1> {
        self.repository_artifact()
            .map(|artifact| artifact.transport)
    }

    pub fn object_hashes(&self) -> Result<Vec<String>> {
        self.transport().map(|transport| {
            transport
                .objects
                .into_iter()
                .map(|object| object.sha256)
                .collect()
        })
    }

    /// Classify whether a CAS chunk is reachable from a current conversion,
    /// stale conversions only, or no conversions.
    pub fn chunk_conversion_state(conn: &Connection, hash: &str) -> Result<ChunkConversionState> {
        Self::chunk_conversion_state_for_revision_impl(conn, hash, None)
    }

    /// Classify a CAS chunk only against conversions bound to one exact
    /// immutable profile revision.
    pub fn chunk_conversion_state_for_revision(
        conn: &Connection,
        profile_revision_sha256: &str,
        hash: &str,
    ) -> Result<ChunkConversionState> {
        super::validation::validate_sha256(profile_revision_sha256, "profile revision SHA-256")?;
        Self::chunk_conversion_state_for_revision_impl(conn, hash, Some(profile_revision_sha256))
    }

    fn chunk_conversion_state_for_revision_impl(
        conn: &Connection,
        hash: &str,
        profile_revision_sha256: Option<&str>,
    ) -> Result<ChunkConversionState> {
        let bare_hash = hash.strip_prefix("sha256:").unwrap_or(hash);
        let prefixed_hash = format!("sha256:{bare_hash}");
        let bare_pattern = format!("%\"{bare_hash}\"%");
        let prefixed_pattern = format!("%\"{prefixed_hash}\"%");

        let sql = if profile_revision_sha256.is_some() {
            format!(
                "SELECT {} FROM converted_packages
                 WHERE artifact_kind = 'repository'
                   AND profile_revision_sha256 = ?1
                   AND transport_json IS NOT NULL
                   AND (transport_json LIKE ?2 OR transport_json LIKE ?3)",
                Self::COLUMNS
            )
        } else {
            format!(
                "SELECT {} FROM converted_packages
                 WHERE artifact_kind = 'repository'
                   AND transport_json IS NOT NULL
                   AND (transport_json LIKE ?1 OR transport_json LIKE ?2)",
                Self::COLUMNS
            )
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows = if let Some(profile_revision_sha256) = profile_revision_sha256 {
            stmt.query_map(
                params![profile_revision_sha256, bare_pattern, prefixed_pattern],
                Self::from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map(params![bare_pattern, prefixed_pattern], Self::from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        drop(stmt);

        let mut saw_converted_reference = false;
        for candidate in rows {
            let references_hash = candidate
                .object_hashes()?
                .into_iter()
                .any(|chunk_hash| chunk_hash == bare_hash || chunk_hash == prefixed_hash);
            if !references_hash {
                continue;
            }

            saw_converted_reference = true;
            let is_current = if let Some(profile_revision_sha256) = profile_revision_sha256 {
                candidate.repository_conversion_is_current_for_revision(profile_revision_sha256)?
            } else {
                candidate.repository_conversion_is_current()?
            };
            if is_current {
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

    pub(super) fn record_identity(&self) -> String {
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

    /// Find converted packages bound to one exact immutable profile revision
    /// whose conversion algorithm is current.
    ///
    /// Callers that expose lifecycle metadata must parse `scriptlet_summary()`
    /// so malformed current rows surface as data corruption.
    pub fn find_current_conversions(
        conn: &Connection,
        profile_revision_sha256: &str,
        package_name: Option<&str>,
    ) -> Result<Vec<Self>> {
        super::validation::validate_sha256(profile_revision_sha256, "profile revision SHA-256")?;
        let sql = if package_name.is_some() {
            format!(
                "SELECT {} FROM converted_packages
                 WHERE artifact_kind = 'repository'
                   AND profile_revision_sha256 = ?1 AND package_name = ?2
                   AND conversion_version = ?3",
                Self::COLUMNS
            )
        } else {
            format!(
                "SELECT {} FROM converted_packages
                 WHERE artifact_kind = 'repository'
                   AND profile_revision_sha256 = ?1 AND conversion_version = ?2",
                Self::COLUMNS
            )
        };

        let rows = if let Some(package_name) = package_name {
            let mut stmt = conn.prepare(&sql)?;
            stmt.query_map(
                params![profile_revision_sha256, package_name, CONVERSION_VERSION],
                Self::from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            let mut stmt = conn.prepare(&sql)?;
            stmt.query_map(
                params![profile_revision_sha256, CONVERSION_VERSION],
                Self::from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };

        rows.into_iter()
            .filter_map(|converted| {
                match converted
                    .repository_conversion_is_current_for_revision(profile_revision_sha256)
                {
                    Ok(true) => Some(Ok(converted)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect()
    }

    /// Find a converted package by exact profile revision, name, and version
    /// (server-side lookup).
    pub fn find_by_package_identity(
        conn: &Connection,
        profile_revision_sha256: &str,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Self>> {
        Self::find_by_package_identity_with_arch(conn, profile_revision_sha256, name, version, None)
    }

    /// Find a converted package by exact profile revision, name, version, and
    /// architecture.
    pub fn find_by_package_identity_with_arch(
        conn: &Connection,
        profile_revision_sha256: &str,
        name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<Option<Self>> {
        super::validation::validate_sha256(profile_revision_sha256, "profile revision SHA-256")?;
        let result = if let Some(ver) = version {
            if let Some(arch) = architecture {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE artifact_kind = 'repository' \
                     AND profile_revision_sha256 = ?1 AND package_name = ?2 AND package_version = ?3 \
                     AND package_architecture = ?4 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(
                    &sql,
                    params![profile_revision_sha256, name, ver, arch],
                    Self::from_row,
                )
                .optional()?
            } else {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE artifact_kind = 'repository' \
                     AND profile_revision_sha256 = ?1 AND package_name = ?2 AND package_version = ?3 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(
                    &sql,
                    params![profile_revision_sha256, name, ver],
                    Self::from_row,
                )
                .optional()?
            }
        } else {
            if let Some(arch) = architecture {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE artifact_kind = 'repository' \
                     AND profile_revision_sha256 = ?1 AND package_name = ?2 AND package_architecture = ?3 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(
                    &sql,
                    params![profile_revision_sha256, name, arch],
                    Self::from_row,
                )
                .optional()?
            } else {
                let sql = format!(
                    "SELECT {} FROM converted_packages \
                     WHERE artifact_kind = 'repository' \
                     AND profile_revision_sha256 = ?1 AND package_name = ?2 \
                     ORDER BY converted_at DESC LIMIT 1",
                    Self::COLUMNS
                );
                conn.query_row(&sql, params![profile_revision_sha256, name], Self::from_row)
                    .optional()?
            }
        };
        Ok(result)
    }

    /// Find a server-side conversion by exact profile revision and content
    /// hash, accepting both raw and OCI-style `sha256:` references.
    pub fn find_by_content_hash_identity(
        conn: &Connection,
        profile_revision_sha256: &str,
        package: &str,
        content_hash: &str,
    ) -> Result<Option<Self>> {
        super::validation::validate_sha256(profile_revision_sha256, "profile revision SHA-256")?;
        let normalized_hash = content_hash.strip_prefix("sha256:").unwrap_or(content_hash);
        let prefixed_hash = format!("sha256:{normalized_hash}");
        let sql = format!(
            "SELECT {} FROM converted_packages \
             WHERE artifact_kind = 'repository' \
             AND profile_revision_sha256 = ?1 AND package_name = ?2 \
             AND (content_hash = ?3 OR content_hash = ?4) \
             ORDER BY converted_at DESC LIMIT 1",
            Self::COLUMNS
        );
        let result = conn
            .query_row(
                &sql,
                params![
                    profile_revision_sha256,
                    package,
                    normalized_hash,
                    prefixed_hash
                ],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
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

    /// Delete a repository conversion by exact profile revision and checksum.
    pub fn delete_repository_by_checksum(
        conn: &Connection,
        profile_revision_sha256: &str,
        checksum: &str,
    ) -> Result<()> {
        super::validation::validate_sha256(profile_revision_sha256, "profile revision SHA-256")?;
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        let mut statement = tx.prepare(
            "SELECT id FROM converted_packages
             WHERE artifact_kind = 'repository'
               AND profile_revision_sha256 = ?1 AND original_checksum = ?2
             ORDER BY id",
        )?;
        let ids = statement
            .query_map(params![profile_revision_sha256, checksum], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for id in ids {
            Self::delete_with_conversion_pin_in_transaction(&tx, id)?;
        }
        tx.commit()?;
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
