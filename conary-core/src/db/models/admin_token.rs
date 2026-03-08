// conary-core/src/db/models/admin_token.rs

//! Admin API token storage and management

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

/// An admin API token record
#[derive(Debug, Clone, Serialize)]
pub struct AdminToken {
    pub id: i64,
    pub name: String,
    pub token_hash: String,
    pub scopes: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// Create a new admin token, returning the row ID
pub fn create(conn: &Connection, name: &str, token_hash: &str, scopes: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO admin_tokens (name, token_hash, scopes) VALUES (?1, ?2, ?3)",
        params![name, token_hash, scopes],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Find an admin token by its hash
pub fn find_by_hash(conn: &Connection, token_hash: &str) -> Result<Option<AdminToken>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, token_hash, scopes, created_at, last_used_at
         FROM admin_tokens WHERE token_hash = ?1",
    )?;
    let result = stmt
        .query_row(params![token_hash], |row| {
            Ok(AdminToken {
                id: row.get(0)?,
                name: row.get(1)?,
                token_hash: row.get(2)?,
                scopes: row.get(3)?,
                created_at: row.get(4)?,
                last_used_at: row.get(5)?,
            })
        })
        .optional()?;
    Ok(result)
}

/// List all admin tokens (hashes are redacted for safety)
pub fn list(conn: &Connection) -> Result<Vec<AdminToken>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, scopes, created_at, last_used_at FROM admin_tokens ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AdminToken {
            id: row.get(0)?,
            name: row.get(1)?,
            token_hash: String::new(),
            scopes: row.get(2)?,
            created_at: row.get(3)?,
            last_used_at: row.get(4)?,
        })
    })?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// Delete an admin token by ID, returning true if a row was deleted
pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    let affected = conn.execute("DELETE FROM admin_tokens WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

/// Update last_used_at to the current time
pub fn touch(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE admin_tokens SET last_used_at = datetime('now') WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn test_admin_token_create_and_find_by_hash() {
        let conn = test_db();
        let id = create(&conn, "deploy-key", "sha256:abc123", "admin").unwrap();
        assert!(id > 0);

        let token = find_by_hash(&conn, "sha256:abc123").unwrap().unwrap();
        assert_eq!(token.id, id);
        assert_eq!(token.name, "deploy-key");
        assert_eq!(token.token_hash, "sha256:abc123");
        assert_eq!(token.scopes, "admin");
        assert!(!token.created_at.is_empty());
        assert!(token.last_used_at.is_none());
    }

    #[test]
    fn test_admin_token_find_by_hash_not_found() {
        let conn = test_db();
        let result = find_by_hash(&conn, "sha256:nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_admin_token_list_tokens() {
        let conn = test_db();
        create(&conn, "key-one", "sha256:hash1", "admin").unwrap();
        create(&conn, "key-two", "sha256:hash2", "read-only").unwrap();

        let tokens = list(&conn).unwrap();
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].name, "key-one");
        assert_eq!(tokens[1].name, "key-two");
        // Hashes must be redacted
        assert!(tokens[0].token_hash.is_empty());
        assert!(tokens[1].token_hash.is_empty());
    }

    #[test]
    fn test_admin_token_delete_token() {
        let conn = test_db();
        let id = create(&conn, "temp-key", "sha256:temp", "admin").unwrap();

        assert!(delete(&conn, id).unwrap());
        assert!(!delete(&conn, id).unwrap());
        assert!(find_by_hash(&conn, "sha256:temp").unwrap().is_none());
    }

    #[test]
    fn test_admin_token_touch_updates_last_used() {
        let conn = test_db();
        let id = create(&conn, "active-key", "sha256:active", "admin").unwrap();

        let token = find_by_hash(&conn, "sha256:active").unwrap().unwrap();
        assert!(token.last_used_at.is_none());

        touch(&conn, id).unwrap();

        let token = find_by_hash(&conn, "sha256:active").unwrap().unwrap();
        assert!(token.last_used_at.is_some());
    }
}
