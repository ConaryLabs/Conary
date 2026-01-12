// src/db/models/provide_entry.rs

//! ProvideEntry model - capabilities that packages offer
//!
//! This tracks what each package "provides" - the capabilities it offers
//! that can satisfy dependencies. This enables self-contained dependency
//! resolution without querying the host package manager.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// A capability that a package provides
#[derive(Debug, Clone)]
pub struct ProvideEntry {
    pub id: Option<i64>,
    pub trove_id: i64,
    /// The capability name (package name, virtual provide, library, or file path)
    pub capability: String,
    /// Optional version of this capability
    pub version: Option<String>,
}

impl ProvideEntry {
    /// Create a new ProvideEntry
    pub fn new(trove_id: i64, capability: String, version: Option<String>) -> Self {
        Self {
            id: None,
            trove_id,
            capability,
            version,
        }
    }

    /// Insert this provide into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO provides (trove_id, capability, version)
             VALUES (?1, ?2, ?3)",
            params![&self.trove_id, &self.capability, &self.version],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert or ignore if already exists (for idempotent imports)
    pub fn insert_or_ignore(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT OR IGNORE INTO provides (trove_id, capability, version)
             VALUES (?1, ?2, ?3)",
            params![&self.trove_id, &self.capability, &self.version],
        )?;

        // Get the ID (either new or existing)
        let id = conn.query_row(
            "SELECT id FROM provides WHERE trove_id = ?1 AND capability = ?2",
            params![&self.trove_id, &self.capability],
            |row| row.get(0),
        )?;

        self.id = Some(id);
        Ok(id)
    }

    /// Find a provide by capability name
    ///
    /// Returns the first trove that provides this capability
    pub fn find_by_capability(conn: &Connection, capability: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version
             FROM provides WHERE capability = ?1 LIMIT 1",
        )?;

        let provide = stmt.query_row([capability], Self::from_row).optional()?;
        Ok(provide)
    }

    /// Find all troves that provide a capability
    pub fn find_all_by_capability(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version
             FROM provides WHERE capability = ?1",
        )?;

        let provides = stmt
            .query_map([capability], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(provides)
    }

    /// Find all provides for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version
             FROM provides WHERE trove_id = ?1",
        )?;

        let provides = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(provides)
    }

    /// Check if a capability is provided by any installed package
    pub fn is_capability_satisfied(conn: &Connection, capability: &str) -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM provides WHERE capability = ?1",
            [capability],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Find provider with version constraint check
    ///
    /// Returns the trove name and version if the capability is satisfied
    pub fn find_satisfying_provider(
        conn: &Connection,
        capability: &str,
    ) -> Result<Option<(String, String)>> {
        // First try exact match
        let result = conn
            .query_row(
                "SELECT t.name, t.version
                 FROM provides p
                 JOIN troves t ON p.trove_id = t.id
                 WHERE p.capability = ?1
                 LIMIT 1",
                [capability],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        if result.is_some() {
            return Ok(result);
        }

        // Try prefix match for capabilities with version suffixes
        // e.g., "perl(Text::CharWidth)" should match "perl(Text::CharWidth) = 0.04"
        let prefix_pattern = format!("{} %", capability);
        let result = conn
            .query_row(
                "SELECT t.name, t.version
                 FROM provides p
                 JOIN troves t ON p.trove_id = t.id
                 WHERE p.capability LIKE ?1
                 LIMIT 1",
                [&prefix_pattern],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        if result.is_some() {
            return Ok(result);
        }

        // Try case-insensitive prefix match for cross-distro compatibility
        // e.g., perl(Text::Charwidth) should match perl(Text::CharWidth) = 0.04
        let lower_cap = capability.to_lowercase();
        let result = conn
            .query_row(
                "SELECT t.name, t.version
                 FROM provides p
                 JOIN troves t ON p.trove_id = t.id
                 WHERE LOWER(p.capability) LIKE ?1 || ' %' OR LOWER(p.capability) = ?1
                 LIMIT 1",
                [&lower_cap],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        Ok(result)
    }

    /// Search for capabilities matching a pattern (using SQL LIKE)
    pub fn search_capability(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version
             FROM provides WHERE capability LIKE ?1",
        )?;

        let provides = stmt
            .query_map([pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(provides)
    }

    /// Delete all provides for a trove
    pub fn delete_by_trove(conn: &Connection, trove_id: i64) -> Result<()> {
        conn.execute("DELETE FROM provides WHERE trove_id = ?1", [trove_id])?;
        Ok(())
    }

    /// Convert a database row to a ProvideEntry
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            capability: row.get(2)?,
            version: row.get(3)?,
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
                version TEXT NOT NULL
            );
            CREATE TABLE provides (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trove_id INTEGER NOT NULL REFERENCES troves(id),
                capability TEXT NOT NULL,
                version TEXT,
                UNIQUE(trove_id, capability)
            );
            CREATE INDEX idx_provides_capability ON provides(capability);

            INSERT INTO troves (id, name, version) VALUES (1, 'perl-Text-CharWidth', '0.04');
            ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_insert_and_find() {
        let conn = setup_test_db();

        let mut provide = ProvideEntry::new(1, "perl(Text::CharWidth)".to_string(), Some("0.04".to_string()));
        provide.insert(&conn).unwrap();

        let found = ProvideEntry::find_by_capability(&conn, "perl(Text::CharWidth)").unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.capability, "perl(Text::CharWidth)");
        assert_eq!(found.version, Some("0.04".to_string()));
    }

    #[test]
    fn test_is_capability_satisfied() {
        let conn = setup_test_db();

        let mut provide = ProvideEntry::new(1, "libc.so.6".to_string(), None);
        provide.insert(&conn).unwrap();

        assert!(ProvideEntry::is_capability_satisfied(&conn, "libc.so.6").unwrap());
        assert!(!ProvideEntry::is_capability_satisfied(&conn, "libfoo.so.1").unwrap());
    }

    #[test]
    fn test_search_capability() {
        let conn = setup_test_db();

        let mut p1 = ProvideEntry::new(1, "perl(Text::CharWidth)".to_string(), None);
        let mut p2 = ProvideEntry::new(1, "perl(Text::Wrap)".to_string(), None);
        p1.insert(&conn).unwrap();
        p2.insert(&conn).unwrap();

        let results = ProvideEntry::search_capability(&conn, "perl(Text::%)").unwrap();
        assert_eq!(results.len(), 2);
    }
}
