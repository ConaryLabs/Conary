// crates/conary-core/src/db/models/trigger.rs

//! Trigger model for path-based post-installation actions
//!
//! Triggers are handlers that run when files matching certain patterns are
//! installed or removed. They provide a more flexible alternative to scriptlets
//! for system-wide actions like ldconfig, update-desktop-database, etc.
//!
//! Key features:
//! - Pattern-based matching (glob patterns for file paths)
//! - DAG ordering via dependencies
//! - Explicit operator-created mutation authority
//! - Per-changeset tracking of triggered handlers

use crate::error::Result;
use glob::Pattern;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::HashMap;
use strum_macros::AsRefStr;

use super::persisted_value::{InvalidPersistedValue, persisted_value_corruption};

/// A trigger defines a handler that runs when files matching a pattern are modified
#[derive(Debug, Clone)]
pub struct Trigger {
    pub id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    /// Comma-separated glob patterns (e.g., "/usr/lib/*.so*,/usr/lib64/*.so*")
    pub pattern: String,
    /// Command to execute when triggered
    pub handler: String,
    /// Lower priority runs first (default: 50)
    pub priority: i32,
    /// Whether this trigger is enabled
    pub enabled: bool,
    pub created_at: Option<String>,
}

impl Trigger {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, name, description, pattern, handler, priority, \
         enabled, created_at";

    /// Create a new trigger with a fully validated path-pattern grammar.
    pub fn new(name: String, pattern: String, handler: String) -> Result<Self> {
        let trigger = Self {
            id: None,
            name,
            description: None,
            pattern,
            handler,
            priority: 50,
            enabled: true,
            created_at: None,
        };
        trigger.compile_patterns()?;
        Ok(trigger)
    }

    /// Create a new trigger with description
    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    /// Set priority (lower runs first)
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Insert this trigger into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        self.compile_patterns()?;
        conn.execute(
            "INSERT INTO triggers (name, description, pattern, handler, priority, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &self.name,
                &self.description,
                &self.pattern,
                &self.handler,
                self.priority,
                self.enabled,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a trigger by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!("SELECT {} FROM triggers WHERE id = ?1", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        let trigger = stmt.query_row([id], Self::from_row).optional()?;
        Ok(trigger)
    }

    /// Find multiple triggers by ID in a single batch query
    pub fn find_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<Self>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {} FROM triggers WHERE id IN ({})",
            Self::COLUMNS,
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let triggers = stmt
            .query_map(rusqlite::params_from_iter(ids.iter()), Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(triggers)
    }

    /// Find a trigger by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        let sql = format!("SELECT {} FROM triggers WHERE name = ?1", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        let trigger = stmt.query_row([name], Self::from_row).optional()?;
        Ok(trigger)
    }

    /// List all triggers
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM triggers ORDER BY priority, name",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let triggers = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(triggers)
    }

    /// List all enabled triggers
    pub fn list_enabled(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM triggers WHERE enabled = 1 ORDER BY priority, name",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let triggers = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(triggers)
    }

    /// Enable a trigger
    pub fn enable(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("UPDATE triggers SET enabled = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Disable a trigger
    pub fn disable(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("UPDATE triggers SET enabled = 0 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete a trigger
    pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
        let rows = conn.execute("DELETE FROM triggers WHERE id = ?1", [id])?;
        Ok(rows > 0)
    }

    /// Convert a database row to a Trigger
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let trigger = Self {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            description: row.get(2)?,
            pattern: row.get(3)?,
            handler: row.get(4)?,
            priority: row.get(5)?,
            enabled: row.get::<_, i32>(6)? != 0,
            created_at: row.get(7)?,
        };
        trigger.compile_patterns().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        Ok(trigger)
    }

    /// Parse the pattern string into individual glob patterns
    pub fn patterns(&self) -> Vec<&str> {
        self.pattern.split(',').map(|s| s.trim()).collect()
    }

    fn compile_patterns(&self) -> Result<Vec<Pattern>> {
        let raw_patterns = self.patterns();
        if raw_patterns.is_empty() || raw_patterns.iter().any(|pattern| pattern.is_empty()) {
            return Err(crate::error::Error::TriggerError(format!(
                "trigger '{}' has an empty path pattern",
                self.name
            )));
        }
        raw_patterns
            .into_iter()
            .map(|pattern| {
                Pattern::new(pattern).map_err(|error| {
                    crate::error::Error::TriggerError(format!(
                        "trigger '{}' has invalid path pattern '{}': {}",
                        self.name, pattern, error
                    ))
                })
            })
            .collect()
    }

    /// Check if a file path matches any of this trigger's validated patterns.
    pub fn matches(&self, path: &str) -> Result<bool> {
        Ok(self
            .compile_patterns()?
            .into_iter()
            .any(|pattern| pattern.matches(path)))
    }

    /// Get dependencies for this trigger
    pub fn get_dependencies(&self, conn: &Connection) -> Result<Vec<String>> {
        let id = self
            .id
            .ok_or_else(|| crate::error::Error::MissingId("Trigger has no ID".to_string()))?;
        TriggerDependency::get_dependencies(conn, id)
    }

    /// Add a dependency (this trigger must run after `depends_on`)
    pub fn add_dependency(&self, conn: &Connection, depends_on: &str) -> Result<()> {
        let id = self
            .id
            .ok_or_else(|| crate::error::Error::MissingId("Trigger has no ID".to_string()))?;
        TriggerDependency::add(conn, id, depends_on)
    }
}

/// Represents a dependency between triggers
#[derive(Debug, Clone)]
pub struct TriggerDependency {
    pub id: Option<i64>,
    pub trigger_id: i64,
    pub depends_on: String,
}

impl TriggerDependency {
    /// Get all dependencies for a trigger
    pub fn get_dependencies(conn: &Connection, trigger_id: i64) -> Result<Vec<String>> {
        let mut stmt =
            conn.prepare("SELECT depends_on FROM trigger_dependencies WHERE trigger_id = ?1")?;

        let deps = stmt
            .query_map([trigger_id], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;

        Ok(deps)
    }

    /// Get dependencies for multiple triggers in a single batch query.
    /// Returns a map of trigger_id -> Vec<depends_on name>.
    pub fn get_dependencies_batch(
        conn: &Connection,
        trigger_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<String>>> {
        if trigger_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = trigger_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT trigger_id, depends_on FROM trigger_dependencies WHERE trigger_id IN ({})",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut map: HashMap<i64, Vec<String>> = HashMap::new();
        let rows = stmt.query_map(rusqlite::params_from_iter(trigger_ids.iter()), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (tid, dep) = row?;
            map.entry(tid).or_default().push(dep);
        }
        Ok(map)
    }

    /// Add a dependency
    pub fn add(conn: &Connection, trigger_id: i64, depends_on: &str) -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO trigger_dependencies (trigger_id, depends_on) VALUES (?1, ?2)",
            params![trigger_id, depends_on],
        )?;
        Ok(())
    }

    /// Remove a dependency
    pub fn remove(conn: &Connection, trigger_id: i64, depends_on: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM trigger_dependencies WHERE trigger_id = ?1 AND depends_on = ?2",
            params![trigger_id, depends_on],
        )?;
        Ok(())
    }
}

/// Status of a trigger in a changeset
#[derive(Debug, Clone, PartialEq, Eq, AsRefStr)]
#[strum(serialize_all = "lowercase")]
pub enum TriggerStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl TriggerStatus {
    pub fn as_str(&self) -> &str {
        self.as_ref()
    }
}

impl TryFrom<&str> for TriggerStatus {
    type Error = InvalidPersistedValue;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "skipped" => Ok(Self::Skipped),
            other => Err(InvalidPersistedValue::new(
                "trigger status",
                other,
                "pending, running, completed, failed, or skipped",
            )),
        }
    }
}

/// Tracks which triggers were activated for a changeset
#[derive(Debug, Clone)]
pub struct ChangesetTrigger {
    pub id: Option<i64>,
    pub changeset_id: i64,
    pub trigger_id: i64,
    pub status: TriggerStatus,
    pub matched_files: i32,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub output: Option<String>,
}

impl ChangesetTrigger {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, changeset_id, trigger_id, status, matched_files, \
         started_at, completed_at, output";

    /// Create a new changeset trigger record
    pub fn new(changeset_id: i64, trigger_id: i64) -> Self {
        Self {
            id: None,
            changeset_id,
            trigger_id,
            status: TriggerStatus::Pending,
            matched_files: 0,
            started_at: None,
            completed_at: None,
            output: None,
        }
    }

    /// Insert or update a changeset trigger
    pub fn upsert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO changeset_triggers (changeset_id, trigger_id, status, matched_files)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(changeset_id, trigger_id) DO UPDATE SET
                matched_files = matched_files + excluded.matched_files",
            params![
                self.changeset_id,
                self.trigger_id,
                self.status.as_str(),
                self.matched_files,
            ],
        )?;

        let id: i64 = conn.query_row(
            "SELECT id FROM changeset_triggers WHERE changeset_id = ?1 AND trigger_id = ?2",
            params![self.changeset_id, self.trigger_id],
            |row| row.get(0),
        )?;
        self.id = Some(id);
        Ok(id)
    }

    /// Increment matched files count
    pub fn increment_matched(conn: &Connection, changeset_id: i64, trigger_id: i64) -> Result<()> {
        conn.execute(
            "INSERT INTO changeset_triggers (changeset_id, trigger_id, status, matched_files)
             VALUES (?1, ?2, 'pending', 1)
             ON CONFLICT(changeset_id, trigger_id) DO UPDATE SET
                matched_files = matched_files + 1",
            params![changeset_id, trigger_id],
        )?;
        Ok(())
    }

    /// Update status to running
    pub fn mark_running(conn: &Connection, changeset_id: i64, trigger_id: i64) -> Result<()> {
        conn.execute(
            "UPDATE changeset_triggers SET status = 'running', started_at = datetime('now')
             WHERE changeset_id = ?1 AND trigger_id = ?2",
            params![changeset_id, trigger_id],
        )?;
        Ok(())
    }

    /// Update status to completed with output
    pub fn mark_completed(
        conn: &Connection,
        changeset_id: i64,
        trigger_id: i64,
        output: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "UPDATE changeset_triggers SET status = 'completed', completed_at = datetime('now'), output = ?3
             WHERE changeset_id = ?1 AND trigger_id = ?2",
            params![changeset_id, trigger_id, output],
        )?;
        Ok(())
    }

    /// Update status to failed with error message
    pub fn mark_failed(
        conn: &Connection,
        changeset_id: i64,
        trigger_id: i64,
        error: &str,
    ) -> Result<()> {
        conn.execute(
            "UPDATE changeset_triggers SET status = 'failed', completed_at = datetime('now'), output = ?3
             WHERE changeset_id = ?1 AND trigger_id = ?2",
            params![changeset_id, trigger_id, error],
        )?;
        Ok(())
    }

    /// Get all triggers for a changeset
    pub fn find_by_changeset(conn: &Connection, changeset_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM changeset_triggers WHERE changeset_id = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let triggers = stmt
            .query_map([changeset_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(triggers)
    }

    /// Get pending triggers for a changeset
    pub fn find_pending(conn: &Connection, changeset_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM changeset_triggers WHERE changeset_id = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let triggers = stmt
            .query_map([changeset_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(triggers
            .into_iter()
            .filter(|trigger| trigger.status == TriggerStatus::Pending)
            .collect())
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let id: i64 = row.get(0)?;
        let status_str: String = row.get(3)?;
        let status = TriggerStatus::try_from(status_str.as_str()).map_err(|error| {
            persisted_value_corruption("changeset_triggers", id, "status", 3, error)
        })?;
        Ok(Self {
            id: Some(id),
            changeset_id: row.get(1)?,
            trigger_id: row.get(2)?,
            status,
            matched_files: row.get(4)?,
            started_at: row.get(5)?,
            completed_at: row.get(6)?,
            output: row.get(7)?,
        })
    }
}

#[cfg(test)]
mod tests;
