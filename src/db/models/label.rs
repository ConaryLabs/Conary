// src/db/models/label.rs

//! Label model for package provenance tracking
//!
//! Labels use the format `repository@namespace:tag` to track where packages
//! came from. This enables:
//! - Tracking package origin (which repository/branch)
//! - Label-based dependency resolution
//! - Branch-aware updates and rollbacks

use crate::error::Result;
use crate::label::Label as LabelSpec;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// Database representation of a label
#[derive(Debug, Clone)]
pub struct LabelEntry {
    pub id: Option<i64>,
    pub repository: String,
    pub namespace: String,
    pub tag: String,
    pub description: Option<String>,
    pub parent_label_id: Option<i64>,
    pub created_at: Option<String>,
    /// Repository to use for package resolution through this label (v30)
    pub repository_id: Option<i64>,
    /// Delegate resolution to another label (v30 federation)
    pub delegate_to_label_id: Option<i64>,
}

impl LabelEntry {
    /// Create a new label entry
    pub fn new(repository: String, namespace: String, tag: String) -> Self {
        Self {
            id: None,
            repository,
            namespace,
            tag,
            description: None,
            parent_label_id: None,
            created_at: None,
            repository_id: None,
            delegate_to_label_id: None,
        }
    }

    /// Create a label entry from a LabelSpec
    pub fn from_spec(spec: &LabelSpec) -> Self {
        Self {
            id: None,
            repository: spec.repository.clone(),
            namespace: spec.namespace.clone(),
            tag: spec.tag.clone(),
            description: None,
            parent_label_id: None,
            created_at: None,
            repository_id: None,
            delegate_to_label_id: None,
        }
    }

    /// Convert to a LabelSpec
    pub fn to_spec(&self) -> LabelSpec {
        LabelSpec::new(&self.repository, &self.namespace, &self.tag)
    }

    /// Insert this label into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO labels (repository, namespace, tag, description, parent_label_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.repository,
                &self.namespace,
                &self.tag,
                &self.description,
                &self.parent_label_id,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert or get existing label
    pub fn insert_or_get(&mut self, conn: &Connection) -> Result<i64> {
        // Try to find existing
        if let Some(existing) = Self::find_by_spec(conn, &self.repository, &self.namespace, &self.tag)? {
            self.id = existing.id;
            self.created_at = existing.created_at;
            return Ok(existing.id.expect("Label entry from database should have an id"));
        }

        // Insert new
        self.insert(conn)
    }

    /// Find a label by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository, namespace, tag, description, parent_label_id, created_at, repository_id, delegate_to_label_id
             FROM labels WHERE id = ?1",
        )?;

        let label = stmt.query_row([id], Self::from_row).optional()?;
        Ok(label)
    }

    /// Find a label by its components
    pub fn find_by_spec(
        conn: &Connection,
        repository: &str,
        namespace: &str,
        tag: &str,
    ) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository, namespace, tag, description, parent_label_id, created_at, repository_id, delegate_to_label_id
             FROM labels WHERE repository = ?1 AND namespace = ?2 AND tag = ?3",
        )?;

        let label = stmt.query_row([repository, namespace, tag], Self::from_row).optional()?;
        Ok(label)
    }

    /// Find a label by its string representation
    pub fn find_by_string(conn: &Connection, label_str: &str) -> Result<Option<Self>> {
        let spec = LabelSpec::parse(label_str)
            .map_err(|e| crate::error::Error::ParseError(e.to_string()))?;
        Self::find_by_spec(conn, &spec.repository, &spec.namespace, &spec.tag)
    }

    /// List all labels
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository, namespace, tag, description, parent_label_id, created_at, repository_id, delegate_to_label_id
             FROM labels ORDER BY repository, namespace, tag",
        )?;

        let labels = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(labels)
    }

    /// Find labels by repository
    pub fn find_by_repository(conn: &Connection, repository: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository, namespace, tag, description, parent_label_id, created_at, repository_id, delegate_to_label_id
             FROM labels WHERE repository = ?1 ORDER BY namespace, tag",
        )?;

        let labels = stmt
            .query_map([repository], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(labels)
    }

    /// Find labels by repository and namespace (all tags on a branch)
    pub fn find_by_branch(conn: &Connection, repository: &str, namespace: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository, namespace, tag, description, parent_label_id, created_at, repository_id, delegate_to_label_id
             FROM labels WHERE repository = ?1 AND namespace = ?2 ORDER BY tag",
        )?;

        let labels = stmt
            .query_map([repository, namespace], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(labels)
    }

    /// Search labels by pattern (LIKE query on full label string)
    pub fn search(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        let search_pattern = format!("%{pattern}%");
        let mut stmt = conn.prepare(
            "SELECT id, repository, namespace, tag, description, parent_label_id, created_at, repository_id, delegate_to_label_id
             FROM labels
             WHERE repository LIKE ?1 OR namespace LIKE ?1 OR tag LIKE ?1 OR description LIKE ?1
             ORDER BY repository, namespace, tag",
        )?;

        let labels = stmt
            .query_map([&search_pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(labels)
    }

    /// Update label description
    pub fn update_description(&self, conn: &Connection, description: Option<&str>) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot update label without ID".to_string())
        })?;

        conn.execute(
            "UPDATE labels SET description = ?1 WHERE id = ?2",
            params![description, id],
        )?;

        Ok(())
    }

    /// Set parent label for branch history tracking
    pub fn set_parent(&mut self, conn: &Connection, parent_id: Option<i64>) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot update label without ID".to_string())
        })?;

        conn.execute(
            "UPDATE labels SET parent_label_id = ?1 WHERE id = ?2",
            params![parent_id, id],
        )?;

        self.parent_label_id = parent_id;
        Ok(())
    }

    /// Get parent label (for branch history)
    pub fn parent(&self, conn: &Connection) -> Result<Option<Self>> {
        if let Some(parent_id) = self.parent_label_id {
            Self::find_by_id(conn, parent_id)
        } else {
            Ok(None)
        }
    }

    /// Get all child labels (labels that have this as parent)
    pub fn children(&self, conn: &Connection) -> Result<Vec<Self>> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot find children without label ID".to_string())
        })?;

        let mut stmt = conn.prepare(
            "SELECT id, repository, namespace, tag, description, parent_label_id, created_at, repository_id, delegate_to_label_id
             FROM labels WHERE parent_label_id = ?1 ORDER BY tag",
        )?;

        let labels = stmt
            .query_map([id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(labels)
    }

    // --- Federation support (v30) ---

    /// Set the repository for package resolution through this label
    pub fn set_repository(&mut self, conn: &Connection, repo_id: Option<i64>) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot update label without ID".to_string())
        })?;

        conn.execute(
            "UPDATE labels SET repository_id = ?1 WHERE id = ?2",
            params![repo_id, id],
        )?;

        self.repository_id = repo_id;
        Ok(())
    }

    /// Set the delegation target (another label to delegate resolution to)
    pub fn set_delegate(&mut self, conn: &Connection, delegate_label_id: Option<i64>) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot update label without ID".to_string())
        })?;

        conn.execute(
            "UPDATE labels SET delegate_to_label_id = ?1 WHERE id = ?2",
            params![delegate_label_id, id],
        )?;

        self.delegate_to_label_id = delegate_label_id;
        Ok(())
    }

    /// Get the delegation target label
    pub fn delegate_to(&self, conn: &Connection) -> Result<Option<Self>> {
        if let Some(delegate_id) = self.delegate_to_label_id {
            Self::find_by_id(conn, delegate_id)
        } else {
            Ok(None)
        }
    }

    /// Check if this label delegates to another
    pub fn is_delegation(&self) -> bool {
        self.delegate_to_label_id.is_some()
    }

    /// Find all labels that delegate to this label
    pub fn delegating_labels(&self, conn: &Connection) -> Result<Vec<Self>> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot find delegating labels without ID".to_string())
        })?;

        let mut stmt = conn.prepare(
            "SELECT id, repository, namespace, tag, description, parent_label_id, created_at, repository_id, delegate_to_label_id
             FROM labels WHERE delegate_to_label_id = ?1 ORDER BY repository, namespace, tag",
        )?;

        let labels = stmt
            .query_map([id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(labels)
    }

    /// Find labels by their linked repository
    pub fn find_by_linked_repository(conn: &Connection, repo_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository, namespace, tag, description, parent_label_id, created_at, repository_id, delegate_to_label_id
             FROM labels WHERE repository_id = ?1 ORDER BY repository, namespace, tag",
        )?;

        let labels = stmt
            .query_map([repo_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(labels)
    }

    /// Delete this label
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM labels WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Count packages using this label
    pub fn package_count(&self, conn: &Connection) -> Result<i64> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot count packages without label ID".to_string())
        })?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM troves WHERE label_id = ?1",
            [id],
            |row| row.get(0),
        )?;

        Ok(count)
    }

    /// Convert a database row to a LabelEntry
    /// Row columns: id, repository, namespace, tag, description, parent_label_id, created_at, repository_id, delegate_to_label_id
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            repository: row.get(1)?,
            namespace: row.get(2)?,
            tag: row.get(3)?,
            description: row.get(4)?,
            parent_label_id: row.get(5)?,
            created_at: row.get(6)?,
            // v30 fields - may not exist in older databases
            repository_id: row.get(7).unwrap_or(None),
            delegate_to_label_id: row.get(8).unwrap_or(None),
        })
    }
}

impl std::fmt::Display for LabelEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}:{}", self.repository, self.namespace, self.tag)
    }
}

/// Label path entry for resolution priority
#[derive(Debug, Clone)]
pub struct LabelPathEntry {
    pub id: Option<i64>,
    pub label_id: i64,
    pub priority: i32,
    pub enabled: bool,
}

impl LabelPathEntry {
    /// Create a new label path entry
    pub fn new(label_id: i64, priority: i32) -> Self {
        Self {
            id: None,
            label_id,
            priority,
            enabled: true,
        }
    }

    /// Insert this path entry into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO label_path (label_id, priority, enabled) VALUES (?1, ?2, ?3)",
            params![&self.label_id, &self.priority, self.enabled as i32],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert or update (upsert) the priority
    pub fn upsert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO label_path (label_id, priority, enabled)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(label_id) DO UPDATE SET priority = excluded.priority, enabled = excluded.enabled",
            params![&self.label_id, &self.priority, self.enabled as i32],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find path entry by label ID
    pub fn find_by_label(conn: &Connection, label_id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, label_id, priority, enabled FROM label_path WHERE label_id = ?1",
        )?;

        let entry = stmt.query_row([label_id], Self::from_row).optional()?;
        Ok(entry)
    }

    /// List all path entries ordered by priority
    pub fn list_ordered(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, label_id, priority, enabled
             FROM label_path ORDER BY priority ASC",
        )?;

        let entries = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    /// List enabled path entries ordered by priority
    pub fn list_enabled_ordered(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, label_id, priority, enabled
             FROM label_path WHERE enabled = 1 ORDER BY priority ASC",
        )?;

        let entries = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    /// Enable or disable a label in the path
    pub fn set_enabled(conn: &Connection, label_id: i64, enabled: bool) -> Result<()> {
        conn.execute(
            "UPDATE label_path SET enabled = ?1 WHERE label_id = ?2",
            params![enabled as i32, label_id],
        )?;
        Ok(())
    }

    /// Update priority
    pub fn set_priority(conn: &Connection, label_id: i64, priority: i32) -> Result<()> {
        conn.execute(
            "UPDATE label_path SET priority = ?1 WHERE label_id = ?2",
            params![priority, label_id],
        )?;
        Ok(())
    }

    /// Remove a label from the path
    pub fn delete(conn: &Connection, label_id: i64) -> Result<()> {
        conn.execute("DELETE FROM label_path WHERE label_id = ?1", [label_id])?;
        Ok(())
    }

    /// Get the label for this path entry
    pub fn label(&self, conn: &Connection) -> Result<Option<LabelEntry>> {
        LabelEntry::find_by_id(conn, self.label_id)
    }

    /// Convert a database row to a LabelPathEntry
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            label_id: row.get(1)?,
            priority: row.get(2)?,
            enabled: row.get::<_, i32>(3)? != 0,
        })
    }
}

/// Get the current label path as a list of LabelEntry objects
pub fn get_label_path(conn: &Connection) -> Result<Vec<LabelEntry>> {
    let entries = LabelPathEntry::list_enabled_ordered(conn)?;
    let mut labels = Vec::new();

    for entry in entries {
        if let Some(label) = entry.label(conn)? {
            labels.push(label);
        }
    }

    Ok(labels)
}

/// Add a label to the path at the specified priority
pub fn add_to_path(conn: &Connection, label_id: i64, priority: i32) -> Result<()> {
    let mut entry = LabelPathEntry::new(label_id, priority);
    entry.upsert(conn)?;
    Ok(())
}

/// Remove a label from the path
pub fn remove_from_path(conn: &Connection, label_id: i64) -> Result<()> {
    LabelPathEntry::delete(conn, label_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        crate::db::schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_label_crud() {
        let (_temp, conn) = create_test_db();

        // Create a label
        let mut label = LabelEntry::new(
            "conary.example.com".to_string(),
            "rpl".to_string(),
            "2".to_string(),
        );
        label.description = Some("rPath Linux 2".to_string());

        let id = label.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by ID
        let found = LabelEntry::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.repository, "conary.example.com");
        assert_eq!(found.namespace, "rpl");
        assert_eq!(found.tag, "2");
        assert_eq!(found.to_string(), "conary.example.com@rpl:2");

        // Find by spec
        let found = LabelEntry::find_by_spec(&conn, "conary.example.com", "rpl", "2")
            .unwrap()
            .unwrap();
        assert_eq!(found.id, Some(id));

        // Find by string
        let found = LabelEntry::find_by_string(&conn, "conary.example.com@rpl:2")
            .unwrap()
            .unwrap();
        assert_eq!(found.id, Some(id));
    }

    #[test]
    fn test_label_path() {
        let (_temp, conn) = create_test_db();

        // Create labels
        let mut label1 = LabelEntry::new("repo1".to_string(), "ns".to_string(), "1".to_string());
        let id1 = label1.insert(&conn).unwrap();

        let mut label2 = LabelEntry::new("repo2".to_string(), "ns".to_string(), "2".to_string());
        let id2 = label2.insert(&conn).unwrap();

        // Add to path with priorities
        add_to_path(&conn, id1, 0).unwrap(); // highest priority
        add_to_path(&conn, id2, 10).unwrap(); // lower priority

        // Get ordered path
        let path = get_label_path(&conn).unwrap();
        assert_eq!(path.len(), 2);
        assert_eq!(path[0].repository, "repo1"); // higher priority first
        assert_eq!(path[1].repository, "repo2");
    }

    #[test]
    fn test_label_parent_child() {
        let (_temp, conn) = create_test_db();

        // Create parent label
        let mut parent = LabelEntry::new("repo".to_string(), "ns".to_string(), "1".to_string());
        let parent_id = parent.insert(&conn).unwrap();

        // Create child label
        let mut child = LabelEntry::new("repo".to_string(), "ns".to_string(), "1.1".to_string());
        child.parent_label_id = Some(parent_id);
        child.insert(&conn).unwrap();

        // Get parent from child
        let parent_found = child.parent(&conn).unwrap().unwrap();
        assert_eq!(parent_found.tag, "1");

        // Get children from parent
        let children = parent.children(&conn).unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].tag, "1.1");
    }

    #[test]
    fn test_label_delegation() {
        let (_temp, conn) = create_test_db();

        // Create two labels
        let mut source = LabelEntry::new("local".to_string(), "devel".to_string(), "main".to_string());
        let source_id = source.insert(&conn).unwrap();

        let mut target = LabelEntry::new("fedora".to_string(), "f41".to_string(), "stable".to_string());
        let target_id = target.insert(&conn).unwrap();

        // Set up delegation
        source.set_delegate(&conn, Some(target_id)).unwrap();

        // Verify delegation is set
        let found = LabelEntry::find_by_id(&conn, source_id).unwrap().unwrap();
        assert_eq!(found.delegate_to_label_id, Some(target_id));
        assert!(found.is_delegation());

        // Get delegation target
        let delegate = found.delegate_to(&conn).unwrap().unwrap();
        assert_eq!(delegate.repository, "fedora");
        assert_eq!(delegate.namespace, "f41");

        // Find labels that delegate to target
        let delegating = target.delegating_labels(&conn).unwrap();
        assert_eq!(delegating.len(), 1);
        assert_eq!(delegating[0].repository, "local");

        // Remove delegation
        source.set_delegate(&conn, None).unwrap();
        let found = LabelEntry::find_by_id(&conn, source_id).unwrap().unwrap();
        assert!(!found.is_delegation());
    }

    #[test]
    fn test_label_repository_link() {
        let (_temp, conn) = create_test_db();

        // Create a repository first
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('test-repo', 'https://example.com', 1, 10)",
            [],
        ).unwrap();
        let repo_id: i64 = conn.last_insert_rowid();

        // Create a label
        let mut label = LabelEntry::new("fedora".to_string(), "f41".to_string(), "stable".to_string());
        let label_id = label.insert(&conn).unwrap();

        // Link label to repository
        label.set_repository(&conn, Some(repo_id)).unwrap();

        // Verify link is set
        let found = LabelEntry::find_by_id(&conn, label_id).unwrap().unwrap();
        assert_eq!(found.repository_id, Some(repo_id));

        // Find labels by repository
        let labels = LabelEntry::find_by_linked_repository(&conn, repo_id).unwrap();
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].repository, "fedora");

        // Unlink
        label.set_repository(&conn, None).unwrap();
        let found = LabelEntry::find_by_id(&conn, label_id).unwrap().unwrap();
        assert_eq!(found.repository_id, None);
    }
}
