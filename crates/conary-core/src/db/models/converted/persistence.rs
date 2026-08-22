// crates/conary-core/src/db/models/converted/persistence.rs

//! Durable row persistence and installed-conversion ownership.

use super::validation::default_scriptlet_summary_json;
use super::{CONVERSION_VERSION, ConvertedArtifactKind, ConvertedPackage};
use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

impl ConvertedPackage {
    /// Column list for SELECT queries.
    pub(super) const COLUMNS: &'static str = "id, artifact_kind, trove_id, original_format, original_checksum, \
         profile_revision_sha256, repository_provides_digest, conversion_version, converted_at, \
         enhancement_version, extracted_provenance_json, \
         enhancement_status, enhancement_error, enhancement_attempted_at, \
         package_name, package_version, source_profile, transport_json, total_size, \
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
            profile_revision_sha256: None,
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
            transport_json: None,
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
        profile_revision_sha256: String,
        package_name: String,
        package_version: String,
        package_architecture: String,
        original_format: String,
        original_checksum: String,
        transport: &crate::ccs::transport::CcsTransportEnvelopeV1,
        total_size: i64,
        content_hash: String,
        ccs_path: String,
        repository_provides_digest: String,
    ) -> Self {
        let transport_json =
            serde_json::to_string(transport).expect("CCS transport always serializes to JSON");
        Self {
            id: None,
            artifact_kind: ConvertedArtifactKind::Repository,
            trove_id: None,
            original_format,
            original_checksum,
            profile_revision_sha256: Some(profile_revision_sha256),
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
            transport_json: Some(transport_json),
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
    pub(super) fn from_row(row: &Row) -> rusqlite::Result<Self> {
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
            profile_revision_sha256: row.get(5)?,
            repository_provides_digest: row.get(6)?,
            conversion_version: row.get(7)?,
            converted_at: row.get(8)?,
            enhancement_version: row.get(9)?,
            extracted_provenance_json: row.get(10)?,
            enhancement_status: row.get(11)?,
            enhancement_error: row.get(12)?,
            enhancement_attempted_at: row.get(13)?,
            package_name: row.get(14)?,
            package_version: row.get(15)?,
            source_profile: row.get(16)?,
            transport_json: row.get(17)?,
            total_size: row.get(18)?,
            content_hash: row.get(19)?,
            ccs_path: row.get(20)?,
            package_architecture: row.get(21)?,
            scriptlet_fidelity: row.get(22)?,
            evidence_digest: row.get(23)?,
            scriptlet_summary_json: row.get(24)?,
        })
    }

    /// Insert this converted package into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        self.validate_artifact_contract()?;
        self.scriptlet_summary()?;
        conn.execute(
            "INSERT INTO converted_packages (artifact_kind, trove_id, original_format, original_checksum, profile_revision_sha256, repository_provides_digest, conversion_version,
                enhancement_version, extracted_provenance_json, enhancement_status,
                package_name, package_version, source_profile, transport_json, total_size, content_hash, ccs_path, package_architecture,
                scriptlet_fidelity, evidence_digest,
                scriptlet_summary_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                self.artifact_kind.as_ref(),
                self.trove_id,
                &self.original_format,
                &self.original_checksum,
                &self.profile_revision_sha256,
                &self.repository_provides_digest,
                self.conversion_version,
                self.enhancement_version,
                &self.extracted_provenance_json,
                &self.enhancement_status,
                &self.package_name,
                &self.package_version,
                &self.source_profile,
                &self.transport_json,
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

    /// Find a repository conversion by exact immutable profile revision and
    /// original checksum.
    pub fn find_repository_by_checksum(
        conn: &Connection,
        profile_revision_sha256: &str,
        checksum: &str,
    ) -> Result<Option<Self>> {
        super::validation::validate_sha256(profile_revision_sha256, "profile revision SHA-256")?;
        let sql = format!(
            "SELECT {} FROM converted_packages
             WHERE artifact_kind = 'repository'
               AND profile_revision_sha256 = ?1 AND original_checksum = ?2",
            Self::COLUMNS
        );
        conn.query_row(
            &sql,
            params![profile_revision_sha256, checksum],
            Self::from_row,
        )
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

    /// Find one converted row by its durable database identity.
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM converted_packages WHERE id = ?1",
            Self::COLUMNS
        );
        conn.query_row(&sql, [id], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    /// Delete conversion evidence for one installed trove.
    pub fn delete_installed_by_trove(conn: &Connection, trove_id: i64) -> Result<usize> {
        conn.execute(
            "DELETE FROM converted_packages
             WHERE artifact_kind = 'installed' AND trove_id = ?1",
            [trove_id],
        )
        .map_err(Into::into)
    }

    /// Deterministic owner identity for a conversion revision pin.
    pub fn conversion_pin_owner_identity(id: i64) -> String {
        id.to_string()
    }

    /// Deterministic pin identifier for a persisted conversion row.
    pub fn conversion_pin_id(id: i64) -> String {
        format!("conversion-{id}")
    }

    /// Insert a repository conversion and its exact profile-revision pin in
    /// one immediate transaction owned by this method.
    pub fn insert_with_conversion_pin(&mut self, conn: &Connection, pinned_at: i64) -> Result<i64> {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        let id = self.insert_with_conversion_pin_in_transaction(&tx, pinned_at)?;
        tx.commit()?;
        Ok(id)
    }

    /// Insert a repository conversion and its exact profile-revision pin in
    /// the caller's immediate transaction. The caller owns commit/rollback.
    pub fn insert_with_conversion_pin_in_transaction(
        &mut self,
        tx: &Transaction<'_>,
        pinned_at: i64,
    ) -> Result<i64> {
        let id = self.insert(tx)?;
        let artifact = self.repository_artifact()?;
        let pin = crate::db::models::RemiProfileRevisionPin {
            pin_id: Self::conversion_pin_id(id),
            source_profile: artifact.source_profile.to_string(),
            profile_revision_sha256: artifact.profile_revision_sha256.to_string(),
            owner_kind: crate::db::models::RemiRevisionPinKind::Conversion,
            owner_identity: Self::conversion_pin_owner_identity(id),
            runtime_session_id: None,
            pinned_at,
        };
        pin.insert(tx)?;
        Ok(id)
    }

    /// Delete a conversion row and its exact conversion pin atomically.
    pub fn delete_with_conversion_pin(conn: &Connection, id: i64) -> Result<bool> {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        let deleted = Self::delete_with_conversion_pin_in_transaction(&tx, id)?;
        tx.commit()?;
        Ok(deleted)
    }

    /// Delete a conversion row and its exact conversion pin in the caller's
    /// transaction.
    pub fn delete_with_conversion_pin_in_transaction(
        tx: &Transaction<'_>,
        id: i64,
    ) -> Result<bool> {
        let owner_identity = Self::conversion_pin_owner_identity(id);
        tx.execute(
            "DELETE FROM remi_profile_revision_pins
             WHERE owner_kind = 'conversion' AND owner_identity = ?1",
            [&owner_identity],
        )?;
        Ok(tx.execute("DELETE FROM converted_packages WHERE id = ?1", [id])? == 1)
    }

    /// Require the exact durable conversion pin for one repository row.
    /// Missing, mismatched, or malformed pin state is explicit corruption.
    pub fn require_conversion_pin(
        conn: &Connection,
        id: i64,
    ) -> Result<crate::db::models::RemiProfileRevisionPin> {
        let converted = Self::find_by_id(conn, id)?.ok_or_else(|| {
            crate::Error::NotFound(format!("converted package {id} does not exist"))
        })?;
        let artifact = converted.repository_artifact()?;
        let resource = crate::db::models::RemiCatalogResource::find_profile_revision(
            conn,
            artifact.source_profile,
            artifact.profile_revision_sha256,
        )?
        .ok_or_else(|| {
            crate::Error::InternalError(format!(
                "repository conversion {id} profile revision resource is missing or mismatched"
            ))
        })?;
        if !resource.durable {
            return Err(crate::Error::InternalError(format!(
                "repository conversion {id} profile revision resource is not durable"
            )));
        }
        let pin_id = Self::conversion_pin_id(id);
        let pin =
            crate::db::models::RemiProfileRevisionPin::find(conn, &pin_id)?.ok_or_else(|| {
                crate::Error::InternalError(format!(
                    "repository conversion {id} has no exact profile-revision pin"
                ))
            })?;
        if pin.owner_kind != crate::db::models::RemiRevisionPinKind::Conversion
            || pin.owner_identity != Self::conversion_pin_owner_identity(id)
            || pin.source_profile != artifact.source_profile
            || pin.profile_revision_sha256 != artifact.profile_revision_sha256
            || pin.runtime_session_id.is_some()
        {
            return Err(crate::Error::InternalError(format!(
                "repository conversion {id} has mismatched profile-revision pin identity"
            )));
        }
        Ok(pin)
    }
}
