// crates/conary-core/src/db/models/remi_catalog/session.rs

//! Durable ownership of the one Remi runtime session for this database.

use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::validation::validate_uuid;

const SESSION_SLOT: i64 = 1;

/// The exact server session that may create durable Remi reader pins.
///
/// The current-only schema intentionally permits one row only. A server first
/// acquires the process-wide runtime lock, then replaces this row in an
/// immediate transaction. Reader pins belonging to the previous row are
/// removed in that same transaction; conversion and work pins are untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiRuntimeSession {
    pub session_id: String,
    pub started_at: i64,
}

impl RemiRuntimeSession {
    /// Start a new exact runtime session and recover only the prior session's
    /// reader pins.
    pub fn begin(conn: &Connection, started_at: i64) -> Result<Self> {
        let session = Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            started_at,
        };
        session.validate()?;

        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        let prior_session_id = current_from_transaction(&tx)?.map(|prior| prior.session_id);
        if let Some(prior_session_id) = prior_session_id {
            tx.execute(
                "DELETE FROM remi_profile_revision_pins
                 WHERE owner_kind = 'reader' AND runtime_session_id = ?1",
                [&prior_session_id],
            )?;
        }
        let unknown_reader_pin = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM remi_profile_revision_pins
                 WHERE owner_kind = 'reader'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if unknown_reader_pin {
            return Err(Error::ConflictError(
                "reader pin is not owned by the prior durable Remi runtime session".to_string(),
            ));
        }
        tx.execute(
            "INSERT INTO remi_runtime_sessions (session_slot, session_id, started_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(session_slot) DO UPDATE SET
                 session_id = excluded.session_id,
                 started_at = excluded.started_at",
            rusqlite::params![SESSION_SLOT, &session.session_id, session.started_at],
        )?;
        tx.commit()?;
        Ok(session)
    }

    /// Return the currently installed session, if server startup has installed
    /// one. No timestamp or process-liveness heuristic participates here.
    pub fn current(conn: &Connection) -> Result<Option<Self>> {
        current_from_connection(conn)
    }

    fn validate(&self) -> Result<()> {
        validate_uuid(&self.session_id, "Remi runtime session ID")?;
        if self.started_at < 0 {
            return Err(Error::ConfigError(
                "Remi runtime session start time must not be negative".to_string(),
            ));
        }
        Ok(())
    }
}

fn current_from_connection(conn: &Connection) -> Result<Option<RemiRuntimeSession>> {
    let row = conn
        .query_row(
            "SELECT session_id, started_at
             FROM remi_runtime_sessions
             WHERE session_slot = ?1",
            [SESSION_SLOT],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    row.map(|(session_id, started_at)| {
        let session = RemiRuntimeSession {
            session_id,
            started_at,
        };
        session.validate()?;
        Ok(session)
    })
    .transpose()
}

fn current_from_transaction(tx: &Transaction<'_>) -> Result<Option<RemiRuntimeSession>> {
    current_from_connection(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{
        RemiCatalogResource, RemiCatalogResourceKind, RemiProfileRevisionPin, RemiRevisionPinKind,
    };
    use crate::db::schema::ensure_current;
    use rusqlite::params;

    fn profile_resource(conn: &Connection) -> String {
        let manifest_json = r#"{"resource":"runtime-session-test"}"#;
        let resource_sha256 = crate::hash::sha256(manifest_json.as_bytes());
        RemiCatalogResource {
            resource_sha256: resource_sha256.clone(),
            kind: RemiCatalogResourceKind::ProfileRevision,
            source_profile: "test-profile".to_string(),
            artifact_sha256: "a".repeat(64),
            artifact_size: 1,
            logical_digest_sha256: "b".repeat(64),
            manifest_json: manifest_json.to_string(),
            durable: true,
            created_at: 1,
        }
        .insert(conn)
        .unwrap();
        resource_sha256
    }

    fn pin(
        resource_sha256: &str,
        pin_id: &str,
        owner_kind: RemiRevisionPinKind,
        owner_identity: &str,
        runtime_session_id: Option<String>,
    ) -> RemiProfileRevisionPin {
        RemiProfileRevisionPin {
            pin_id: pin_id.to_string(),
            source_profile: "test-profile".to_string(),
            profile_revision_sha256: resource_sha256.to_string(),
            owner_kind,
            owner_identity: owner_identity.to_string(),
            runtime_session_id,
            pinned_at: 1,
        }
    }

    #[test]
    fn beginning_a_session_removes_only_the_prior_session_reader_pins() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let resource_sha256 = profile_resource(&conn);

        let first = RemiRuntimeSession::begin(&conn, 10).unwrap();
        pin(
            &resource_sha256,
            "reader-pin",
            RemiRevisionPinKind::Reader,
            "reader-owner",
            Some(first.session_id.clone()),
        )
        .insert(&conn)
        .unwrap();
        pin(
            &resource_sha256,
            "conversion-pin",
            RemiRevisionPinKind::Conversion,
            "conversion-owner",
            None,
        )
        .insert(&conn)
        .unwrap();
        pin(
            &resource_sha256,
            "work-pin",
            RemiRevisionPinKind::Work,
            "work-owner",
            None,
        )
        .insert(&conn)
        .unwrap();

        let second = RemiRuntimeSession::begin(&conn, 20).unwrap();

        assert_ne!(first.session_id, second.session_id);
        assert_eq!(RemiRuntimeSession::current(&conn).unwrap(), Some(second));
        assert!(
            RemiProfileRevisionPin::find(&conn, "reader-pin")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            RemiProfileRevisionPin::find(&conn, "conversion-pin")
                .unwrap()
                .unwrap()
                .owner_kind,
            RemiRevisionPinKind::Conversion
        );
        assert_eq!(
            RemiProfileRevisionPin::find(&conn, "work-pin")
                .unwrap()
                .unwrap()
                .owner_kind,
            RemiRevisionPinKind::Work
        );
    }

    #[test]
    fn pin_session_ownership_is_strict_for_reader_and_non_reader_kinds() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let resource_sha256 = profile_resource(&conn);
        let session = RemiRuntimeSession::begin(&conn, 10).unwrap();

        let missing_reader_session = pin(
            &resource_sha256,
            "reader-without-session",
            RemiRevisionPinKind::Reader,
            "reader-owner",
            None,
        )
        .insert(&conn)
        .unwrap_err();
        assert!(
            missing_reader_session
                .to_string()
                .contains("reader pins require a runtime session ID")
        );

        let non_reader_session = pin(
            &resource_sha256,
            "work-with-session",
            RemiRevisionPinKind::Work,
            "work-owner",
            Some(session.session_id.clone()),
        )
        .insert(&conn)
        .unwrap_err();
        assert!(
            non_reader_session
                .to_string()
                .contains("non-reader pins must not carry a runtime session ID")
        );

        let direct_reader_without_session = conn.execute(
            "INSERT INTO remi_profile_revision_pins (
                 pin_id, source_profile, profile_revision_sha256, owner_kind,
                 owner_identity, runtime_session_id, pinned_at
             ) VALUES (?1, 'test-profile', ?2, 'reader', 'direct-reader', NULL, 1)",
            params!["direct-reader-pin", &resource_sha256],
        );
        assert!(direct_reader_without_session.is_err());

        let direct_non_reader_with_session = conn.execute(
            "INSERT INTO remi_profile_revision_pins (
                 pin_id, source_profile, profile_revision_sha256, owner_kind,
                 owner_identity, runtime_session_id, pinned_at
             ) VALUES (?1, 'test-profile', ?2, 'work', 'direct-work', ?3, 1)",
            params!["direct-work-pin", &resource_sha256, &session.session_id],
        );
        assert!(direct_non_reader_with_session.is_err());
    }

    #[test]
    fn current_session_rejects_invalid_durable_identity() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        conn.execute(
            "INSERT INTO remi_runtime_sessions (session_slot, session_id, started_at)
             VALUES (1, 'not-a-uuid', 1)",
            [],
        )
        .unwrap_err();
    }

    #[test]
    fn session_replacement_fails_closed_on_an_unknown_reader_pin() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let resource_sha256 = profile_resource(&conn);
        let current = RemiRuntimeSession::begin(&conn, 10).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute(
            "INSERT INTO remi_profile_revision_pins (
                 pin_id, source_profile, profile_revision_sha256, owner_kind,
                 owner_identity, runtime_session_id, pinned_at
             ) VALUES ('unknown-reader', 'test-profile', ?1, 'reader',
                       'unknown-owner', ?2, 1)",
            rusqlite::params![&resource_sha256, "00000000-0000-4000-8000-000000000099",],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        let error = RemiRuntimeSession::begin(&conn, 20).unwrap_err();

        assert!(error.to_string().contains("not owned by the prior durable"));
        assert_eq!(RemiRuntimeSession::current(&conn).unwrap(), Some(current));
        assert!(
            RemiProfileRevisionPin::find(&conn, "unknown-reader")
                .unwrap()
                .is_some()
        );
    }
}
