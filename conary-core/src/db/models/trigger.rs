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
//! - Built-in triggers for common system actions
//! - Per-changeset tracking of triggered handlers

use crate::error::Result;
use glob::Pattern;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::{HashMap, VecDeque};
use strum_macros::{AsRefStr, EnumString};
use tracing::{debug, warn};

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
    /// Whether this is a built-in system trigger
    pub builtin: bool,
    pub created_at: Option<String>,
}

impl Trigger {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, name, description, pattern, handler, priority, \
         enabled, builtin, created_at";

    /// Create a new trigger
    pub fn new(name: String, pattern: String, handler: String) -> Self {
        Self {
            id: None,
            name,
            description: None,
            pattern,
            handler,
            priority: 50,
            enabled: true,
            builtin: false,
            created_at: None,
        }
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
        conn.execute(
            "INSERT INTO triggers (name, description, pattern, handler, priority, enabled, builtin)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &self.name,
                &self.description,
                &self.pattern,
                &self.handler,
                self.priority,
                self.enabled,
                self.builtin,
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

    /// List built-in triggers
    pub fn list_builtin(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM triggers WHERE builtin = 1 ORDER BY priority, name",
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

    /// Delete a trigger (only non-builtin)
    pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
        let rows = conn.execute("DELETE FROM triggers WHERE id = ?1 AND builtin = 0", [id])?;
        Ok(rows > 0)
    }

    /// Convert a database row to a Trigger
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            description: row.get(2)?,
            pattern: row.get(3)?,
            handler: row.get(4)?,
            priority: row.get(5)?,
            enabled: row.get::<_, i32>(6)? != 0,
            builtin: row.get::<_, i32>(7)? != 0,
            created_at: row.get(8)?,
        })
    }

    /// Parse the pattern string into individual glob patterns
    pub fn patterns(&self) -> Vec<&str> {
        self.pattern.split(',').map(|s| s.trim()).collect()
    }

    /// Check if a file path matches any of this trigger's patterns
    pub fn matches(&self, path: &str) -> bool {
        for pattern_str in self.patterns() {
            if let Ok(pattern) = Pattern::new(pattern_str)
                && pattern.matches(path)
            {
                return true;
            }
        }
        false
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
        let rows = stmt.query_map(
            rusqlite::params_from_iter(trigger_ids.iter()),
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )?;
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
#[derive(Debug, Clone, PartialEq, Eq, AsRefStr, EnumString)]
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

    pub fn parse(s: &str) -> Self {
        s.parse().unwrap_or(TriggerStatus::Pending)
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
            "SELECT {} FROM changeset_triggers WHERE changeset_id = ?1 AND status = 'pending'",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let triggers = stmt
            .query_map([changeset_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(triggers)
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let status_str: String = row.get(3)?;
        Ok(Self {
            id: Some(row.get(0)?),
            changeset_id: row.get(1)?,
            trigger_id: row.get(2)?,
            status: TriggerStatus::parse(&status_str),
            matched_files: row.get(4)?,
            started_at: row.get(5)?,
            completed_at: row.get(6)?,
            output: row.get(7)?,
        })
    }
}

/// Trigger execution engine
pub struct TriggerEngine<'a> {
    conn: &'a Connection,
}

impl<'a> TriggerEngine<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Find all triggers that match the given file paths
    pub fn find_matching_triggers(
        &self,
        file_paths: &[String],
    ) -> Result<Vec<(Trigger, Vec<String>)>> {
        let triggers = Trigger::list_enabled(self.conn)?;
        let mut matches: HashMap<i64, (Trigger, Vec<String>)> = HashMap::new();

        for path in file_paths {
            for trigger in &triggers {
                if trigger.matches(path) {
                    let trigger_id = trigger.id.unwrap_or(0);
                    matches
                        .entry(trigger_id)
                        .or_insert_with(|| (trigger.clone(), Vec::new()))
                        .1
                        .push(path.clone());
                }
            }
        }

        Ok(matches.into_values().collect())
    }

    /// Record triggered handlers for a changeset
    pub fn record_triggers(
        &self,
        changeset_id: i64,
        file_paths: &[String],
    ) -> Result<Vec<Trigger>> {
        let matching = self.find_matching_triggers(file_paths)?;
        let mut triggered = Vec::new();

        for (trigger, matched_files) in matching {
            if let Some(trigger_id) = trigger.id {
                // Record the trigger with matched file count
                let mut ct = ChangesetTrigger::new(changeset_id, trigger_id);
                ct.matched_files = matched_files.len() as i32;
                ct.upsert(self.conn)?;

                debug!(
                    "Trigger '{}' matched {} files",
                    trigger.name,
                    matched_files.len()
                );
                triggered.push(trigger);
            }
        }

        Ok(triggered)
    }

    /// Get triggers for a changeset in execution order (topologically sorted)
    pub fn get_execution_order(&self, changeset_id: i64) -> Result<Vec<Trigger>> {
        // Get all pending triggers for this changeset
        let changeset_triggers = ChangesetTrigger::find_pending(self.conn, changeset_id)?;
        if changeset_triggers.is_empty() {
            return Ok(Vec::new());
        }

        // Batch-load all trigger details in a single query
        let trigger_ids: Vec<i64> = changeset_triggers.iter().map(|ct| ct.trigger_id).collect();
        let mut triggers: HashMap<String, Trigger> = Trigger::find_by_ids(self.conn, &trigger_ids)?
            .into_iter()
            .map(|t| (t.name.clone(), t))
            .collect();

        // Batch-load all dependencies in a single query
        let loaded_ids: Vec<i64> = triggers
            .values()
            .filter_map(|t| t.id)
            .collect();
        let all_deps = TriggerDependency::get_dependencies_batch(self.conn, &loaded_ids)?;

        // Build dependency graph
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

        for trigger in triggers.values() {
            in_degree.entry(trigger.name.clone()).or_insert(0);
            dependents.entry(trigger.name.clone()).or_default();
        }

        for trigger in triggers.values() {
            let trigger_id = match trigger.id {
                Some(id) => id,
                None => continue,
            };
            let deps = all_deps
                .get(&trigger_id)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            for dep in deps {
                // Only count edges to triggers we're actually running
                if triggers.contains_key(dep.as_str()) {
                    *in_degree.entry(trigger.name.clone()).or_insert(0) += 1;
                    dependents
                        .entry(dep.clone())
                        .or_default()
                        .push(trigger.name.clone());
                }
            }
        }

        // Topological sort using Kahn's algorithm with priority-aware level ordering.
        // Within each topological level (triggers whose dependencies are all satisfied
        // at the same point), we sort by (priority, name) so lower priority runs first.
        let mut sorted = Vec::new();
        let mut ready: Vec<String> = in_degree
            .iter()
            .filter(|&(_, &degree)| degree == 0)
            .map(|(name, _)| name.clone())
            .collect();
        ready.sort_by(|a, b| {
            let pa = triggers.get(a).map_or(i32::MAX, |t| t.priority);
            let pb = triggers.get(b).map_or(i32::MAX, |t| t.priority);
            pa.cmp(&pb).then(a.cmp(b))
        });
        let mut queue: VecDeque<String> = ready.into_iter().collect();

        while let Some(name) = queue.pop_front() {
            // Collect newly-ready triggers from this node's dependents
            let mut newly_ready = Vec::new();

            if let Some(deps) = dependents.get(&name) {
                for dependent in deps {
                    if let Some(degree) = in_degree.get_mut(dependent) {
                        *degree -= 1;
                        if *degree == 0 {
                            newly_ready.push(dependent.clone());
                        }
                    }
                }
            }

            if let Some(trigger) = triggers.remove(&name) {
                sorted.push(trigger);
            }

            // Sort newly-ready triggers by priority, then name, and insert them
            // at the back of the queue in that order. This preserves topological
            // correctness while giving priority a voice within the same level.
            newly_ready.sort_by(|a, b| {
                let pa = triggers.get(a).map_or(i32::MAX, |t| t.priority);
                let pb = triggers.get(b).map_or(i32::MAX, |t| t.priority);
                pa.cmp(&pb).then(a.cmp(b))
            });
            for r in newly_ready {
                queue.push_back(r);
            }
        }

        // Check for cycles
        if sorted.len() != changeset_triggers.len() {
            warn!("Circular dependency detected in triggers, using priority order fallback");
            // Fall back to remaining triggers in priority order
            let mut remaining: Vec<Trigger> = triggers.into_values().collect();
            remaining.sort_by(|a, b| a.priority.cmp(&b.priority));
            sorted.extend(remaining);
        }

        Ok(sorted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                builtin INTEGER NOT NULL DEFAULT 0,
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
                status TEXT NOT NULL DEFAULT 'pending',
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
        );
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
    }

    #[test]
    fn test_trigger_pattern_matching() {
        let trigger = Trigger::new(
            "ldconfig".to_string(),
            "/usr/lib/*.so*,/usr/lib64/*.so*".to_string(),
            "/sbin/ldconfig".to_string(),
        );

        // Should match
        assert!(trigger.matches("/usr/lib/libssl.so.3"));
        assert!(trigger.matches("/usr/lib64/libc.so.6"));
        assert!(trigger.matches("/usr/lib/libfoo.so"));

        // Should not match
        assert!(!trigger.matches("/usr/bin/ls"));
        assert!(!trigger.matches("/etc/passwd"));
        assert!(!trigger.matches("/usr/lib/pkgconfig/foo.pc"));
    }

    #[test]
    fn test_trigger_dependencies() {
        let (_temp, conn) = create_test_db();

        // Create triggers
        let mut trigger1 = Trigger::new(
            "sysusers".to_string(),
            "/usr/lib/sysusers.d/*".to_string(),
            "systemd-sysusers".to_string(),
        );
        let mut trigger2 = Trigger::new(
            "tmpfiles".to_string(),
            "/usr/lib/tmpfiles.d/*".to_string(),
            "systemd-tmpfiles".to_string(),
        );

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
        let mut trigger = Trigger::new("test".to_string(), "/*".to_string(), "true".to_string());
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
        );
        let mut t2 = Trigger::new(
            "icons".to_string(),
            "/usr/share/icons/*".to_string(),
            "gtk-update-icon-cache".to_string(),
        );
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
        );
        trigger_b.priority = 90;
        let id_b = trigger_b.insert(&conn).unwrap();

        // Create trigger A (high priority = would run first by priority alone, priority 10)
        // but A depends on B, so B must run first
        let mut trigger_a = Trigger::new(
            "trigger_a".to_string(),
            "/usr/lib/*".to_string(),
            "/bin/true".to_string(),
        );
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
        );
        t_low.priority = 90;
        let id_low = t_low.insert(&conn).unwrap();

        let mut t_high = Trigger::new(
            "aa_high_priority".to_string(),
            "/usr/lib/*".to_string(),
            "/bin/true".to_string(),
        );
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
}
