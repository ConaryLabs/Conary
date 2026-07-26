// conary-core/src/db/models/generation_activation.rs

//! Durable runtime activation work owned by immutable generations.
//!
//! Package transactions append exact typed requests to their changeset. Every
//! subsequently built generation projects all unapplied requests through its
//! changeset high-water mark, so skipping an unbooted generation cannot lose
//! runtime work.

use crate::activation::{RuntimeActivationInvocation, SecurityPolicyActivationInvocation};
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationRequestSourceKind {
    CapturedSystemctl,
    CcsService,
    CapturedSelinux,
    CapturedApparmor,
}

impl ActivationRequestSourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CapturedSystemctl => "captured-systemctl",
            Self::CcsService => "ccs-service",
            Self::CapturedSelinux => "captured-selinux",
            Self::CapturedApparmor => "captured-apparmor",
        }
    }

    pub fn captured_for(invocation: &RuntimeActivationInvocation) -> Self {
        match invocation {
            RuntimeActivationInvocation::Systemd(_) => Self::CapturedSystemctl,
            RuntimeActivationInvocation::SecurityPolicy(
                SecurityPolicyActivationInvocation::Selinux(_),
            ) => Self::CapturedSelinux,
            RuntimeActivationInvocation::SecurityPolicy(
                SecurityPolicyActivationInvocation::Apparmor(_),
            ) => Self::CapturedApparmor,
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "captured-systemctl" => Ok(Self::CapturedSystemctl),
            "ccs-service" => Ok(Self::CcsService),
            "captured-selinux" => Ok(Self::CapturedSelinux),
            "captured-apparmor" => Ok(Self::CapturedApparmor),
            _ => Err(Error::InternalError(format!(
                "persisted activation request has unknown source kind '{value}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewActivationRequest {
    pub source_kind: ActivationRequestSourceKind,
    pub source_package: String,
    pub source_version: String,
    pub source_entry: String,
    pub invocation: RuntimeActivationInvocation,
}

impl NewActivationRequest {
    pub fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("source_package", self.source_package.as_str()),
            ("source_version", self.source_version.as_str()),
            ("source_entry", self.source_entry.as_str()),
        ] {
            if value.is_empty() {
                return Err(Error::InternalError(format!(
                    "activation request {field} is empty"
                )));
            }
        }
        self.invocation.validate().map_err(Error::ParseError)?;
        let compatible = matches!(
            (&self.source_kind, &self.invocation),
            (
                ActivationRequestSourceKind::CapturedSystemctl
                    | ActivationRequestSourceKind::CcsService,
                RuntimeActivationInvocation::Systemd(_)
            ) | (
                ActivationRequestSourceKind::CapturedSelinux,
                RuntimeActivationInvocation::SecurityPolicy(
                    SecurityPolicyActivationInvocation::Selinux(_)
                )
            ) | (
                ActivationRequestSourceKind::CapturedApparmor,
                RuntimeActivationInvocation::SecurityPolicy(
                    SecurityPolicyActivationInvocation::Apparmor(_)
                )
            )
        );
        if !compatible {
            return Err(Error::InternalError(format!(
                "activation source kind '{}' does not match invocation provider '{}'",
                self.source_kind.as_str(),
                self.invocation.source_kind()
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationRequest {
    pub id: i64,
    pub changeset_id: i64,
    pub sequence: i64,
    pub source_kind: ActivationRequestSourceKind,
    pub source_package: String,
    pub source_version: String,
    pub source_entry: String,
    pub invocation: RuntimeActivationInvocation,
    pub invocation_sha256: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationActivationIntentStatus {
    Pending,
    Executing,
    Applied,
    Failed,
    Superseded,
}

impl GenerationActivationIntentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Executing => "executing",
            Self::Applied => "applied",
            Self::Failed => "failed",
            Self::Superseded => "superseded",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "executing" => Ok(Self::Executing),
            "applied" => Ok(Self::Applied),
            "failed" => Ok(Self::Failed),
            "superseded" => Ok(Self::Superseded),
            _ => Err(Error::InternalError(format!(
                "persisted generation activation intent has unknown status '{value}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationActivationIntent {
    pub generation_number: i64,
    pub request: ActivationRequest,
    pub status: GenerationActivationIntentStatus,
    pub attempt_count: i64,
    pub last_error: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug)]
struct RawRequest {
    id: i64,
    changeset_id: i64,
    sequence: i64,
    source_kind: String,
    source_package: String,
    source_version: String,
    source_entry: String,
    invocation_json: String,
    invocation_sha256: String,
    created_at: String,
}

impl RawRequest {
    fn from_row(row: &Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(offset)?,
            changeset_id: row.get(offset + 1)?,
            sequence: row.get(offset + 2)?,
            source_kind: row.get(offset + 3)?,
            source_package: row.get(offset + 4)?,
            source_version: row.get(offset + 5)?,
            source_entry: row.get(offset + 6)?,
            invocation_json: row.get(offset + 7)?,
            invocation_sha256: row.get(offset + 8)?,
            created_at: row.get(offset + 9)?,
        })
    }

    fn decode(self) -> Result<ActivationRequest> {
        let observed = crate::hash::sha256_prefixed(self.invocation_json.as_bytes());
        if observed != self.invocation_sha256 {
            return Err(Error::ChecksumMismatch {
                expected: self.invocation_sha256,
                actual: observed,
            });
        }
        let invocation: RuntimeActivationInvocation = serde_json::from_str(&self.invocation_json)?;
        let source_kind = ActivationRequestSourceKind::parse(&self.source_kind)?;
        NewActivationRequest {
            source_kind,
            source_package: self.source_package.clone(),
            source_version: self.source_version.clone(),
            source_entry: self.source_entry.clone(),
            invocation: invocation.clone(),
        }
        .validate()?;
        Ok(ActivationRequest {
            id: self.id,
            changeset_id: self.changeset_id,
            sequence: self.sequence,
            source_kind,
            source_package: self.source_package,
            source_version: self.source_version,
            source_entry: self.source_entry,
            invocation,
            invocation_sha256: self.invocation_sha256,
            created_at: self.created_at,
        })
    }
}

#[derive(Debug)]
struct RawIntent {
    generation_number: i64,
    status: String,
    attempt_count: i64,
    last_error: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    updated_at: String,
    request: RawRequest,
}

impl RawIntent {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            generation_number: row.get(0)?,
            status: row.get(1)?,
            attempt_count: row.get(2)?,
            last_error: row.get(3)?,
            started_at: row.get(4)?,
            completed_at: row.get(5)?,
            updated_at: row.get(6)?,
            request: RawRequest::from_row(row, 7)?,
        })
    }

    fn decode(self) -> Result<GenerationActivationIntent> {
        Ok(GenerationActivationIntent {
            generation_number: self.generation_number,
            request: self.request.decode()?,
            status: GenerationActivationIntentStatus::parse(&self.status)?,
            attempt_count: self.attempt_count,
            last_error: self.last_error,
            started_at: self.started_at,
            completed_at: self.completed_at,
            updated_at: self.updated_at,
        })
    }
}

impl ActivationRequest {
    const REQUEST_COLUMNS: &'static str = "r.id, r.changeset_id, r.sequence, r.source_kind, \
        r.source_package, r.source_version, r.source_entry, r.invocation_json, \
        r.invocation_sha256, r.created_at";

    /// Append exact requests to a still-pending changeset.
    ///
    /// Sequence is allocated from persisted state, so native and CCS sources
    /// can append independently without inventing a compatibility default.
    pub fn append_batch(
        conn: &Connection,
        changeset_id: i64,
        requests: &[NewActivationRequest],
    ) -> Result<Vec<i64>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM changesets WHERE id = ?1",
                [changeset_id],
                |row| row.get(0),
            )
            .optional()?;
        match status.as_deref() {
            Some("pending") => {}
            Some(other) => {
                return Err(Error::ConflictError(format!(
                    "activation requests cannot be appended to changeset {changeset_id} in status '{other}'"
                )));
            }
            None => {
                return Err(Error::NotFound(format!(
                    "activation request changeset {changeset_id}"
                )));
            }
        }

        let next_sequence: i64 = conn.query_row(
            "SELECT COALESCE(MAX(sequence), -1) + 1
             FROM activation_requests
             WHERE changeset_id = ?1",
            [changeset_id],
            |row| row.get(0),
        )?;
        let mut ids = Vec::with_capacity(requests.len());
        for (sequence, request) in (next_sequence..).zip(requests) {
            request.validate()?;
            let invocation_json = serde_json::to_string(&request.invocation)?;
            let invocation_sha256 = crate::hash::sha256_prefixed(invocation_json.as_bytes());
            conn.execute(
                "INSERT INTO activation_requests (
                    changeset_id, sequence, source_kind, source_package, source_version,
                    source_entry, invocation_json, invocation_sha256, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, CURRENT_TIMESTAMP)",
                params![
                    changeset_id,
                    sequence,
                    request.source_kind.as_str(),
                    &request.source_package,
                    &request.source_version,
                    &request.source_entry,
                    invocation_json,
                    invocation_sha256,
                ],
            )?;
            ids.push(conn.last_insert_rowid());
        }
        Ok(ids)
    }

    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM activation_requests r WHERE r.id = ?1",
            Self::REQUEST_COLUMNS
        );
        let raw = conn
            .prepare(&sql)?
            .query_row([id], |row| RawRequest::from_row(row, 0))
            .optional()?;
        raw.map(RawRequest::decode).transpose()
    }
}

impl GenerationActivationIntent {
    const INTENT_COLUMNS: &'static str = "i.generation_number, i.status, i.attempt_count, \
        i.last_error, i.started_at, i.completed_at, i.updated_at";

    /// Project every eligible request through the generation's changeset
    /// high-water mark. Existing projections are preserved exactly.
    pub fn project_through(
        conn: &Connection,
        generation_number: i64,
        applied_high_water_changeset_id: Option<i64>,
    ) -> Result<usize> {
        let Some(high_water) = applied_high_water_changeset_id else {
            return Ok(0);
        };
        conn.execute(
            "INSERT OR IGNORE INTO generation_activation_intents (
                generation_number, request_id, status, attempt_count, last_error,
                started_at, completed_at, updated_at
             )
             SELECT ?1, r.id, 'pending', 0, NULL, NULL, NULL, CURRENT_TIMESTAMP
             FROM activation_requests r
             JOIN changesets c ON c.id = r.changeset_id
             WHERE r.changeset_id <= ?2
               AND c.status = 'applied'
             ORDER BY r.changeset_id, r.sequence, r.id",
            params![generation_number, high_water],
        )
        .map_err(Into::into)
    }

    /// Return executable work for exactly one booted generation.
    ///
    /// A request completed through another generation is projected as
    /// superseded rather than replayed. Requests whose source changeset was
    /// rolled back are likewise retired before selection.
    pub fn ready_for_generation(conn: &Connection, generation_number: i64) -> Result<Vec<Self>> {
        conn.execute(
            "UPDATE generation_activation_intents AS current
             SET status = 'superseded',
                 last_error = NULL,
                 completed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE current.generation_number = ?1
               AND current.status IN ('pending', 'failed')
               AND (
                   EXISTS (
                       SELECT 1
                       FROM generation_activation_intents completed
                       WHERE completed.request_id = current.request_id
                         AND completed.status = 'applied'
                   )
                   OR NOT EXISTS (
                       SELECT 1
                       FROM activation_requests r
                       JOIN changesets c ON c.id = r.changeset_id
                       WHERE r.id = current.request_id
                         AND c.status = 'applied'
                   )
               )",
            [generation_number],
        )?;
        Self::list_with_statuses(
            conn,
            generation_number,
            &[
                GenerationActivationIntentStatus::Pending,
                GenerationActivationIntentStatus::Failed,
            ],
        )
    }

    /// Recover work claimed by an interrupted consumer.
    ///
    /// The external systemd call and SQLite cannot share one atomic commit.
    /// Conary therefore chooses the source-package-manager-compatible
    /// at-least-once boundary: an interrupted exact operation is retried,
    /// while every durably applied request remains globally suppressed.
    pub fn recover_interrupted_for_generation(
        conn: &Connection,
        generation_number: i64,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE generation_activation_intents
             SET status = 'failed',
                 last_error = 'activation consumer interrupted before durable completion; retrying exact request',
                 completed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE generation_number = ?1
               AND status = 'executing'",
            [generation_number],
        )
        .map_err(Into::into)
    }

    fn list_with_statuses(
        conn: &Connection,
        generation_number: i64,
        statuses: &[GenerationActivationIntentStatus],
    ) -> Result<Vec<Self>> {
        let status_filter = match statuses {
            [
                GenerationActivationIntentStatus::Pending,
                GenerationActivationIntentStatus::Failed,
            ] => "('pending', 'failed')",
            _ => {
                return Err(Error::InternalError(
                    "unsupported activation intent status query".to_string(),
                ));
            }
        };
        let sql = format!(
            "SELECT {}, {}
             FROM generation_activation_intents i
             JOIN activation_requests r ON r.id = i.request_id
             WHERE i.generation_number = ?1
               AND i.status IN {status_filter}
             ORDER BY r.changeset_id, r.sequence, r.id",
            Self::INTENT_COLUMNS,
            ActivationRequest::REQUEST_COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([generation_number], RawIntent::from_row)?;
        rows.map(|row| row.map_err(Error::from).and_then(RawIntent::decode))
            .collect()
    }

    pub fn mark_executing(&self, conn: &Connection) -> Result<()> {
        let changed = conn.execute(
            "UPDATE generation_activation_intents
             SET status = 'executing',
                 attempt_count = attempt_count + 1,
                 last_error = NULL,
                 started_at = CURRENT_TIMESTAMP,
                 completed_at = NULL,
                 updated_at = CURRENT_TIMESTAMP
             WHERE generation_number = ?1
               AND request_id = ?2
               AND status IN ('pending', 'failed')",
            params![self.generation_number, self.request.id],
        )?;
        if changed != 1 {
            return Err(Error::ConflictError(format!(
                "activation request {} for generation {} is no longer ready",
                self.request.id, self.generation_number
            )));
        }
        Ok(())
    }

    pub fn mark_applied(&self, conn: &Connection) -> Result<()> {
        conn.execute("SAVEPOINT mark_activation_applied", [])?;
        let outcome = (|| -> Result<()> {
            let changed = conn.execute(
                "UPDATE generation_activation_intents
                 SET status = 'applied',
                     last_error = NULL,
                     completed_at = CURRENT_TIMESTAMP,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE generation_number = ?1
                   AND request_id = ?2
                   AND status = 'executing'",
                params![self.generation_number, self.request.id],
            )?;
            if changed != 1 {
                return Err(Error::ConflictError(format!(
                    "activation request {} for generation {} is not executing",
                    self.request.id, self.generation_number
                )));
            }
            conn.execute(
                "UPDATE generation_activation_intents
                 SET status = 'superseded',
                     last_error = NULL,
                     completed_at = CURRENT_TIMESTAMP,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE request_id = ?1
                   AND generation_number <> ?2
                   AND status IN ('pending', 'failed')",
                params![self.request.id, self.generation_number],
            )?;
            Ok(())
        })();
        match outcome {
            Ok(()) => {
                conn.execute("RELEASE SAVEPOINT mark_activation_applied", [])?;
                Ok(())
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK TO SAVEPOINT mark_activation_applied", []);
                let _ = conn.execute("RELEASE SAVEPOINT mark_activation_applied", []);
                Err(error)
            }
        }
    }

    pub fn mark_failed(&self, conn: &Connection, error: &str) -> Result<()> {
        let changed = conn.execute(
            "UPDATE generation_activation_intents
             SET status = 'failed',
                 last_error = ?1,
                 completed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE generation_number = ?2
               AND request_id = ?3
               AND status = 'executing'",
            params![error, self.generation_number, self.request.id],
        )?;
        if changed != 1 {
            return Err(Error::ConflictError(format!(
                "activation request {} for generation {} is not executing",
                self.request.id, self.generation_number
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activation::{SystemdActivationAction, SystemdJobMode};
    use crate::db::models::{Changeset, ChangesetStatus, SystemState};
    use crate::db::testing::create_test_db;

    fn request(unit: &str) -> NewActivationRequest {
        NewActivationRequest {
            source_kind: ActivationRequestSourceKind::CapturedSystemctl,
            source_package: "demo".to_string(),
            source_version: "1".to_string(),
            source_entry: "post-install".to_string(),
            invocation: crate::activation::SystemdActivationInvocation {
                action: SystemdActivationAction::Restart,
                units: vec![unit.to_string()],
                job_mode: Some(SystemdJobMode::Replace),
                all: false,
                no_block: false,
                no_ask_password: true,
                quiet: false,
                no_warn: false,
                wait: false,
                show_transaction: false,
            }
            .into(),
        }
    }

    fn applied_changeset(conn: &Connection) -> i64 {
        let mut changeset = Changeset::new("install demo".to_string());
        let id = changeset.insert(conn).unwrap();
        ActivationRequest::append_batch(conn, id, &[request("demo.service")]).unwrap();
        changeset
            .update_status(conn, ChangesetStatus::Applied)
            .unwrap();
        id
    }

    fn generation(conn: &Connection, number: i64) {
        SystemState::new(number, format!("generation {number}"))
            .insert(conn)
            .unwrap();
    }

    #[test]
    fn skipped_generation_carries_forward_unapplied_requests() {
        let (_temp, conn) = create_test_db();
        let changeset_id = applied_changeset(&conn);
        generation(&conn, 4);
        generation(&conn, 5);

        assert_eq!(
            GenerationActivationIntent::project_through(&conn, 4, Some(changeset_id)).unwrap(),
            1
        );
        assert_eq!(
            GenerationActivationIntent::project_through(&conn, 5, Some(changeset_id)).unwrap(),
            1
        );
        let for_four = GenerationActivationIntent::ready_for_generation(&conn, 4).unwrap();
        let for_five = GenerationActivationIntent::ready_for_generation(&conn, 5).unwrap();
        assert_eq!(for_four.len(), 1);
        assert_eq!(for_five.len(), 1);

        for_five[0].mark_executing(&conn).unwrap();
        for_five[0].mark_applied(&conn).unwrap();
        assert!(
            GenerationActivationIntent::ready_for_generation(&conn, 4)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rolled_back_source_is_not_projected() {
        let (_temp, conn) = create_test_db();
        let changeset_id = applied_changeset(&conn);
        let mut changeset = Changeset::find_by_id(&conn, changeset_id).unwrap().unwrap();
        changeset
            .update_status(&conn, ChangesetStatus::RolledBack)
            .unwrap();
        generation(&conn, 2);

        assert_eq!(
            GenerationActivationIntent::project_through(&conn, 2, Some(changeset_id)).unwrap(),
            0
        );
    }

    #[test]
    fn interrupted_execution_is_durably_requeued() {
        let (_temp, conn) = create_test_db();
        let changeset_id = applied_changeset(&conn);
        generation(&conn, 7);
        GenerationActivationIntent::project_through(&conn, 7, Some(changeset_id)).unwrap();
        let intent = GenerationActivationIntent::ready_for_generation(&conn, 7)
            .unwrap()
            .remove(0);
        intent.mark_executing(&conn).unwrap();

        assert_eq!(
            GenerationActivationIntent::recover_interrupted_for_generation(&conn, 7).unwrap(),
            1
        );
        let recovered = GenerationActivationIntent::ready_for_generation(&conn, 7).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].status,
            GenerationActivationIntentStatus::Failed
        );
        assert_eq!(recovered[0].attempt_count, 1);
    }

    #[test]
    fn persisted_invocation_digest_is_verified_before_execution() {
        let (_temp, conn) = create_test_db();
        let changeset_id = applied_changeset(&conn);
        conn.execute(
            "UPDATE activation_requests SET invocation_json = '{}' WHERE changeset_id = ?1",
            [changeset_id],
        )
        .unwrap();
        generation(&conn, 9);
        GenerationActivationIntent::project_through(&conn, 9, Some(changeset_id)).unwrap();

        assert!(
            matches!(
                GenerationActivationIntent::ready_for_generation(&conn, 9),
                Err(Error::ChecksumMismatch { .. })
            ),
            "tampered intent must fail closed before execution"
        );
    }

    #[test]
    fn persisted_source_kind_must_match_the_typed_provider() {
        let (_temp, conn) = create_test_db();
        let changeset_id = applied_changeset(&conn);
        let request_id: i64 = conn
            .query_row(
                "SELECT id FROM activation_requests WHERE changeset_id = ?1",
                [changeset_id],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute(
            "UPDATE activation_requests SET source_kind = 'captured-selinux' WHERE id = ?1",
            [request_id],
        )
        .unwrap();

        let error = ActivationRequest::find_by_id(&conn, request_id).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not match invocation provider"),
            "{error:#}"
        );
    }
}
