// conary-core/src/db/models/lifecycle_event.rs

//! Persisted per-changeset evidence for lifecycle failures the source format
//! allowed the transaction to continue past.

use crate::error::{Error, Result, ScriptletFailureKind};
use crate::scriptlet::{EffectiveSandbox, SandboxMode, ScriptletFailureOutcome};
use rusqlite::{Connection, OptionalExtension, Row, params};

/// Typed lifecycle-failure evidence ready to append to one pending changeset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLifecycleEvent {
    pub source_package: String,
    pub source_version: String,
    pub source_entry: String,
    pub failure: ScriptletFailureOutcome,
}

impl NewLifecycleEvent {
    fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("source_package", self.source_package.as_str()),
            ("source_version", self.source_version.as_str()),
            ("source_entry", self.source_entry.as_str()),
        ] {
            if value.is_empty() {
                return Err(Error::InternalError(format!(
                    "lifecycle event {field} is empty"
                )));
            }
        }
        Ok(())
    }
}

/// One typed continued-lifecycle-failure event attached to a changeset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleEvent {
    pub id: i64,
    pub changeset_id: i64,
    pub sequence: i64,
    pub source_package: String,
    pub source_version: String,
    pub source_entry: String,
    pub failure_kind: ScriptletFailureKind,
    pub requested_sandbox_mode: SandboxMode,
    pub effective_sandbox: EffectiveSandbox,
    pub phase: String,
    pub message: String,
    pub created_at: String,
}

impl LifecycleEvent {
    const COLUMNS: &'static str = "id, changeset_id, sequence, source_package, \
        source_version, source_entry, failure_kind, requested_sandbox_mode, \
        effective_sandbox, phase, message, created_at";

    /// Append typed lifecycle failures to a still-pending changeset.
    pub fn append_batch(
        conn: &Connection,
        changeset_id: i64,
        events: &[NewLifecycleEvent],
    ) -> Result<Vec<i64>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        // This pending-only guard is coupled to the four production callers
        // inserting the changeset immediately before driving the graph. A
        // future caller that drives after update_status(Applied) will reject
        // evidence here; native_graph treats that as evidence loss and warns
        // without changing the graph's posture.
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM changesets WHERE id = ?1",
                [changeset_id],
                |row| row.get(0),
            )
            .optional()?;
        match status.as_deref() {
            Some(status) if status == super::changeset::ChangesetStatus::Pending.as_str() => {}
            Some(other) => {
                return Err(Error::ConflictError(format!(
                    "lifecycle events cannot be appended to changeset {changeset_id} in status '{other}'"
                )));
            }
            None => {
                return Err(Error::NotFound(format!(
                    "lifecycle event changeset {changeset_id}"
                )));
            }
        }

        let next_sequence: i64 = conn.query_row(
            "SELECT COALESCE(MAX(sequence), -1) + 1
             FROM lifecycle_events
             WHERE changeset_id = ?1",
            [changeset_id],
            |row| row.get(0),
        )?;

        // Validate the whole batch before the first insert: a mid-batch
        // validation failure must not commit a prefix of the evidence while
        // the caller's warn-and-continue handling drops the rest.
        for event in events {
            event.validate()?;
        }

        let mut ids = Vec::with_capacity(events.len());
        for (sequence, event) in (next_sequence..).zip(events) {
            conn.execute(
                "INSERT INTO lifecycle_events (
                    changeset_id, sequence, source_package, source_version, source_entry,
                    failure_kind, requested_sandbox_mode, effective_sandbox, phase, message
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    changeset_id,
                    sequence,
                    &event.source_package,
                    &event.source_version,
                    &event.source_entry,
                    event.failure.failure_kind.as_str(),
                    event.failure.requested_sandbox_mode.as_str(),
                    event.failure.effective_sandbox.as_str(),
                    &event.failure.phase,
                    &event.failure.message,
                ],
            )?;
            ids.push(conn.last_insert_rowid());
        }
        Ok(ids)
    }

    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM lifecycle_events WHERE id = ?1",
            Self::COLUMNS
        );
        conn.prepare(&sql)?
            .query_row([id], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    /// Return events in the exact order the native graph observed them.
    pub fn list_for_changeset(conn: &Connection, changeset_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM lifecycle_events
             WHERE changeset_id = ?1
             ORDER BY sequence, id",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([changeset_id], Self::from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            changeset_id: row.get(1)?,
            sequence: row.get(2)?,
            source_package: row.get(3)?,
            source_version: row.get(4)?,
            source_entry: row.get(5)?,
            failure_kind: parse_failure_kind(row.get(6)?, 6)?,
            requested_sandbox_mode: parse_sandbox_mode(row.get(7)?, 7)?,
            effective_sandbox: parse_effective_sandbox(row.get(8)?, 8)?,
            phase: row.get(9)?,
            message: row.get(10)?,
            created_at: row.get(11)?,
        })
    }
}

fn parse_failure_kind(value: String, column: usize) -> rusqlite::Result<ScriptletFailureKind> {
    let parsed = match value.as_str() {
        "ScriptExited" => ScriptletFailureKind::ScriptExited,
        "ScriptTimedOut" => ScriptletFailureKind::ScriptTimedOut,
        "ContractViolation" => ScriptletFailureKind::ContractViolation,
        "ProgramUnavailable" => ScriptletFailureKind::ProgramUnavailable,
        "ProcessSetupFailed" => ScriptletFailureKind::ProcessSetupFailed,
        "SandboxSetupUnavailable" => ScriptletFailureKind::SandboxSetupUnavailable,
        "EnforcementSetupFailed" => ScriptletFailureKind::EnforcementSetupFailed,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(Error::InternalError(format!(
                    "persisted lifecycle event has unknown failure kind '{value}'"
                ))),
            ));
        }
    };
    Ok(parsed)
}

fn parse_sandbox_mode(value: String, column: usize) -> rusqlite::Result<SandboxMode> {
    match value.as_str() {
        "always" => Ok(SandboxMode::Always),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(Error::InternalError(format!(
                "persisted lifecycle event has unknown requested sandbox mode '{value}'"
            ))),
        )),
    }
}

fn parse_effective_sandbox(value: String, column: usize) -> rusqlite::Result<EffectiveSandbox> {
    match value.as_str() {
        "target-root" => Ok(EffectiveSandbox::TargetRoot),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(Error::InternalError(format!(
                "persisted lifecycle event has unknown effective sandbox '{value}'"
            ))),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{Changeset, ChangesetStatus};
    use crate::db::testing::create_test_db;

    fn pending_changeset(conn: &Connection) -> i64 {
        Changeset::new("Install lifecycle fixture".to_string())
            .insert(conn)
            .expect("insert pending changeset")
    }

    fn event() -> NewLifecycleEvent {
        NewLifecycleEvent {
            source_package: "fixture".to_string(),
            source_version: "1.0.0".to_string(),
            source_entry: "rpm:%post".to_string(),
            failure: ScriptletFailureOutcome {
                phase: "post-install".to_string(),
                failure_kind: ScriptletFailureKind::ScriptExited,
                requested_sandbox_mode: SandboxMode::Always,
                effective_sandbox: EffectiveSandbox::TargetRoot,
                message: "script returned 42: stderr excerpt".to_string(),
            },
        }
    }

    #[test]
    fn lifecycle_event_round_trips_all_typed_failure_fields() {
        let (_temp, conn) = create_test_db();
        let changeset_id = pending_changeset(&conn);

        let ids = LifecycleEvent::append_batch(&conn, changeset_id, &[event()]).unwrap();
        assert_eq!(ids.len(), 1);

        let events = LifecycleEvent::list_for_changeset(&conn, changeset_id).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, ids[0]);
        assert_eq!(events[0].changeset_id, changeset_id);
        assert_eq!(events[0].sequence, 0);
        assert_eq!(events[0].source_package, "fixture");
        assert_eq!(events[0].source_version, "1.0.0");
        assert_eq!(events[0].source_entry, "rpm:%post");
        assert_eq!(events[0].failure_kind, ScriptletFailureKind::ScriptExited);
        assert_eq!(events[0].requested_sandbox_mode, SandboxMode::Always);
        assert_eq!(events[0].effective_sandbox, EffectiveSandbox::TargetRoot);
        assert_eq!(events[0].phase, "post-install");
        assert_eq!(events[0].message, "script returned 42: stderr excerpt");
        assert!(!events[0].created_at.is_empty());
    }

    #[test]
    fn a_mid_batch_validation_failure_persists_nothing() {
        let (_temp, conn) = create_test_db();
        let changeset_id = pending_changeset(&conn);

        let mut invalid = event();
        invalid.source_entry = String::new();
        // Two valid events ahead of the invalid one: the batch must be
        // all-or-nothing, not a committed prefix with a dropped tail.
        let error = LifecycleEvent::append_batch(&conn, changeset_id, &[event(), event(), invalid])
            .unwrap_err();
        assert!(
            error.to_string().contains("source_entry is empty"),
            "unexpected error: {error}"
        );
        assert!(
            LifecycleEvent::list_for_changeset(&conn, changeset_id)
                .unwrap()
                .is_empty(),
            "a rejected batch must not commit a prefix"
        );
    }

    #[test]
    fn lifecycle_events_cascade_with_their_changeset() {
        let (_temp, conn) = create_test_db();
        let changeset_id = pending_changeset(&conn);
        LifecycleEvent::append_batch(&conn, changeset_id, &[event()]).unwrap();
        conn.execute(
            "UPDATE changesets SET status = ?1 WHERE id = ?2",
            params![ChangesetStatus::Applied.as_str(), changeset_id],
        )
        .unwrap();

        conn.execute("DELETE FROM changesets WHERE id = ?1", [changeset_id])
            .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_events WHERE changeset_id = ?1",
                [changeset_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
