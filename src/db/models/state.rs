// src/db/models/state.rs

//! System state snapshot model for full system state tracking

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// A snapshot of the complete system state at a point in time
#[derive(Debug, Clone)]
pub struct SystemState {
    pub id: Option<i64>,
    pub state_number: i64,
    pub summary: String,
    pub description: Option<String>,
    pub created_at: Option<String>,
    pub changeset_id: Option<i64>,
    pub is_active: bool,
    pub package_count: i64,
}

impl SystemState {
    /// Create a new system state
    pub fn new(state_number: i64, summary: String) -> Self {
        Self {
            id: None,
            state_number,
            summary,
            description: None,
            created_at: None,
            changeset_id: None,
            is_active: false,
            package_count: 0,
        }
    }

    /// Insert this state into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO system_states (state_number, summary, description, changeset_id, is_active, package_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                self.state_number,
                &self.summary,
                &self.description,
                self.changeset_id,
                self.is_active as i32,
                self.package_count
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a state by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, state_number, summary, description, created_at, changeset_id, is_active, package_count
             FROM system_states WHERE id = ?1",
        )?;

        let state = stmt.query_row([id], Self::from_row).optional()?;
        Ok(state)
    }

    /// Find a state by state number
    pub fn find_by_number(conn: &Connection, state_number: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, state_number, summary, description, created_at, changeset_id, is_active, package_count
             FROM system_states WHERE state_number = ?1",
        )?;

        let state = stmt.query_row([state_number], Self::from_row).optional()?;
        Ok(state)
    }

    /// Get the currently active state
    pub fn get_active(conn: &Connection) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, state_number, summary, description, created_at, changeset_id, is_active, package_count
             FROM system_states WHERE is_active = 1",
        )?;

        let state = stmt.query_row([], Self::from_row).optional()?;
        Ok(state)
    }

    /// Get the next state number
    pub fn next_state_number(conn: &Connection) -> Result<i64> {
        let max: Option<i64> = conn.query_row(
            "SELECT MAX(state_number) FROM system_states",
            [],
            |row| row.get(0),
        )?;

        Ok(max.unwrap_or(-1) + 1)
    }

    /// List all states ordered by state number (newest first)
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, state_number, summary, description, created_at, changeset_id, is_active, package_count
             FROM system_states ORDER BY state_number DESC",
        )?;

        let states = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(states)
    }

    /// List states with limit
    pub fn list_recent(conn: &Connection, limit: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, state_number, summary, description, created_at, changeset_id, is_active, package_count
             FROM system_states ORDER BY state_number DESC LIMIT ?1",
        )?;

        let states = stmt
            .query_map([limit], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(states)
    }

    /// Set this state as the active state (unsets all others)
    pub fn set_active(&self, conn: &Connection) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot set active without ID".to_string())
        })?;

        // Unset all active states
        conn.execute("UPDATE system_states SET is_active = 0 WHERE is_active = 1", [])?;

        // Set this one as active
        conn.execute(
            "UPDATE system_states SET is_active = 1 WHERE id = ?1",
            [id],
        )?;

        Ok(())
    }

    /// Delete a state by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM system_states WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete states older than a certain state number (for pruning)
    pub fn delete_older_than(conn: &Connection, state_number: i64, keep_active: bool) -> Result<i64> {
        let deleted = if keep_active {
            conn.execute(
                "DELETE FROM system_states WHERE state_number < ?1 AND is_active = 0",
                [state_number],
            )?
        } else {
            conn.execute(
                "DELETE FROM system_states WHERE state_number < ?1",
                [state_number],
            )?
        };

        Ok(deleted as i64)
    }

    /// Get members of this state
    pub fn get_members(&self, conn: &Connection) -> Result<Vec<StateMember>> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot get members without ID".to_string())
        })?;

        StateMember::find_by_state(conn, id)
    }

    /// Convert a database row to a SystemState
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let is_active_int: i32 = row.get(6)?;

        Ok(Self {
            id: Some(row.get(0)?),
            state_number: row.get(1)?,
            summary: row.get(2)?,
            description: row.get(3)?,
            created_at: row.get(4)?,
            changeset_id: row.get(5)?,
            is_active: is_active_int != 0,
            package_count: row.get(7)?,
        })
    }
}

/// A package member of a state snapshot
#[derive(Debug, Clone)]
pub struct StateMember {
    pub id: Option<i64>,
    pub state_id: i64,
    pub trove_name: String,
    pub trove_version: String,
    pub architecture: Option<String>,
    pub install_reason: String,
    pub selection_reason: Option<String>,
}

impl StateMember {
    /// Create a new state member
    pub fn new(
        state_id: i64,
        trove_name: String,
        trove_version: String,
    ) -> Self {
        Self {
            id: None,
            state_id,
            trove_name,
            trove_version,
            architecture: None,
            install_reason: "explicit".to_string(),
            selection_reason: None,
        }
    }

    /// Insert this member into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO state_members (state_id, trove_name, trove_version, architecture, install_reason, selection_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                self.state_id,
                &self.trove_name,
                &self.trove_version,
                &self.architecture,
                &self.install_reason,
                &self.selection_reason
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all members of a state
    pub fn find_by_state(conn: &Connection, state_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, state_id, trove_name, trove_version, architecture, install_reason, selection_reason
             FROM state_members WHERE state_id = ?1 ORDER BY trove_name",
        )?;

        let members = stmt
            .query_map([state_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(members)
    }

    /// Convert a database row to a StateMember
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            state_id: row.get(1)?,
            trove_name: row.get(2)?,
            trove_version: row.get(3)?,
            architecture: row.get(4)?,
            install_reason: row.get(5)?,
            selection_reason: row.get(6)?,
        })
    }
}

/// Result of comparing two states
#[derive(Debug, Clone)]
pub struct StateDiff {
    /// Packages added in the newer state
    pub added: Vec<StateMember>,
    /// Packages removed in the newer state
    pub removed: Vec<StateMember>,
    /// Packages with version changes (old version, new version)
    pub upgraded: Vec<(StateMember, StateMember)>,
}

impl StateDiff {
    /// Compare two states and return the diff
    pub fn compare(conn: &Connection, from_state_id: i64, to_state_id: i64) -> Result<Self> {
        let from_members = StateMember::find_by_state(conn, from_state_id)?;
        let to_members = StateMember::find_by_state(conn, to_state_id)?;

        // Build lookup maps by package name
        let from_map: std::collections::HashMap<&str, &StateMember> = from_members
            .iter()
            .map(|m| (m.trove_name.as_str(), m))
            .collect();

        let to_map: std::collections::HashMap<&str, &StateMember> = to_members
            .iter()
            .map(|m| (m.trove_name.as_str(), m))
            .collect();

        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut upgraded = Vec::new();

        // Find added and upgraded packages
        for member in &to_members {
            if let Some(old_member) = from_map.get(member.trove_name.as_str()) {
                if old_member.trove_version != member.trove_version {
                    upgraded.push(((*old_member).clone(), member.clone()));
                }
            } else {
                added.push(member.clone());
            }
        }

        // Find removed packages
        for member in &from_members {
            if !to_map.contains_key(member.trove_name.as_str()) {
                removed.push(member.clone());
            }
        }

        Ok(Self {
            added,
            removed,
            upgraded,
        })
    }

    /// Check if the diff is empty (no changes)
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.upgraded.is_empty()
    }

    /// Total number of changes
    pub fn change_count(&self) -> usize {
        self.added.len() + self.removed.len() + self.upgraded.len()
    }
}

/// Engine for creating and managing system states
pub struct StateEngine<'a> {
    conn: &'a Connection,
}

impl<'a> StateEngine<'a> {
    /// Create a new state engine
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Create a new state snapshot from current system
    pub fn create_snapshot(&self, summary: &str, description: Option<&str>, changeset_id: Option<i64>) -> Result<SystemState> {
        let state_number = SystemState::next_state_number(self.conn)?;

        // Count current packages
        let package_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM troves WHERE type = 'package'",
            [],
            |row| row.get(0),
        )?;

        // Create the state
        let mut state = SystemState::new(state_number, summary.to_string());
        state.description = description.map(String::from);
        state.changeset_id = changeset_id;
        state.package_count = package_count;
        let state_id = state.insert(self.conn)?;

        // Populate with current packages
        self.conn.execute(
            "INSERT INTO state_members (state_id, trove_name, trove_version, architecture, install_reason, selection_reason)
             SELECT ?1, name, version, architecture, install_reason, selection_reason
             FROM troves WHERE type = 'package'",
            [state_id],
        )?;

        // Set as active state
        state.set_active(self.conn)?;
        state.is_active = true;

        Ok(state)
    }

    /// Get operations needed to restore to a target state
    pub fn plan_restore(&self, target_state_id: i64) -> Result<RestorePlan> {
        // Get current active state
        let current = SystemState::get_active(self.conn)?
            .ok_or_else(|| crate::error::Error::InitError("No active state found".to_string()))?;

        let current_id = current.id
            .ok_or_else(|| crate::error::Error::InitError("Current state has no ID".to_string()))?;

        // Get the diff from current to target
        let diff = StateDiff::compare(self.conn, current_id, target_state_id)?;

        Ok(RestorePlan {
            from_state: current,
            to_state: SystemState::find_by_id(self.conn, target_state_id)?
                .ok_or_else(|| crate::error::Error::InitError("Target state not found".to_string()))?,
            to_remove: diff.removed,
            to_install: diff.added,
            to_upgrade: diff.upgraded,
        })
    }

    /// Prune old states, keeping only the most recent N states
    pub fn prune(&self, keep_count: i64) -> Result<i64> {
        // Get the state number threshold
        let threshold: Option<i64> = self.conn.query_row(
            "SELECT state_number FROM system_states
             ORDER BY state_number DESC
             LIMIT 1 OFFSET ?1",
            [keep_count - 1],
            |row| row.get(0),
        ).optional()?;

        if let Some(threshold) = threshold {
            SystemState::delete_older_than(self.conn, threshold, true)
        } else {
            Ok(0) // Not enough states to prune
        }
    }
}

/// Plan for restoring to a previous state
#[derive(Debug, Clone)]
pub struct RestorePlan {
    /// Current system state
    pub from_state: SystemState,
    /// Target state to restore to
    pub to_state: SystemState,
    /// Packages to remove (in current but not in target)
    pub to_remove: Vec<StateMember>,
    /// Packages to install (in target but not in current)
    pub to_install: Vec<StateMember>,
    /// Packages to upgrade/downgrade (different version)
    pub to_upgrade: Vec<(StateMember, StateMember)>,
}

impl RestorePlan {
    /// Check if no operations are needed
    pub fn is_empty(&self) -> bool {
        self.to_remove.is_empty() && self.to_install.is_empty() && self.to_upgrade.is_empty()
    }

    /// Total number of operations
    pub fn operation_count(&self) -> usize {
        self.to_remove.len() + self.to_install.len() + self.to_upgrade.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_system_state_crud() {
        let (_temp, conn) = create_test_db();

        let mut state = SystemState::new(1, "Test state".to_string());
        state.description = Some("Test description".to_string());
        state.package_count = 5;

        let id = state.insert(&conn).unwrap();
        assert!(id > 0);

        let found = SystemState::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.state_number, 1);
        assert_eq!(found.summary, "Test state");
        assert_eq!(found.package_count, 5);
    }

    #[test]
    fn test_system_state_active() {
        let (_temp, conn) = create_test_db();

        // Clear any initial state
        conn.execute("DELETE FROM system_states", []).unwrap();

        let mut state1 = SystemState::new(1, "State 1".to_string());
        state1.insert(&conn).unwrap();
        state1.set_active(&conn).unwrap();

        let mut state2 = SystemState::new(2, "State 2".to_string());
        state2.insert(&conn).unwrap();

        // State 1 should be active
        let active = SystemState::get_active(&conn).unwrap().unwrap();
        assert_eq!(active.state_number, 1);

        // Set state 2 as active
        state2.set_active(&conn).unwrap();
        let active = SystemState::get_active(&conn).unwrap().unwrap();
        assert_eq!(active.state_number, 2);
    }

    #[test]
    fn test_state_member_crud() {
        let (_temp, conn) = create_test_db();

        // Clear any initial state
        conn.execute("DELETE FROM system_states", []).unwrap();

        let mut state = SystemState::new(1, "Test state".to_string());
        let state_id = state.insert(&conn).unwrap();

        let mut member = StateMember::new(state_id, "nginx".to_string(), "1.24.0".to_string());
        member.architecture = Some("x86_64".to_string());
        member.insert(&conn).unwrap();

        let members = StateMember::find_by_state(&conn, state_id).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].trove_name, "nginx");
        assert_eq!(members[0].trove_version, "1.24.0");
    }

    #[test]
    fn test_state_diff() {
        let (_temp, conn) = create_test_db();

        // Clear any initial state
        conn.execute("DELETE FROM system_states", []).unwrap();

        // Create state 1 with packages A, B, C
        let mut state1 = SystemState::new(1, "State 1".to_string());
        let state1_id = state1.insert(&conn).unwrap();
        StateMember::new(state1_id, "pkg-a".to_string(), "1.0".to_string()).insert(&conn).unwrap();
        StateMember::new(state1_id, "pkg-b".to_string(), "1.0".to_string()).insert(&conn).unwrap();
        StateMember::new(state1_id, "pkg-c".to_string(), "1.0".to_string()).insert(&conn).unwrap();

        // Create state 2 with packages B (upgraded), C, D (new)
        let mut state2 = SystemState::new(2, "State 2".to_string());
        let state2_id = state2.insert(&conn).unwrap();
        StateMember::new(state2_id, "pkg-b".to_string(), "2.0".to_string()).insert(&conn).unwrap();
        StateMember::new(state2_id, "pkg-c".to_string(), "1.0".to_string()).insert(&conn).unwrap();
        StateMember::new(state2_id, "pkg-d".to_string(), "1.0".to_string()).insert(&conn).unwrap();

        let diff = StateDiff::compare(&conn, state1_id, state2_id).unwrap();

        // pkg-a was removed
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.removed[0].trove_name, "pkg-a");

        // pkg-d was added
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.added[0].trove_name, "pkg-d");

        // pkg-b was upgraded
        assert_eq!(diff.upgraded.len(), 1);
        assert_eq!(diff.upgraded[0].0.trove_version, "1.0");
        assert_eq!(diff.upgraded[0].1.trove_version, "2.0");
    }

    #[test]
    fn test_state_engine_snapshot() {
        let (_temp, conn) = create_test_db();

        // Clear any initial state and add test packages
        conn.execute("DELETE FROM system_states", []).unwrap();
        conn.execute(
            "INSERT INTO troves (name, version, type, architecture, install_reason)
             VALUES ('test-pkg', '1.0', 'package', 'x86_64', 'explicit')",
            [],
        ).unwrap();

        let engine = StateEngine::new(&conn);
        let state = engine.create_snapshot("Test snapshot", Some("Description"), None).unwrap();

        assert_eq!(state.state_number, 0); // First state after clearing
        assert_eq!(state.package_count, 1);
        assert!(state.is_active);

        let members = state.get_members(&conn).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].trove_name, "test-pkg");
    }
}
