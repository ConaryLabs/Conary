// conary-core/src/db/models/settings.rs

//! Key-value settings storage

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension};

/// Get a setting value by key
pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
    let result = stmt.query_row([key], |row| row.get(0)).optional()?;
    Ok(result)
}

/// Set a setting value (upsert)
pub fn set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

/// Delete a setting
pub fn delete(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;

    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn test_settings_get_set_delete() {
        let conn = create_test_db();
        assert_eq!(get(&conn, "update-channel").unwrap(), None);
        set(&conn, "update-channel", "https://example.com").unwrap();
        assert_eq!(
            get(&conn, "update-channel").unwrap(),
            Some("https://example.com".to_string())
        );
        delete(&conn, "update-channel").unwrap();
        assert_eq!(get(&conn, "update-channel").unwrap(), None);
    }

    #[test]
    fn test_settings_upsert() {
        let conn = create_test_db();
        set(&conn, "key1", "value1").unwrap();
        set(&conn, "key1", "value2").unwrap();
        assert_eq!(get(&conn, "key1").unwrap(), Some("value2".to_string()));
    }
}
