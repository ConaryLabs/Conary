// conary-core/src/db/models/remi_catalog.rs

//! Operational metadata for immutable Remi source and profile catalogs.
//!
//! The package catalog itself lives in standalone, read-only SQLite files.
//! These models hold only the exact resource identities needed to activate
//! and retain those files. In particular, no package, provide, or requirement
//! row belongs here.

mod validation;
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use std::time::{SystemTime, UNIX_EPOCH};

use validation::{validate_canonical_manifest, validate_identity, validate_sha256, validate_uuid};

const RESOURCE_COLUMNS: &str = "resource_sha256, resource_kind, source_profile, \
    artifact_sha256, artifact_size, logical_digest_sha256, manifest_json, durable, created_at";
const MEMBER_COLUMNS: &str = "profile_revision_sha256, ordinal, source_snapshot_sha256, \
    source_identity, repository_identity, stream_kind, stream_identity, priority, required";
const ACTIVE_COLUMNS: &str = "source_profile, profile_revision_sha256, fencing_epoch, \
    activation_run_id, owner_instance_uuid, activated_at";
const PIN_COLUMNS: &str = "pin_id, source_profile, profile_revision_sha256, owner_kind, \
    owner_identity, pinned_at";

/// The two resource classes that may be referenced by an activated profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemiCatalogResourceKind {
    SourceSnapshot,
    ProfileRevision,
}

impl RemiCatalogResourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceSnapshot => "source_snapshot",
            Self::ProfileRevision => "profile_revision",
        }
    }

    fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "source_snapshot" => Ok(Self::SourceSnapshot),
            "profile_revision" => Ok(Self::ProfileRevision),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                format!("invalid Remi catalog resource kind {other}").into(),
            )),
        }
    }
}

/// One durable catalog artifact and the manifest metadata that binds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiCatalogResource {
    /// SHA-256 of the canonical SourceSnapshotV1 or ProfileRevisionV1 manifest.
    pub resource_sha256: String,
    pub kind: RemiCatalogResourceKind,
    pub source_profile: String,
    /// SHA-256 of the standalone catalog SQLite file.
    pub artifact_sha256: String,
    pub artifact_size: i64,
    pub logical_digest_sha256: String,
    /// Canonical JSON bytes of the versioned manifest, stored as UTF-8 text.
    pub manifest_json: String,
    /// Set only after the artifact and its containing directory were durably
    /// synchronized and the immutable publication path exists.
    pub durable: bool,
    pub created_at: i64,
}

impl RemiCatalogResource {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        self.validate()?;
        conn.execute(
            "INSERT INTO remi_catalog_resources (
                 resource_sha256, resource_kind, source_profile, artifact_sha256,
                 artifact_size, logical_digest_sha256, manifest_json, durable, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                &self.resource_sha256,
                self.kind.as_str(),
                &self.source_profile,
                &self.artifact_sha256,
                self.artifact_size,
                &self.logical_digest_sha256,
                &self.manifest_json,
                self.durable as i64,
                self.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn find_by_sha256(conn: &Connection, resource_sha256: &str) -> Result<Option<Self>> {
        validate_sha256(resource_sha256, "catalog resource SHA-256")?;
        let sql = format!(
            "SELECT {RESOURCE_COLUMNS} FROM remi_catalog_resources WHERE resource_sha256 = ?1"
        );
        conn.query_row(&sql, [resource_sha256], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn find_profile_revision(
        conn: &Connection,
        source_profile: &str,
        resource_sha256: &str,
    ) -> Result<Option<Self>> {
        validate_identity(source_profile, "catalog source profile")?;
        validate_sha256(resource_sha256, "profile revision SHA-256")?;
        let sql = format!(
            "SELECT {RESOURCE_COLUMNS} FROM remi_catalog_resources
             WHERE resource_sha256 = ?1 AND resource_kind = 'profile_revision'
               AND source_profile = ?2"
        );
        conn.query_row(
            &sql,
            params![resource_sha256, source_profile],
            Self::from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn validate(&self) -> Result<()> {
        validate_sha256(&self.resource_sha256, "catalog resource SHA-256")?;
        validate_identity(&self.source_profile, "catalog source profile")?;
        validate_sha256(&self.artifact_sha256, "catalog artifact SHA-256")?;
        validate_sha256(&self.logical_digest_sha256, "catalog logical digest")?;
        if self.artifact_size < 0 {
            return Err(Error::ConfigError(
                "catalog artifact size must not be negative".to_string(),
            ));
        }
        if self.created_at < 0 {
            return Err(Error::ConfigError(
                "catalog resource creation time must not be negative".to_string(),
            ));
        }
        validate_canonical_manifest(&self.manifest_json)?;
        let manifest_sha256 = crate::hash::sha256(self.manifest_json.as_bytes());
        if manifest_sha256 != self.resource_sha256 {
            return Err(Error::ChecksumMismatch {
                expected: self.resource_sha256.clone(),
                actual: manifest_sha256,
            });
        }
        Ok(())
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            resource_sha256: row.get(0)?,
            kind: RemiCatalogResourceKind::from_db(&row.get::<_, String>(1)?, 1)?,
            source_profile: row.get(2)?,
            artifact_sha256: row.get(3)?,
            artifact_size: row.get(4)?,
            logical_digest_sha256: row.get(5)?,
            manifest_json: row.get(6)?,
            durable: row.get::<_, i64>(7)? != 0,
            created_at: row.get(8)?,
        })
    }
}

/// The canonical ordered member binding inside one profile revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiProfileRevisionMember {
    pub profile_revision_sha256: String,
    pub ordinal: i64,
    pub source_snapshot_sha256: String,
    pub source_identity: String,
    pub repository_identity: String,
    pub stream_kind: String,
    pub stream_identity: String,
    pub priority: i64,
    pub required: bool,
}

impl RemiProfileRevisionMember {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        self.validate()?;
        conn.execute(
            "INSERT INTO remi_profile_revision_members (
                 profile_revision_sha256, ordinal, source_snapshot_sha256,
                 source_identity, repository_identity, stream_kind, stream_identity,
                 priority, required
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                &self.profile_revision_sha256,
                self.ordinal,
                &self.source_snapshot_sha256,
                &self.source_identity,
                &self.repository_identity,
                &self.stream_kind,
                &self.stream_identity,
                self.priority,
                self.required as i64,
            ],
        )?;
        Ok(())
    }

    pub fn list_for_revision(
        conn: &Connection,
        profile_revision_sha256: &str,
    ) -> Result<Vec<Self>> {
        validate_sha256(profile_revision_sha256, "profile revision SHA-256")?;
        let sql = format!(
            "SELECT {MEMBER_COLUMNS} FROM remi_profile_revision_members
             WHERE profile_revision_sha256 = ?1 ORDER BY ordinal"
        );
        let mut statement = conn.prepare(&sql)?;
        let members = statement
            .query_map([profile_revision_sha256], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(members)
    }

    fn validate(&self) -> Result<()> {
        validate_sha256(&self.profile_revision_sha256, "profile revision SHA-256")?;
        validate_sha256(&self.source_snapshot_sha256, "source snapshot SHA-256")?;
        if self.ordinal < 0 {
            return Err(Error::ConfigError(
                "profile revision member ordinal must not be negative".to_string(),
            ));
        }
        validate_identity(&self.source_identity, "profile member source identity")?;
        validate_identity(
            &self.repository_identity,
            "profile member repository identity",
        )?;
        if !matches!(self.stream_kind.as_str(), "release" | "channel" | "rolling") {
            return Err(Error::ConfigError(format!(
                "profile member stream kind '{}' is unsupported",
                self.stream_kind
            )));
        }
        validate_identity(&self.stream_identity, "profile member stream identity")?;
        Ok(())
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            profile_revision_sha256: row.get(0)?,
            ordinal: row.get(1)?,
            source_snapshot_sha256: row.get(2)?,
            source_identity: row.get(3)?,
            repository_identity: row.get(4)?,
            stream_kind: row.get(5)?,
            stream_identity: row.get(6)?,
            priority: row.get(7)?,
            required: row.get::<_, i64>(8)? != 0,
        })
    }
}

/// The operational pointer visible to profile readers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiActiveProfileRevision {
    pub source_profile: String,
    pub profile_revision_sha256: String,
    pub fencing_epoch: i64,
    pub activation_run_id: String,
    pub owner_instance_uuid: String,
    pub activated_at: i64,
}

impl RemiActiveProfileRevision {
    pub fn find(conn: &Connection, source_profile: &str) -> Result<Option<Self>> {
        validate_identity(source_profile, "active source profile")?;
        let sql = format!(
            "SELECT {ACTIVE_COLUMNS} FROM remi_active_profile_revisions
             WHERE source_profile = ?1"
        );
        conn.query_row(&sql, [source_profile], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn list(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {ACTIVE_COLUMNS} FROM remi_active_profile_revisions
             ORDER BY source_profile"
        );
        let mut statement = conn.prepare(&sql)?;
        let pointers = statement
            .query_map([], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(pointers)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            source_profile: row.get(0)?,
            profile_revision_sha256: row.get(1)?,
            fencing_epoch: row.get(2)?,
            activation_run_id: row.get(3)?,
            owner_instance_uuid: row.get(4)?,
            activated_at: row.get(5)?,
        })
    }
}

/// The owner class of an exact profile-revision pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemiRevisionPinKind {
    Conversion,
    Work,
    Reader,
}

impl RemiRevisionPinKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conversion => "conversion",
            Self::Work => "work",
            Self::Reader => "reader",
        }
    }

    fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "conversion" => Ok(Self::Conversion),
            "work" => Ok(Self::Work),
            "reader" => Ok(Self::Reader),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                format!("invalid Remi revision pin kind {other}").into(),
            )),
        }
    }
}

/// One exact, durable reachability root for an immutable profile revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiProfileRevisionPin {
    pub pin_id: String,
    pub source_profile: String,
    pub profile_revision_sha256: String,
    pub owner_kind: RemiRevisionPinKind,
    pub owner_identity: String,
    pub pinned_at: i64,
}

impl RemiProfileRevisionPin {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        self.validate()?;
        conn.execute(
            "INSERT INTO remi_profile_revision_pins (
                 pin_id, source_profile, profile_revision_sha256, owner_kind,
                 owner_identity, pinned_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &self.pin_id,
                &self.source_profile,
                &self.profile_revision_sha256,
                self.owner_kind.as_str(),
                &self.owner_identity,
                self.pinned_at,
            ],
        )?;
        Ok(())
    }

    pub fn find(conn: &Connection, pin_id: &str) -> Result<Option<Self>> {
        validate_identity(pin_id, "profile revision pin ID")?;
        let sql = format!("SELECT {PIN_COLUMNS} FROM remi_profile_revision_pins WHERE pin_id = ?1");
        conn.query_row(&sql, [pin_id], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn list_for_revision(
        conn: &Connection,
        source_profile: &str,
        profile_revision_sha256: &str,
    ) -> Result<Vec<Self>> {
        validate_identity(source_profile, "pinned source profile")?;
        validate_sha256(profile_revision_sha256, "pinned profile revision SHA-256")?;
        let sql = format!(
            "SELECT {PIN_COLUMNS} FROM remi_profile_revision_pins
             WHERE source_profile = ?1 AND profile_revision_sha256 = ?2
             ORDER BY pin_id"
        );
        let mut statement = conn.prepare(&sql)?;
        let pins = statement
            .query_map(
                params![source_profile, profile_revision_sha256],
                Self::from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(pins)
    }

    pub fn release(conn: &Connection, pin_id: &str) -> Result<bool> {
        validate_identity(pin_id, "profile revision pin ID")?;
        Ok(conn.execute(
            "DELETE FROM remi_profile_revision_pins WHERE pin_id = ?1",
            [pin_id],
        )? == 1)
    }

    fn validate(&self) -> Result<()> {
        validate_identity(&self.pin_id, "profile revision pin ID")?;
        validate_identity(&self.source_profile, "pinned source profile")?;
        validate_sha256(
            &self.profile_revision_sha256,
            "pinned profile revision SHA-256",
        )?;
        validate_identity(&self.owner_identity, "profile revision pin owner")?;
        if self.pinned_at < 0 {
            return Err(Error::ConfigError(
                "profile revision pin time must not be negative".to_string(),
            ));
        }
        Ok(())
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            pin_id: row.get(0)?,
            source_profile: row.get(1)?,
            profile_revision_sha256: row.get(2)?,
            owner_kind: RemiRevisionPinKind::from_db(&row.get::<_, String>(3)?, 3)?,
            owner_identity: row.get(4)?,
            pinned_at: row.get(5)?,
        })
    }
}

/// The exact proof required before moving a profile pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiProfileRevisionActivation {
    pub source_profile: String,
    pub profile_revision_sha256: String,
    pub artifact_sha256: String,
    pub artifact_size: i64,
    pub logical_digest_sha256: String,
    pub run_id: String,
    pub owner_instance_uuid: String,
    pub fencing_epoch: i64,
}

impl RemiProfileRevisionActivation {
    fn validate(&self) -> Result<()> {
        validate_identity(&self.source_profile, "activation source profile")?;
        validate_sha256(
            &self.profile_revision_sha256,
            "activation profile revision SHA-256",
        )?;
        validate_sha256(&self.artifact_sha256, "activation catalog artifact SHA-256")?;
        validate_sha256(
            &self.logical_digest_sha256,
            "activation catalog logical digest",
        )?;
        if self.artifact_size < 0 {
            return Err(Error::ConfigError(
                "activation catalog artifact size must not be negative".to_string(),
            ));
        }
        validate_uuid(&self.run_id, "activation run ID")?;
        validate_uuid(&self.owner_instance_uuid, "activation owner instance UUID")?;
        if self.fencing_epoch <= 0 {
            return Err(Error::ConfigError(
                "activation fencing epoch must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

/// Whether the transaction moved the pointer or proved an exact replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemiProfileActivationOutcome {
    Activated(RemiActiveProfileRevision),
    AlreadyActive(RemiActiveProfileRevision),
}

/// Prove the exact current run owner and durable catalog metadata, then move
/// one profile pointer under a single immediate SQLite transaction.
pub fn activate_profile_revision(
    conn: &Connection,
    request: &RemiProfileRevisionActivation,
) -> Result<RemiProfileActivationOutcome> {
    request.validate()?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // Read the lease clock only after acquiring the writer lock. A timestamp
    // captured while waiting could already be expired when the transaction
    // finally obtains activation authority.
    let now = current_unix_seconds()?;
    let outcome = activate_profile_revision_in_transaction(&tx, request, now)?;
    tx.commit()?;
    Ok(outcome)
}

#[cfg(test)]
fn activate_profile_revision_at(
    conn: &Connection,
    request: &RemiProfileRevisionActivation,
    now: i64,
) -> Result<RemiProfileActivationOutcome> {
    request.validate()?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let outcome = activate_profile_revision_in_transaction(&tx, request, now)?;
    tx.commit()?;
    Ok(outcome)
}

fn activate_profile_revision_in_transaction(
    tx: &Transaction<'_>,
    request: &RemiProfileRevisionActivation,
    now: i64,
) -> Result<RemiProfileActivationOutcome> {
    let profile_resource = tx
        .query_row(
            &format!(
                "SELECT {RESOURCE_COLUMNS} FROM remi_catalog_resources
                 WHERE resource_sha256 = ?1
                   AND resource_kind = 'profile_revision'
                   AND source_profile = ?2"
            ),
            params![&request.profile_revision_sha256, &request.source_profile],
            RemiCatalogResource::from_row,
        )
        .optional()?;
    let Some(profile_resource) = profile_resource else {
        return Err(Error::ConflictError(format!(
            "profile {} revision {} lacks exact durable catalog metadata",
            request.source_profile, request.profile_revision_sha256
        )));
    };
    profile_resource.validate()?;
    let resource_matches = profile_resource.durable
        && profile_resource.artifact_sha256 == request.artifact_sha256
        && profile_resource.artifact_size == request.artifact_size
        && profile_resource.logical_digest_sha256 == request.logical_digest_sha256;
    if !resource_matches {
        return Err(Error::ConflictError(format!(
            "profile {} revision {} lacks exact durable catalog metadata",
            request.source_profile, request.profile_revision_sha256
        )));
    }

    let (member_count, min_ordinal, max_ordinal) = tx.query_row(
        "SELECT COUNT(*), COALESCE(MIN(ordinal), -1), COALESCE(MAX(ordinal), -1)
         FROM remi_profile_revision_members
         WHERE profile_revision_sha256 = ?1",
        params![&request.profile_revision_sha256,],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )?;
    if member_count == 0 || min_ordinal != 0 || max_ordinal != member_count - 1 {
        return Err(Error::ConflictError(format!(
            "profile {} revision {} has noncanonical ordered members",
            request.source_profile, request.profile_revision_sha256
        )));
    }

    let mut source_statement = tx.prepare(&format!(
        "SELECT source.{RESOURCE_COLUMNS}
         FROM remi_profile_revision_members member
         JOIN remi_catalog_resources source
           ON source.resource_sha256 = member.source_snapshot_sha256
         WHERE member.profile_revision_sha256 = ?1
         ORDER BY member.ordinal"
    ))?;
    let mut source_rows = source_statement.query([&request.profile_revision_sha256])?;
    let mut source_count = 0_i64;
    while let Some(row) = source_rows.next()? {
        let source = RemiCatalogResource::from_row(row)?;
        source.validate()?;
        if source.kind != RemiCatalogResourceKind::SourceSnapshot
            || source.source_profile != request.source_profile
            || !source.durable
        {
            return Err(Error::ConflictError(format!(
                "profile {} revision {} has a missing or non-durable source snapshot",
                request.source_profile, request.profile_revision_sha256
            )));
        }
        source_count += 1;
    }
    if source_count != member_count {
        return Err(Error::ConflictError(format!(
            "profile {} revision {} has a missing source snapshot member",
            request.source_profile, request.profile_revision_sha256
        )));
    }

    let owned_run = tx.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM repository_sync_runs run
             JOIN repository_sync_scopes scope
               ON scope.repository_id = run.repository_id
              AND scope.current_run_id = run.run_id
              AND scope.fencing_epoch = run.fencing_epoch
             JOIN repositories repository ON repository.id = run.repository_id
             WHERE run.run_id = ?1
               AND run.owner_instance_uuid = ?2
               AND run.fencing_epoch = ?3
               AND run.state = 'ready_to_publish'
               AND run.lease_expires_at > ?4
               AND COALESCE(run.source_profile, repository.source_profile) = ?5
         )",
        params![
            &request.run_id,
            &request.owner_instance_uuid,
            request.fencing_epoch,
            now,
            &request.source_profile,
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if !owned_run {
        return Err(Error::ConflictError(format!(
            "profile {} activation lost run {} fencing epoch {}",
            request.source_profile, request.run_id, request.fencing_epoch
        )));
    }

    let current = RemiActiveProfileRevision::find_in_transaction(tx, &request.source_profile)?;
    if let Some(current) = current {
        if current.fencing_epoch > request.fencing_epoch {
            return Err(Error::ConflictError(format!(
                "profile {} activation fencing epoch {} is stale behind {}",
                request.source_profile, request.fencing_epoch, current.fencing_epoch
            )));
        }
        if current.fencing_epoch == request.fencing_epoch {
            if current.profile_revision_sha256 == request.profile_revision_sha256
                && current.activation_run_id == request.run_id
                && current.owner_instance_uuid == request.owner_instance_uuid
            {
                return Ok(RemiProfileActivationOutcome::AlreadyActive(current));
            }
            return Err(Error::ConflictError(format!(
                "profile {} activation replay conflicts at fencing epoch {}",
                request.source_profile, request.fencing_epoch
            )));
        }
    }

    let updated = tx.execute(
        "INSERT INTO remi_active_profile_revisions (
             source_profile, profile_revision_sha256, fencing_epoch,
             activation_run_id, owner_instance_uuid, activated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(source_profile) DO UPDATE SET
             profile_revision_sha256 = excluded.profile_revision_sha256,
             fencing_epoch = excluded.fencing_epoch,
             activation_run_id = excluded.activation_run_id,
             owner_instance_uuid = excluded.owner_instance_uuid,
             activated_at = excluded.activated_at
         WHERE excluded.fencing_epoch > remi_active_profile_revisions.fencing_epoch",
        params![
            &request.source_profile,
            &request.profile_revision_sha256,
            request.fencing_epoch,
            &request.run_id,
            &request.owner_instance_uuid,
            now,
        ],
    )?;
    if updated != 1 {
        return Err(Error::ConflictError(format!(
            "profile {} activation was rejected by monotonic fencing",
            request.source_profile
        )));
    }
    let active = RemiActiveProfileRevision::find_in_transaction(tx, &request.source_profile)?
        .ok_or_else(|| Error::InternalError("activated profile pointer disappeared".to_string()))?;
    Ok(RemiProfileActivationOutcome::Activated(active))
}

impl RemiActiveProfileRevision {
    fn find_in_transaction(tx: &Transaction<'_>, source_profile: &str) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {ACTIVE_COLUMNS} FROM remi_active_profile_revisions
             WHERE source_profile = ?1"
        );
        tx.query_row(&sql, [source_profile], Self::from_row)
            .optional()
            .map_err(Into::into)
    }
}

fn current_unix_seconds() -> Result<i64> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::InternalError(format!("system time precedes Unix epoch: {error}")))?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|_| Error::InternalError("system time exceeds SQLite integer range".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::Repository;
    use crate::db::schema::ensure_current;

    const OWNER_ONE: &str = "00000000-0000-4000-8000-000000000001";
    const OWNER_TWO: &str = "00000000-0000-4000-8000-000000000002";
    const RUN_ONE: &str = "10000000-0000-4000-8000-000000000001";
    const RUN_TWO: &str = "10000000-0000-4000-8000-000000000002";

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn resource_digest(byte: char) -> String {
        crate::hash::sha256(format!("{{\"resource\":\"{byte}\"}}").as_bytes())
    }

    fn resource(
        sha: char,
        artifact: char,
        kind: RemiCatalogResourceKind,
        durable: bool,
    ) -> RemiCatalogResource {
        RemiCatalogResource {
            resource_sha256: resource_digest(sha),
            kind,
            source_profile: "fedora-44".to_string(),
            artifact_sha256: digest(artifact),
            artifact_size: 4096,
            logical_digest_sha256: digest('c'),
            manifest_json: format!("{{\"resource\":\"{sha}\"}}"),
            durable,
            created_at: 100,
        }
    }

    fn member(profile: char, source: char, ordinal: i64) -> RemiProfileRevisionMember {
        RemiProfileRevisionMember {
            profile_revision_sha256: resource_digest(profile),
            ordinal,
            source_snapshot_sha256: resource_digest(source),
            source_identity: format!("source-{source}"),
            repository_identity: format!("repository-{source}"),
            stream_kind: "release".to_string(),
            stream_identity: "fixture".to_string(),
            priority: ordinal,
            required: true,
        }
    }

    fn setup() -> (Connection, Repository) {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let mut repo = Repository::new(
            "fixture-repository".to_string(),
            "https://fixture.test".to_string(),
        );
        repo.source_profile = Some("fedora-44".to_string());
        repo.id = Some(repo.insert(&conn).unwrap());
        (conn, repo)
    }

    fn insert_run(conn: &Connection, repo: &Repository, run_id: &str, owner: &str, epoch: i64) {
        let repository_id = repo.id.unwrap();
        conn.execute(
            "INSERT INTO repository_sync_runs (
                 run_id, repository_id, scope_kind, scope_identity, source_profile,
                 owner_instance_uuid, fencing_epoch, staging_repository_id, state,
                 started_at, heartbeat_at, lease_expires_at
             ) VALUES (?1, ?2, 'repository', ?2, ?3, ?4, ?5, ?2, 'ready_to_publish', 100, 100, 1000)",
            params![run_id, repository_id, repo.source_profile.as_deref(), owner, epoch],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repository_sync_scopes (
                 repository_id, fencing_epoch, current_run_id, active_revision
             ) VALUES (?1, ?2, ?3, NULL)
             ON CONFLICT(repository_id) DO UPDATE SET
                 fencing_epoch = excluded.fencing_epoch,
                 current_run_id = excluded.current_run_id,
                 active_revision = excluded.active_revision",
            params![repository_id, epoch, run_id],
        )
        .unwrap();
    }

    fn activation(
        epoch: i64,
        run_id: &str,
        owner: &str,
        profile: char,
    ) -> RemiProfileRevisionActivation {
        RemiProfileRevisionActivation {
            source_profile: "fedora-44".to_string(),
            profile_revision_sha256: resource_digest(profile),
            artifact_sha256: digest(if profile == 'd' { 'b' } else { 'e' }),
            artifact_size: 4096,
            logical_digest_sha256: digest('c'),
            run_id: run_id.to_string(),
            owner_instance_uuid: owner.to_string(),
            fencing_epoch: epoch,
        }
    }

    fn install_catalog(conn: &Connection, profile: char, source: char, durable: bool) {
        resource(
            source,
            source,
            RemiCatalogResourceKind::SourceSnapshot,
            durable,
        )
        .insert(conn)
        .unwrap();
        resource(
            profile,
            if profile == 'd' { 'b' } else { 'e' },
            RemiCatalogResourceKind::ProfileRevision,
            durable,
        )
        .insert(conn)
        .unwrap();
        member(profile, source, 0).insert(conn).unwrap();
    }

    #[test]
    fn activation_requires_exact_owner_and_durable_resources() {
        let (conn, repo) = setup();
        insert_run(&conn, &repo, RUN_ONE, OWNER_ONE, 1);
        install_catalog(&conn, 'd', 'e', false);

        let error =
            activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200)
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("lacks exact durable catalog metadata")
        );
        assert!(
            RemiActiveProfileRevision::find(&conn, "fedora-44")
                .unwrap()
                .is_none()
        );

        // Durability is immutable metadata: replacing the resource is the
        // only valid way to register a post-fsync artifact.
        conn.execute(
            "DELETE FROM remi_profile_revision_members WHERE profile_revision_sha256 = ?1",
            [resource_digest('d')],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM remi_catalog_resources WHERE resource_sha256 IN (?1, ?2)",
            params![resource_digest('d'), resource_digest('e')],
        )
        .unwrap();
        install_catalog(&conn, 'd', 'e', true);

        let wrong_owner =
            activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_TWO, 'd'), 200)
                .unwrap_err();
        assert!(wrong_owner.to_string().contains("lost run"));
        assert!(
            RemiActiveProfileRevision::find(&conn, "fedora-44")
                .unwrap()
                .is_none()
        );

        let outcome =
            activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200)
                .unwrap();
        assert!(matches!(
            outcome,
            RemiProfileActivationOutcome::Activated(_)
        ));
    }

    #[test]
    fn stale_and_replayed_activation_leave_pointer_unchanged() {
        let (conn, repo) = setup();
        insert_run(&conn, &repo, RUN_ONE, OWNER_ONE, 1);
        install_catalog(&conn, 'd', 'e', true);
        let first =
            activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200)
                .unwrap();
        let first = match first {
            RemiProfileActivationOutcome::Activated(value) => value,
            RemiProfileActivationOutcome::AlreadyActive(_) => panic!("first activation replayed"),
        };

        let replay =
            activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200)
                .unwrap();
        assert!(matches!(
            replay,
            RemiProfileActivationOutcome::AlreadyActive(_)
        ));
        assert_eq!(
            RemiActiveProfileRevision::find(&conn, "fedora-44").unwrap(),
            Some(first.clone())
        );

        insert_run(&conn, &repo, RUN_TWO, OWNER_TWO, 2);
        install_catalog(&conn, 'f', '0', true);
        let second =
            activate_profile_revision_at(&conn, &activation(2, RUN_TWO, OWNER_TWO, 'f'), 200)
                .unwrap();
        assert!(matches!(second, RemiProfileActivationOutcome::Activated(_)));
        let after_second = RemiActiveProfileRevision::find(&conn, "fedora-44")
            .unwrap()
            .unwrap();
        assert_eq!(after_second.fencing_epoch, 2);

        let stale =
            activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200)
                .unwrap_err();
        assert!(stale.to_string().contains("lost run"));
        assert_eq!(
            RemiActiveProfileRevision::find(&conn, "fedora-44").unwrap(),
            Some(after_second)
        );
    }

    #[test]
    fn pins_are_exact_durable_roots_and_cannot_be_repointed() {
        let (conn, _repo) = setup();
        install_catalog(&conn, 'd', 'e', true);
        let pin = RemiProfileRevisionPin {
            pin_id: "conversion-1".to_string(),
            source_profile: "fedora-44".to_string(),
            profile_revision_sha256: resource_digest('d'),
            owner_kind: RemiRevisionPinKind::Conversion,
            owner_identity: "conversion-work-1".to_string(),
            pinned_at: 200,
        };
        pin.insert(&conn).unwrap();
        assert_eq!(
            RemiProfileRevisionPin::find(&conn, "conversion-1").unwrap(),
            Some(pin.clone())
        );
        assert_eq!(
            RemiProfileRevisionPin::list_for_revision(&conn, "fedora-44", &resource_digest('d'),)
                .unwrap(),
            vec![pin]
        );

        let error = conn
            .execute(
                "UPDATE remi_profile_revision_pins
                 SET profile_revision_sha256 = ?1 WHERE pin_id = 'conversion-1'",
                [digest('f')],
            )
            .unwrap_err();
        assert!(error.to_string().contains("cannot be repointed"));
        assert!(RemiProfileRevisionPin::release(&conn, "conversion-1").unwrap());
        assert!(!RemiProfileRevisionPin::release(&conn, "conversion-1").unwrap());
    }

    #[test]
    fn members_are_returned_in_declared_ordinal_order() {
        let (conn, _repo) = setup();
        resource('b', 'b', RemiCatalogResourceKind::SourceSnapshot, true)
            .insert(&conn)
            .unwrap();
        resource('c', 'c', RemiCatalogResourceKind::SourceSnapshot, true)
            .insert(&conn)
            .unwrap();
        resource('a', 'b', RemiCatalogResourceKind::ProfileRevision, true)
            .insert(&conn)
            .unwrap();
        member('a', 'c', 1).insert(&conn).unwrap();
        member('a', 'b', 0).insert(&conn).unwrap();
        let members =
            RemiProfileRevisionMember::list_for_revision(&conn, &resource_digest('a')).unwrap();
        assert_eq!(
            members
                .iter()
                .map(|member| member.ordinal)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(members[0].source_snapshot_sha256, resource_digest('b'));
        assert_eq!(members[1].source_snapshot_sha256, resource_digest('c'));
    }
}
