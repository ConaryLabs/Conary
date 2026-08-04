// conary-core/src/db/models/trigger.rs

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
mod tests {
    use super::super::persisted_value::PersistedValueCorruption;
    use super::super::trigger_engine::TriggerEngine;
    use super::*;
    use crate::trigger::TriggerExecutor;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();

        // Create tables
        conn.execute_batch(
            "
            CREATE TABLE triggers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                description TEXT,
                pattern TEXT NOT NULL,
                handler TEXT NOT NULL,
                priority INTEGER NOT NULL DEFAULT 50,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE trigger_dependencies (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trigger_id INTEGER NOT NULL REFERENCES triggers(id) ON DELETE CASCADE,
                depends_on TEXT NOT NULL,
                UNIQUE(trigger_id, depends_on)
            );

            CREATE TABLE changesets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                description TEXT
            );

            CREATE TABLE changeset_triggers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                changeset_id INTEGER NOT NULL,
                trigger_id INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK(status IN ('pending', 'running', 'completed', 'failed', 'skipped')),
                matched_files INTEGER NOT NULL DEFAULT 0,
                started_at TEXT,
                completed_at TEXT,
                output TEXT,
                UNIQUE(changeset_id, trigger_id)
            );
            ",
        )
        .unwrap();

        (temp_file, conn)
    }

    #[test]
    fn test_trigger_crud() {
        let (_temp, conn) = create_test_db();

        // Create trigger
        let mut trigger = Trigger::new(
            "ldconfig".to_string(),
            "/usr/lib/*.so*".to_string(),
            "/sbin/ldconfig".to_string(),
        )
        .unwrap();
        trigger.description = Some("Update shared library cache".to_string());
        let id = trigger.insert(&conn).unwrap();

        // Find by ID
        let found = Trigger::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.name, "ldconfig");
        assert_eq!(found.handler, "/sbin/ldconfig");

        // Find by name
        let found = Trigger::find_by_name(&conn, "ldconfig").unwrap().unwrap();
        assert_eq!(found.id, Some(id));

        // List all
        let triggers = Trigger::list_all(&conn).unwrap();
        assert_eq!(triggers.len(), 1);

        // Disable
        Trigger::disable(&conn, id).unwrap();
        let found = Trigger::find_by_id(&conn, id).unwrap().unwrap();
        assert!(!found.enabled);

        // Enable
        Trigger::enable(&conn, id).unwrap();
        let found = Trigger::find_by_id(&conn, id).unwrap().unwrap();
        assert!(found.enabled);

        assert!(Trigger::delete(&conn, id).unwrap());
        assert!(Trigger::find_by_id(&conn, id).unwrap().is_none());
    }

    #[test]
    fn test_trigger_pattern_matching() {
        let trigger = Trigger::new(
            "ldconfig".to_string(),
            "/usr/lib/*.so*,/usr/lib64/*.so*".to_string(),
            "/sbin/ldconfig".to_string(),
        )
        .unwrap();

        // Should match
        assert!(trigger.matches("/usr/lib/libssl.so.3").unwrap());
        assert!(trigger.matches("/usr/lib64/libc.so.6").unwrap());
        assert!(trigger.matches("/usr/lib/libfoo.so").unwrap());

        // Should not match
        assert!(!trigger.matches("/usr/bin/ls").unwrap());
        assert!(!trigger.matches("/etc/passwd").unwrap());
        assert!(!trigger.matches("/usr/lib/pkgconfig/foo.pc").unwrap());
    }

    #[test]
    fn invalid_trigger_patterns_fail_before_persistence() {
        let invalid_glob = Trigger::new(
            "invalid-glob".to_string(),
            "/usr/lib/[broken".to_string(),
            "/bin/true".to_string(),
        )
        .unwrap_err();
        assert!(
            invalid_glob
                .to_string()
                .contains("invalid path pattern '/usr/lib/[broken'"),
            "{invalid_glob}"
        );

        let empty_member = Trigger::new(
            "empty-member".to_string(),
            "/usr/lib/*,,/usr/lib64/*".to_string(),
            "/bin/true".to_string(),
        )
        .unwrap_err();
        assert!(
            empty_member.to_string().contains("empty path pattern"),
            "{empty_member}"
        );
    }

    #[test]
    fn invalid_persisted_trigger_pattern_is_a_planning_error() {
        let (_temp, conn) = create_test_db();
        conn.execute(
            "INSERT INTO triggers (name, pattern, handler, priority, enabled)
             VALUES ('corrupt', '/usr/lib/[broken', '/bin/true', 50, 1)",
            [],
        )
        .unwrap();

        let error = TriggerEngine::new(&conn)
            .find_matching_triggers(&["/usr/lib/libfoo.so".to_string()])
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("corrupt"), "{message}");
        assert!(message.contains("invalid path pattern"), "{message}");
    }

    #[test]
    fn corrupt_persisted_status_stops_execution_before_the_handler_loop() {
        let (_temp, conn) = create_test_db();
        let mut trigger = Trigger::new(
            "must-not-run".to_string(),
            "/usr/lib/*".to_string(),
            "/bin/true".to_string(),
        )
        .unwrap();
        let trigger_id = trigger.insert(&conn).unwrap();
        conn.execute(
            "INSERT INTO changesets (description) VALUES ('corrupt status')",
            [],
        )
        .unwrap();
        let changeset_id = conn.last_insert_rowid();
        let mut changeset_trigger = ChangesetTrigger::new(changeset_id, trigger_id);
        let row_id = changeset_trigger.upsert(&conn).unwrap();

        conn.pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        conn.execute(
            "UPDATE changeset_triggers SET status = 'already-ran-maybe' WHERE id = ?1",
            [row_id],
        )
        .unwrap();
        conn.pragma_update(None, "ignore_check_constraints", false)
            .unwrap();

        let root = tempfile::tempdir().unwrap();
        let error = TriggerExecutor::new(&conn, root.path())
            .execute_pending(changeset_id)
            .unwrap_err();
        let crate::error::Error::Database(rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            source,
        )) = error
        else {
            panic!("unexpected error: {error}");
        };
        let corruption = source
            .downcast_ref::<PersistedValueCorruption>()
            .expect("status error must retain typed row corruption");
        assert_eq!(corruption.table(), "changeset_triggers");
        assert_eq!(corruption.row_id(), row_id);
        assert_eq!(corruption.column(), "status");
        assert_eq!(corruption.value(), "already-ran-maybe");

        let stored_status: String = conn
            .query_row(
                "SELECT status FROM changeset_triggers WHERE id = ?1",
                [row_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_status, "already-ran-maybe");
    }

    #[test]
    fn insert_revalidates_public_pattern_field() {
        let (_temp, conn) = create_test_db();
        let mut trigger = Trigger::new(
            "mutated".to_string(),
            "/usr/lib/*".to_string(),
            "/bin/true".to_string(),
        )
        .unwrap();
        trigger.pattern = "/usr/lib/[broken".to_string();

        let error = trigger.insert(&conn).unwrap_err();
        assert!(
            error.to_string().contains("invalid path pattern"),
            "{error}"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM triggers", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_trigger_dependencies() {
        let (_temp, conn) = create_test_db();

        // Create triggers
        let mut trigger1 = Trigger::new(
            "sysusers".to_string(),
            "/usr/lib/sysusers.d/*".to_string(),
            "systemd-sysusers".to_string(),
        )
        .unwrap();
        let mut trigger2 = Trigger::new(
            "tmpfiles".to_string(),
            "/usr/lib/tmpfiles.d/*".to_string(),
            "systemd-tmpfiles".to_string(),
        )
        .unwrap();

        trigger1.insert(&conn).unwrap();
        let id2 = trigger2.insert(&conn).unwrap();

        // tmpfiles depends on sysusers
        TriggerDependency::add(&conn, id2, "sysusers").unwrap();

        let deps = TriggerDependency::get_dependencies(&conn, id2).unwrap();
        assert_eq!(deps, vec!["sysusers"]);
    }

    #[test]
    fn test_changeset_trigger_tracking() {
        let (_temp, conn) = create_test_db();

        // Create a changeset
        conn.execute("INSERT INTO changesets (description) VALUES ('test')", [])
            .unwrap();
        let changeset_id = conn.last_insert_rowid();

        // Create trigger
        let mut trigger =
            Trigger::new("test".to_string(), "/*".to_string(), "true".to_string()).unwrap();
        let trigger_id = trigger.insert(&conn).unwrap();

        // Track trigger
        let mut ct = ChangesetTrigger::new(changeset_id, trigger_id);
        ct.matched_files = 5;
        ct.upsert(&conn).unwrap();

        // Find pending
        let pending = ChangesetTrigger::find_pending(&conn, changeset_id).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].matched_files, 5);

        // Mark running
        ChangesetTrigger::mark_running(&conn, changeset_id, trigger_id).unwrap();
        let all = ChangesetTrigger::find_by_changeset(&conn, changeset_id).unwrap();
        assert_eq!(all[0].status, TriggerStatus::Running);

        // Mark completed
        ChangesetTrigger::mark_completed(&conn, changeset_id, trigger_id, Some("OK")).unwrap();
        let all = ChangesetTrigger::find_by_changeset(&conn, changeset_id).unwrap();
        assert_eq!(all[0].status, TriggerStatus::Completed);
    }

    #[test]
    fn test_trigger_engine_matching() {
        let (_temp, conn) = create_test_db();

        // Create triggers
        let mut t1 = Trigger::new(
            "ldconfig".to_string(),
            "/usr/lib/*.so*".to_string(),
            "/sbin/ldconfig".to_string(),
        )
        .unwrap();
        let mut t2 = Trigger::new(
            "icons".to_string(),
            "/usr/share/icons/*".to_string(),
            "gtk-update-icon-cache".to_string(),
        )
        .unwrap();
        t1.insert(&conn).unwrap();
        t2.insert(&conn).unwrap();

        let engine = TriggerEngine::new(&conn);
        let files = vec![
            "/usr/lib/libssl.so.3".to_string(),
            "/usr/lib/libcrypto.so.3".to_string(),
            "/usr/share/icons/hicolor/48x48/apps/foo.png".to_string(),
        ];

        let matches = engine.find_matching_triggers(&files).unwrap();
        assert_eq!(matches.len(), 2);

        // Check that ldconfig matched 2 files and icons matched 1
        for (trigger, matched) in &matches {
            match trigger.name.as_str() {
                "ldconfig" => assert_eq!(matched.len(), 2),
                "icons" => assert_eq!(matched.len(), 1),
                _ => panic!("Unexpected trigger"),
            }
        }
    }

    #[test]
    fn test_execution_order_preserves_topological_sort() {
        // Regression test: a high-priority trigger that depends on a low-priority
        // trigger must still execute after its dependency. The topological order
        // must not be destroyed by a secondary priority sort.
        let (_temp, conn) = create_test_db();

        // Create trigger B (low priority = runs first if no deps, priority 90)
        let mut trigger_b = Trigger::new(
            "trigger_b".to_string(),
            "/usr/lib/*".to_string(),
            "/bin/true".to_string(),
        )
        .unwrap();
        trigger_b.priority = 90;
        let id_b = trigger_b.insert(&conn).unwrap();

        // Create trigger A (high priority = would run first by priority alone, priority 10)
        // but A depends on B, so B must run first
        let mut trigger_a = Trigger::new(
            "trigger_a".to_string(),
            "/usr/lib/*".to_string(),
            "/bin/true".to_string(),
        )
        .unwrap();
        trigger_a.priority = 10;
        let id_a = trigger_a.insert(&conn).unwrap();

        // A depends on B
        TriggerDependency::add(&conn, id_a, "trigger_b").unwrap();

        // Create a changeset with both triggers pending
        conn.execute("INSERT INTO changesets (description) VALUES ('test')", [])
            .unwrap();
        let changeset_id = conn.last_insert_rowid();

        let mut ct_b = ChangesetTrigger::new(changeset_id, id_b);
        ct_b.upsert(&conn).unwrap();
        let mut ct_a = ChangesetTrigger::new(changeset_id, id_a);
        ct_a.upsert(&conn).unwrap();

        let engine = TriggerEngine::new(&conn);
        let order = engine.get_execution_order(changeset_id).unwrap();

        assert_eq!(order.len(), 2);
        // B must come before A despite A having higher (lower number) priority
        assert_eq!(order[0].name, "trigger_b", "dependency must execute first");
        assert_eq!(order[1].name, "trigger_a", "dependent must execute second");
    }

    #[test]
    fn test_execution_order_respects_priority_within_level() {
        // Triggers with no dependency relationship should be ordered by priority
        let (_temp, conn) = create_test_db();

        let mut t_low = Trigger::new(
            "zz_low_priority".to_string(),
            "/usr/lib/*".to_string(),
            "/bin/true".to_string(),
        )
        .unwrap();
        t_low.priority = 90;
        let id_low = t_low.insert(&conn).unwrap();

        let mut t_high = Trigger::new(
            "aa_high_priority".to_string(),
            "/usr/lib/*".to_string(),
            "/bin/true".to_string(),
        )
        .unwrap();
        t_high.priority = 10;
        let id_high = t_high.insert(&conn).unwrap();

        conn.execute("INSERT INTO changesets (description) VALUES ('test')", [])
            .unwrap();
        let changeset_id = conn.last_insert_rowid();

        let mut ct_low = ChangesetTrigger::new(changeset_id, id_low);
        ct_low.upsert(&conn).unwrap();
        let mut ct_high = ChangesetTrigger::new(changeset_id, id_high);
        ct_high.upsert(&conn).unwrap();

        let engine = TriggerEngine::new(&conn);
        let order = engine.get_execution_order(changeset_id).unwrap();

        assert_eq!(order.len(), 2);
        // Both at the same topological level, so priority wins (lower number first)
        assert_eq!(
            order[0].name, "aa_high_priority",
            "higher priority (lower number) should run first within same level"
        );
        assert_eq!(order[1].name, "zz_low_priority");
    }

    #[test]
    fn test_execution_order_rejects_dependency_cycles() {
        let (_temp, conn) = create_test_db();

        let mut trigger_a = Trigger::new(
            "trigger_a".to_string(),
            "/usr/lib/*".to_string(),
            "/bin/true".to_string(),
        )
        .unwrap();
        let id_a = trigger_a.insert(&conn).unwrap();
        let mut trigger_b = Trigger::new(
            "trigger_b".to_string(),
            "/usr/lib/*".to_string(),
            "/bin/true".to_string(),
        )
        .unwrap();
        let id_b = trigger_b.insert(&conn).unwrap();

        TriggerDependency::add(&conn, id_a, "trigger_b").unwrap();
        TriggerDependency::add(&conn, id_b, "trigger_a").unwrap();

        conn.execute("INSERT INTO changesets (description) VALUES ('test')", [])
            .unwrap();
        let changeset_id = conn.last_insert_rowid();
        ChangesetTrigger::new(changeset_id, id_a)
            .upsert(&conn)
            .unwrap();
        ChangesetTrigger::new(changeset_id, id_b)
            .upsert(&conn)
            .unwrap();

        let error = TriggerEngine::new(&conn)
            .get_execution_order(changeset_id)
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("dependency cycle"));
        assert!(message.contains("trigger_a"));
        assert!(message.contains("trigger_b"));
    }
}
