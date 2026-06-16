// conary-core/src/db/models/try_session.rs

//! Durable state for `conary try` sessions.

use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use strum_macros::{AsRefStr, Display, EnumString};

#[derive(Debug, Clone, Copy, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum TrySessionStatus {
    Active,
    Orphaned,
    Kept,
    RolledBack,
}

impl TrySessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Orphaned => "orphaned",
            Self::Kept => "kept",
            Self::RolledBack => "rolled_back",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum TrySessionMode {
    Namespace,
    Activated,
}

impl TrySessionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Namespace => "namespace",
            Self::Activated => "activated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrySession {
    pub id: String,
    pub package_path: String,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
    pub previous_generation_id: Option<i64>,
    pub try_generation_id: Option<i64>,
    pub launcher_pid: Option<i64>,
    pub launcher_boot_id: Option<String>,
    pub status: TrySessionStatus,
    pub mode: TrySessionMode,
    pub work_dir: String,
    pub last_error: Option<String>,
    pub started_at: Option<String>,
    pub updated_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateTrySession<'a> {
    pub id: &'a str,
    pub package_path: &'a str,
    pub package_name: Option<&'a str>,
    pub package_version: Option<&'a str>,
    pub previous_generation_id: Option<i64>,
    pub mode: TrySessionMode,
    pub work_dir: &'a str,
}

impl TrySession {
    const COLUMNS: &'static str = "id, package_path, package_name, package_version, \
        previous_generation_id, try_generation_id, launcher_pid, launcher_boot_id, \
        status, mode, work_dir, last_error, started_at, updated_at, completed_at";

    pub fn create_active(conn: &Connection, session: CreateTrySession<'_>) -> Result<Self> {
        let result = conn.execute(
            "INSERT INTO try_sessions (
                id, package_path, package_name, package_version, previous_generation_id,
                status, mode, work_dir
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session.id,
                session.package_path,
                session.package_name,
                session.package_version,
                session.previous_generation_id,
                TrySessionStatus::Active.as_str(),
                session.mode.as_str(),
                session.work_dir,
            ],
        );

        if let Err(err) = result {
            if Self::is_single_open_constraint_error(&err) {
                return Err(Self::active_session_conflict(conn)?);
            }
            return Err(err.into());
        }

        Self::find_by_id(conn, session.id)?
            .ok_or_else(|| Error::InternalError("inserted try session row not found".to_string()))
    }

    pub fn find_active_or_orphaned(conn: &Connection) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM try_sessions
             WHERE status IN ('active', 'orphaned')
             ORDER BY updated_at DESC, started_at DESC, id DESC
             LIMIT 1",
            Self::COLUMNS
        );
        conn.prepare(&sql)?
            .query_row([], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn find_by_id(conn: &Connection, id: &str) -> Result<Option<Self>> {
        let sql = format!("SELECT {} FROM try_sessions WHERE id = ?1", Self::COLUMNS);
        conn.prepare(&sql)?
            .query_row([id], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn set_try_generation(&self, conn: &Connection, try_generation_id: i64) -> Result<()> {
        let affected = conn.execute(
            "UPDATE try_sessions
             SET try_generation_id = ?1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
             WHERE id = ?2
               AND status IN ('active', 'orphaned')",
            params![try_generation_id, self.id],
        )?;
        self.require_open_update(conn, affected)
    }

    pub fn set_launcher(
        &self,
        conn: &Connection,
        launcher_pid: i64,
        launcher_boot_id: &str,
    ) -> Result<()> {
        let affected = conn.execute(
            "UPDATE try_sessions
             SET launcher_pid = ?1,
                 launcher_boot_id = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
             WHERE id = ?3
               AND status IN ('active', 'orphaned')",
            params![launcher_pid, launcher_boot_id, self.id],
        )?;
        self.require_open_update(conn, affected)
    }

    pub fn clear_launcher(&self, conn: &Connection) -> Result<()> {
        let affected = conn.execute(
            "UPDATE try_sessions
             SET launcher_pid = NULL,
                 launcher_boot_id = NULL,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
             WHERE id = ?1
               AND status IN ('active', 'orphaned')",
            params![self.id],
        )?;
        self.require_open_update(conn, affected)
    }

    pub fn record_boot_without_launcher(&self, conn: &Connection, boot_id: &str) -> Result<()> {
        let affected = conn.execute(
            "UPDATE try_sessions
             SET launcher_pid = NULL,
                 launcher_boot_id = ?1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
             WHERE id = ?2
               AND status IN ('active', 'orphaned')",
            params![boot_id, self.id],
        )?;
        self.require_open_update(conn, affected)
    }

    pub fn mark_orphaned(&self, conn: &Connection) -> Result<()> {
        self.set_status(conn, TrySessionStatus::Orphaned, false, None)
    }

    pub fn mark_kept(&self, conn: &Connection) -> Result<()> {
        self.set_status(conn, TrySessionStatus::Kept, true, None)
    }

    pub fn mark_rolled_back(&self, conn: &Connection) -> Result<()> {
        self.set_status(conn, TrySessionStatus::RolledBack, true, None)
    }

    pub fn mark_failed_orphaned(&self, conn: &Connection, last_error: &str) -> Result<()> {
        self.set_status(conn, TrySessionStatus::Orphaned, false, Some(last_error))
    }

    fn set_status(
        &self,
        conn: &Connection,
        status: TrySessionStatus,
        complete: bool,
        last_error: Option<&str>,
    ) -> Result<()> {
        let affected = conn.execute(
            "UPDATE try_sessions
             SET status = ?1,
                 last_error = COALESCE(?2, last_error),
                 completed_at = CASE
                     WHEN ?3 THEN strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
                     ELSE completed_at
                 END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
             WHERE id = ?4
               AND status IN ('active', 'orphaned')",
            params![status.as_str(), last_error, complete, self.id],
        )?;
        self.require_open_update(conn, affected)
    }

    fn require_open_update(&self, conn: &Connection, affected: usize) -> Result<()> {
        if affected > 0 {
            return Ok(());
        }

        match Self::find_by_id(conn, &self.id)? {
            Some(session) => Err(Error::ConflictError(format!(
                "try session {} is {}, not active or orphaned",
                self.id,
                session.status.as_str()
            ))),
            None => Err(Error::NotFound(format!(
                "try session {} not found",
                self.id
            ))),
        }
    }

    fn active_session_conflict(conn: &Connection) -> Result<Error> {
        let message = match Self::find_active_or_orphaned(conn)? {
            Some(session) => format!(
                "active or orphaned try session already exists: {}",
                session.id
            ),
            None => "active or orphaned try session already exists".to_string(),
        };
        Ok(Error::ConflictError(message))
    }

    fn is_single_open_constraint_error(error: &rusqlite::Error) -> bool {
        match error {
            rusqlite::Error::SqliteFailure(sqlite_error, message) => {
                sqlite_error.code == rusqlite::ErrorCode::ConstraintViolation
                    && message.as_deref().is_some_and(|message| {
                        message.contains("try_sessions.open_slot")
                            || message.contains("idx_try_sessions_single_open")
                    })
            }
            _ => false,
        }
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let status_raw: String = row.get(8)?;
        let mode_raw: String = row.get(9)?;
        let status = status_raw.parse::<TrySessionStatus>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
        })?;
        let mode = mode_raw.parse::<TrySessionMode>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e))
        })?;

        Ok(Self {
            id: row.get(0)?,
            package_path: row.get(1)?,
            package_name: row.get(2)?,
            package_version: row.get(3)?,
            previous_generation_id: row.get(4)?,
            try_generation_id: row.get(5)?,
            launcher_pid: row.get(6)?,
            launcher_boot_id: row.get(7)?,
            status,
            mode,
            work_dir: row.get(10)?,
            last_error: row.get(11)?,
            started_at: row.get(12)?,
            updated_at: row.get(13)?,
            completed_at: row.get(14)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    fn create_namespace_session(conn: &Connection, id: &str) -> TrySession {
        TrySession::create_active(
            conn,
            CreateTrySession {
                id,
                package_path: &format!("/tmp/{id}.ccs"),
                package_name: Some("demo"),
                package_version: Some("1.0.0-1"),
                previous_generation_id: Some(41),
                mode: TrySessionMode::Namespace,
                work_dir: &format!("/var/lib/conary/try/{id}"),
            },
        )
        .unwrap()
    }

    fn unsaved_session(id: &str) -> TrySession {
        TrySession {
            id: id.to_string(),
            package_path: format!("/tmp/{id}.ccs"),
            package_name: None,
            package_version: None,
            previous_generation_id: None,
            try_generation_id: None,
            launcher_pid: None,
            launcher_boot_id: None,
            status: TrySessionStatus::Active,
            mode: TrySessionMode::Namespace,
            work_dir: format!("/var/lib/conary/try/{id}"),
            last_error: None,
            started_at: None,
            updated_at: None,
            completed_at: None,
        }
    }

    fn force_old_updated_at(conn: &Connection, id: &str) {
        conn.execute(
            "UPDATE try_sessions SET updated_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
            [id],
        )
        .unwrap();
    }

    fn assert_rfc3339_utc(value: Option<&str>) {
        let value = value.unwrap();
        assert_eq!(value.len(), "2026-06-12T12:00:00Z".len());
        assert!(value.contains('T'));
        assert!(value.ends_with('Z'));
    }

    #[test]
    fn create_active_persists_active_session() {
        let (_temp, conn) = create_test_db();

        let session = TrySession::create_active(
            &conn,
            CreateTrySession {
                id: "try-a",
                package_path: "/tmp/demo.ccs",
                package_name: Some("demo"),
                package_version: Some("1.0.0-1"),
                previous_generation_id: Some(7),
                mode: TrySessionMode::Namespace,
                work_dir: "/var/lib/conary/try/try-a",
            },
        )
        .unwrap();

        assert_eq!(session.id, "try-a");
        assert_eq!(session.package_path, "/tmp/demo.ccs");
        assert_eq!(session.package_name.as_deref(), Some("demo"));
        assert_eq!(session.package_version.as_deref(), Some("1.0.0-1"));
        assert_eq!(session.previous_generation_id, Some(7));
        assert_eq!(session.try_generation_id, None);
        assert_eq!(session.status, TrySessionStatus::Active);
        assert_eq!(session.mode, TrySessionMode::Namespace);
        assert_eq!(session.work_dir, "/var/lib/conary/try/try-a");
        assert_eq!(session.last_error, None);
        assert_rfc3339_utc(session.started_at.as_deref());
        assert_rfc3339_utc(session.updated_at.as_deref());
        assert_eq!(session.completed_at, None);

        let stored = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
        assert_eq!(stored, session);
    }

    #[test]
    fn second_active_session_fails() {
        let (_temp, conn) = create_test_db();
        create_namespace_session(&conn, "try-a");

        let err = TrySession::create_active(
            &conn,
            CreateTrySession {
                id: "try-b",
                package_path: "/tmp/other.ccs",
                package_name: None,
                package_version: None,
                previous_generation_id: None,
                mode: TrySessionMode::Namespace,
                work_dir: "/var/lib/conary/try/try-b",
            },
        )
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("Conflict"));
        assert!(message.contains("try-a"));
        assert!(message.contains("active or orphaned try session"));
        assert!(!message.contains("UNIQUE"));
        assert!(!message.contains("try_sessions_single_open"));
    }

    #[test]
    fn rolled_back_session_allows_later_active_session() {
        let (_temp, conn) = create_test_db();
        let first = create_namespace_session(&conn, "try-a");
        force_old_updated_at(&conn, "try-a");

        first.mark_rolled_back(&conn).unwrap();

        let stored_first = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
        assert_eq!(stored_first.status, TrySessionStatus::RolledBack);
        assert_ne!(
            stored_first.updated_at.as_deref(),
            Some("2000-01-01T00:00:00Z")
        );
        assert_rfc3339_utc(stored_first.completed_at.as_deref());

        let second = create_namespace_session(&conn, "try-b");
        assert_eq!(second.status, TrySessionStatus::Active);
    }

    #[test]
    fn find_active_or_orphaned_returns_only_open_sessions() {
        let (_temp, conn) = create_test_db();
        let active = create_namespace_session(&conn, "try-a");

        let found = TrySession::find_active_or_orphaned(&conn).unwrap().unwrap();
        assert_eq!(found.id, active.id);
        assert_eq!(found.status, TrySessionStatus::Active);

        force_old_updated_at(&conn, "try-a");
        active.mark_kept(&conn).unwrap();
        let kept = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
        assert_eq!(kept.status, TrySessionStatus::Kept);
        assert_ne!(kept.updated_at.as_deref(), Some("2000-01-01T00:00:00Z"));
        assert_rfc3339_utc(kept.completed_at.as_deref());
        assert!(
            TrySession::find_active_or_orphaned(&conn)
                .unwrap()
                .is_none()
        );

        let orphaned = create_namespace_session(&conn, "try-b");
        force_old_updated_at(&conn, "try-b");
        orphaned.mark_orphaned(&conn).unwrap();

        let found = TrySession::find_active_or_orphaned(&conn).unwrap().unwrap();
        assert_eq!(found.id, "try-b");
        assert_eq!(found.status, TrySessionStatus::Orphaned);
        assert_ne!(found.updated_at.as_deref(), Some("2000-01-01T00:00:00Z"));
        assert_eq!(found.completed_at, None);
    }

    #[test]
    fn set_launcher_records_process_and_boot_identity() {
        let (_temp, conn) = create_test_db();
        let session = create_namespace_session(&conn, "try-a");
        force_old_updated_at(&conn, "try-a");

        session.set_launcher(&conn, 4242, "boot-123").unwrap();

        let stored = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
        assert_eq!(stored.launcher_pid, Some(4242));
        assert_eq!(stored.launcher_boot_id.as_deref(), Some("boot-123"));
        assert_ne!(stored.updated_at.as_deref(), Some("2000-01-01T00:00:00Z"));
    }

    #[test]
    fn clear_launcher_clears_process_identity_on_open_session() {
        let (_temp, conn) = create_test_db();
        let session = create_namespace_session(&conn, "try-a");
        session.set_launcher(&conn, 4242, "boot-123").unwrap();
        force_old_updated_at(&conn, "try-a");

        session.clear_launcher(&conn).unwrap();

        let stored = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
        assert_eq!(stored.launcher_pid, None);
        assert_eq!(stored.launcher_boot_id, None);
        assert_ne!(stored.updated_at.as_deref(), Some("2000-01-01T00:00:00Z"));
    }

    #[test]
    fn record_boot_without_launcher_records_boot_and_clears_pid_on_open_session() {
        let (_temp, conn) = create_test_db();
        let session = create_namespace_session(&conn, "try-a");
        session.set_launcher(&conn, 4242, "old-boot").unwrap();
        force_old_updated_at(&conn, "try-a");

        session
            .record_boot_without_launcher(&conn, "boot-456")
            .unwrap();

        let stored = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
        assert_eq!(stored.launcher_pid, None);
        assert_eq!(stored.launcher_boot_id.as_deref(), Some("boot-456"));
        assert_ne!(stored.updated_at.as_deref(), Some("2000-01-01T00:00:00Z"));
    }

    #[test]
    fn launcher_identity_helpers_refuse_terminal_sessions() {
        let (_temp, conn) = create_test_db();
        let kept = create_namespace_session(&conn, "try-kept");
        kept.mark_kept(&conn).unwrap();

        for err in [
            kept.clear_launcher(&conn).unwrap_err(),
            kept.record_boot_without_launcher(&conn, "boot-789")
                .unwrap_err(),
        ] {
            let message = err.to_string();
            assert!(message.contains("Conflict"), "{message}");
            assert!(message.contains("try-kept"), "{message}");
            assert!(message.contains("not active or orphaned"), "{message}");
        }
    }

    #[test]
    fn set_try_generation_and_mark_failed_orphaned_update_open_session() {
        let (_temp, conn) = create_test_db();
        let session = create_namespace_session(&conn, "try-a");
        force_old_updated_at(&conn, "try-a");

        session.set_try_generation(&conn, 99).unwrap();

        let stored = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
        assert_eq!(stored.try_generation_id, Some(99));
        assert_ne!(stored.updated_at.as_deref(), Some("2000-01-01T00:00:00Z"));

        force_old_updated_at(&conn, "try-a");
        session
            .mark_failed_orphaned(&conn, "launcher exited before cleanup")
            .unwrap();

        let stored = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
        assert_eq!(stored.status, TrySessionStatus::Orphaned);
        assert_eq!(
            stored.last_error.as_deref(),
            Some("launcher exited before cleanup")
        );
        assert_ne!(stored.updated_at.as_deref(), Some("2000-01-01T00:00:00Z"));
        assert_eq!(stored.completed_at, None);
        assert_eq!(
            TrySession::find_active_or_orphaned(&conn)
                .unwrap()
                .unwrap()
                .id,
            "try-a"
        );
    }

    #[test]
    fn terminal_sessions_cannot_be_reopened() {
        let (_temp, conn) = create_test_db();
        let kept = create_namespace_session(&conn, "try-kept");
        kept.mark_kept(&conn).unwrap();
        let kept_completed_at = TrySession::find_by_id(&conn, "try-kept")
            .unwrap()
            .unwrap()
            .completed_at;

        let err = kept.mark_orphaned(&conn).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("Conflict"));
        assert!(message.contains("try-kept"));
        assert!(message.contains("not active or orphaned"));

        let stored_kept = TrySession::find_by_id(&conn, "try-kept").unwrap().unwrap();
        assert_eq!(stored_kept.status, TrySessionStatus::Kept);
        assert_eq!(stored_kept.completed_at, kept_completed_at);
        assert!(
            TrySession::find_active_or_orphaned(&conn)
                .unwrap()
                .is_none()
        );

        let rolled_back = create_namespace_session(&conn, "try-rolled-back");
        rolled_back.mark_rolled_back(&conn).unwrap();
        let rolled_back_completed_at = TrySession::find_by_id(&conn, "try-rolled-back")
            .unwrap()
            .unwrap()
            .completed_at;

        let err = rolled_back
            .mark_failed_orphaned(&conn, "stale launcher")
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("Conflict"));
        assert!(message.contains("try-rolled-back"));
        assert!(message.contains("not active or orphaned"));

        let stored_rolled_back = TrySession::find_by_id(&conn, "try-rolled-back")
            .unwrap()
            .unwrap();
        assert_eq!(stored_rolled_back.status, TrySessionStatus::RolledBack);
        assert_eq!(stored_rolled_back.last_error, None);
        assert_eq!(stored_rolled_back.completed_at, rolled_back_completed_at);
        assert!(
            TrySession::find_active_or_orphaned(&conn)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn missing_session_updates_return_errors() {
        let (_temp, conn) = create_test_db();
        let missing = unsaved_session("try-missing");

        for err in [
            missing.set_try_generation(&conn, 10).unwrap_err(),
            missing.set_launcher(&conn, 4242, "boot-123").unwrap_err(),
            missing.mark_orphaned(&conn).unwrap_err(),
            missing.mark_kept(&conn).unwrap_err(),
            missing.mark_rolled_back(&conn).unwrap_err(),
            missing.mark_failed_orphaned(&conn, "missing").unwrap_err(),
        ] {
            let message = err.to_string();
            assert!(message.contains("Not found"));
            assert!(message.contains("try-missing"));
        }
    }
}
