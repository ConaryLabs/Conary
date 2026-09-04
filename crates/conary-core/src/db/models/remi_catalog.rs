// crates/conary-core/src/db/models/remi_catalog.rs

//! Operational metadata for immutable Remi source and profile catalogs.
//!
//! The package catalog itself lives in standalone, read-only SQLite files.
//! These models hold only the exact resource identities needed to activate
//! and retain those files. In particular, no package, provide, or requirement
//! row belongs here.

mod activation;
mod gc;
mod resource;
mod session;
mod validation;
use crate::error::{Error, Result};
use crate::repository::supported_profiles::ProfileSourceRole;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::io;

use validation::{validate_identity, validate_sha256};

#[cfg(test)]
use activation::activate_profile_revision_at;
pub use activation::{
    RemiProfileActivationOutcome, RemiProfileRevisionActivation,
    publish_profile_candidate_in_transaction, verify_private_profile_candidate_authority,
};
pub use gc::{
    RemiCatalogCollectionPlan, RemiCatalogCollectionResult, RemiCatalogDeletionIntent,
    RemiCatalogReachabilitySnapshot, RemiCatalogRunCandidate, acknowledge_catalog_deletion,
    delete_catalog_collection, list_catalog_deletion_intents, plan_catalog_collection,
};
use resource::RESOURCE_COLUMNS;
pub use resource::{
    RemiCatalogPhysicalAttestation, RemiCatalogResource, RemiCatalogResourceKind,
    register_profile_catalog_revision,
};
pub use session::RemiRuntimeSession;

const MEMBER_COLUMNS: &str = "profile_revision_sha256, ordinal, source_snapshot_sha256, \
    source_identity, repository_identity, stream_kind, stream_identity, role, precedence, required";
const ACTIVE_COLUMNS: &str = "source_profile, profile_revision_sha256, fencing_epoch, \
    activation_run_id, owner_instance_uuid, activated_at";
const PIN_COLUMNS: &str = "pin_id, source_profile, profile_revision_sha256, owner_kind, \
    owner_identity, runtime_session_id, pinned_at";

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
    pub role: ProfileSourceRole,
    pub precedence: i64,
    pub required: bool,
}

impl RemiProfileRevisionMember {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        self.validate()?;
        conn.execute(
            "INSERT INTO remi_profile_revision_members (
                 profile_revision_sha256, ordinal, source_snapshot_sha256,
                 source_identity, repository_identity, stream_kind, stream_identity,
                 role, precedence, required
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &self.profile_revision_sha256,
                self.ordinal,
                &self.source_snapshot_sha256,
                &self.source_identity,
                &self.repository_identity,
                &self.stream_kind,
                &self.stream_identity,
                self.role.as_str(),
                self.precedence,
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
        let role = ProfileSourceRole::parse(&row.get::<_, String>(7)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Text,
                Box::new(io::Error::new(io::ErrorKind::InvalidData, error)),
            )
        })?;
        Ok(Self {
            profile_revision_sha256: row.get(0)?,
            ordinal: row.get(1)?,
            source_snapshot_sha256: row.get(2)?,
            source_identity: row.get(3)?,
            repository_identity: row.get(4)?,
            stream_kind: row.get(5)?,
            stream_identity: row.get(6)?,
            role,
            precedence: row.get(8)?,
            required: row.get::<_, i64>(9)? != 0,
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

    /// Retire the exact active pointer for one source profile.
    ///
    /// Immutable resources and pins remain intact; readers that already hold
    /// the old revision may finish while new readers fail closed until the
    /// profile is refreshed and activated again.
    pub fn retire(conn: &Connection, source_profile: &str) -> Result<bool> {
        validate_identity(source_profile, "active source profile")?;
        Ok(conn.execute(
            "DELETE FROM remi_active_profile_revisions WHERE source_profile = ?1",
            [source_profile],
        )? == 1)
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
    pub runtime_session_id: Option<String>,
    pub pinned_at: i64,
}

impl RemiProfileRevisionPin {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        self.validate()?;
        conn.execute(
            "INSERT INTO remi_profile_revision_pins (
                 pin_id, source_profile, profile_revision_sha256, owner_kind,
                 owner_identity, runtime_session_id, pinned_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &self.pin_id,
                &self.source_profile,
                &self.profile_revision_sha256,
                self.owner_kind.as_str(),
                &self.owner_identity,
                &self.runtime_session_id,
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
        match (self.owner_kind, self.runtime_session_id.as_deref()) {
            (RemiRevisionPinKind::Reader, Some(session_id)) => {
                validation::validate_uuid(session_id, "Remi reader pin runtime session ID")?;
            }
            (RemiRevisionPinKind::Reader, None) => {
                return Err(Error::ConfigError(
                    "reader pins require a runtime session ID".to_string(),
                ));
            }
            (RemiRevisionPinKind::Conversion | RemiRevisionPinKind::Work, Some(_)) => {
                return Err(Error::ConfigError(
                    "non-reader pins must not carry a runtime session ID".to_string(),
                ));
            }
            (RemiRevisionPinKind::Conversion | RemiRevisionPinKind::Work, None) => {}
        }
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
            runtime_session_id: row.get(5)?,
            pinned_at: row.get(6)?,
        })
    }
}

#[cfg(test)]
mod tests;
