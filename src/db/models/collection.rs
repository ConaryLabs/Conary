// src/db/models/collection.rs

//! CollectionMember model - members of package collections/groups
//!
//! Collections are meta-packages that group other packages together.
//! This module provides the model for tracking collection membership.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// A member of a collection (meta-package)
#[derive(Debug, Clone)]
pub struct CollectionMember {
    pub id: Option<i64>,
    /// The collection (trove with type=collection) this member belongs to
    pub collection_id: i64,
    /// Name of the member package
    pub member_name: String,
    /// Optional version constraint for the member
    pub member_version: Option<String>,
    /// Whether this member is optional
    pub is_optional: bool,
}

impl CollectionMember {
    /// Create a new collection member
    pub fn new(collection_id: i64, member_name: String) -> Self {
        Self {
            id: None,
            collection_id,
            member_name,
            member_version: None,
            is_optional: false,
        }
    }

    /// Create with a version constraint
    pub fn with_version(mut self, version: String) -> Self {
        self.member_version = Some(version);
        self
    }

    /// Mark as optional member
    pub fn optional(mut self) -> Self {
        self.is_optional = true;
        self
    }

    /// Insert this member into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO collection_members (collection_id, member_name, member_version, is_optional)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                &self.collection_id,
                &self.member_name,
                &self.member_version,
                &self.is_optional,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all members of a collection
    pub fn find_by_collection(conn: &Connection, collection_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, collection_id, member_name, member_version, is_optional
             FROM collection_members WHERE collection_id = ?1
             ORDER BY member_name",
        )?;

        let members = stmt
            .query_map([collection_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(members)
    }

    /// Find all collections that contain a specific package
    pub fn find_collections_containing(conn: &Connection, package_name: &str) -> Result<Vec<i64>> {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT collection_id FROM collection_members WHERE member_name = ?1",
        )?;

        let ids = stmt
            .query_map([package_name], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(ids)
    }

    /// Remove a member from a collection
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM collection_members WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Remove all members from a collection
    pub fn delete_all_for_collection(conn: &Connection, collection_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM collection_members WHERE collection_id = ?1",
            [collection_id],
        )?;
        Ok(())
    }

    /// Check if a package is a member of a collection
    pub fn is_member(conn: &Connection, collection_id: i64, member_name: &str) -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM collection_members
             WHERE collection_id = ?1 AND member_name = ?2",
            params![collection_id, member_name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Get a specific member from a collection
    pub fn find_member(
        conn: &Connection,
        collection_id: i64,
        member_name: &str,
    ) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, collection_id, member_name, member_version, is_optional
             FROM collection_members
             WHERE collection_id = ?1 AND member_name = ?2",
        )?;

        let member = stmt
            .query_row(params![collection_id, member_name], Self::from_row)
            .optional()?;

        Ok(member)
    }

    /// Convert a database row to a CollectionMember
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            collection_id: row.get(1)?,
            member_name: row.get(2)?,
            member_version: row.get(3)?,
            is_optional: row.get(4)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                type TEXT NOT NULL
            );
            CREATE TABLE collection_members (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                collection_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
                member_name TEXT NOT NULL,
                member_version TEXT,
                is_optional INTEGER NOT NULL DEFAULT 0,
                UNIQUE(collection_id, member_name)
            );
            INSERT INTO troves (id, name, version, type) VALUES (1, 'dev-tools', '1.0', 'collection');
            ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_insert_and_find() {
        let conn = setup_test_db();

        let mut member = CollectionMember::new(1, "gcc".to_string());
        member.insert(&conn).unwrap();

        let members = CollectionMember::find_by_collection(&conn, 1).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].member_name, "gcc");
    }

    #[test]
    fn test_find_collections_containing() {
        let conn = setup_test_db();

        let mut m1 = CollectionMember::new(1, "gcc".to_string());
        m1.insert(&conn).unwrap();

        let collections = CollectionMember::find_collections_containing(&conn, "gcc").unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0], 1);
    }

    #[test]
    fn test_is_member() {
        let conn = setup_test_db();

        let mut member = CollectionMember::new(1, "gcc".to_string());
        member.insert(&conn).unwrap();

        assert!(CollectionMember::is_member(&conn, 1, "gcc").unwrap());
        assert!(!CollectionMember::is_member(&conn, 1, "clang").unwrap());
    }

    #[test]
    fn test_optional_member() {
        let conn = setup_test_db();

        let mut member = CollectionMember::new(1, "clang".to_string()).optional();
        member.insert(&conn).unwrap();

        let members = CollectionMember::find_by_collection(&conn, 1).unwrap();
        assert!(members[0].is_optional);
    }
}
