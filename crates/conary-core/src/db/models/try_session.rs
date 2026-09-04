// crates/conary-core/src/db/models/try_session.rs

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
    pub package_signing_key: String,
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
    pub package_signing_key: &'a str,
    pub package_name: Option<&'a str>,
    pub package_version: Option<&'a str>,
    pub previous_generation_id: Option<i64>,
    pub mode: TrySessionMode,
    pub work_dir: &'a str,
}

impl TrySession {
    const COLUMNS: &'static str = "id, package_path, package_signing_key, package_name, package_version, \
        previous_generation_id, try_generation_id, launcher_pid, launcher_boot_id, \
        status, mode, work_dir, last_error, started_at, updated_at, completed_at";

    pub fn create_active(conn: &Connection, session: CreateTrySession<'_>) -> Result<Self> {
        let result = conn.execute(
            "INSERT INTO try_sessions (
                id, package_path, package_signing_key, package_name, package_version,
                previous_generation_id, status, mode, work_dir
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session.id,
                session.package_path,
                session.package_signing_key,
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

    pub fn replace_active_try_generation(
        &self,
        conn: &Connection,
        expected_try_generation_id: i64,
        package_path: &str,
        package_signing_key: &str,
        next_try_generation_id: i64,
    ) -> Result<bool> {
        let rows = conn.execute(
            "UPDATE try_sessions
             SET package_path = ?1,
                 package_signing_key = ?2,
                 try_generation_id = ?3,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
             WHERE id = ?4
               AND status = 'active'
               AND try_generation_id = ?5",
            params![
                package_path,
                package_signing_key,
                next_try_generation_id,
                self.id,
                expected_try_generation_id,
            ],
        )?;
        Ok(rows == 1)
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
        let status_raw: String = row.get(9)?;
        let mode_raw: String = row.get(10)?;
        let status = status_raw.parse::<TrySessionStatus>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e))
        })?;
        let mode = mode_raw.parse::<TrySessionMode>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(e))
        })?;

        Ok(Self {
            id: row.get(0)?,
            package_path: row.get(1)?,
            package_signing_key: row.get(2)?,
            package_name: row.get(3)?,
            package_version: row.get(4)?,
            previous_generation_id: row.get(5)?,
            try_generation_id: row.get(6)?,
            launcher_pid: row.get(7)?,
            launcher_boot_id: row.get(8)?,
            status,
            mode,
            work_dir: row.get(11)?,
            last_error: row.get(12)?,
            started_at: row.get(13)?,
            updated_at: row.get(14)?,
            completed_at: row.get(15)?,
        })
    }
}

#[cfg(test)]
mod tests;
