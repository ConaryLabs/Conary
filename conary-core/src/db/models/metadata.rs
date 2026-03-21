// conary-core/src/db/models/metadata.rs

use rusqlite::{Connection, OptionalExtension};

/// Get a value from server_metadata (or client_metadata — same schema).
pub fn get_metadata(conn: &Connection, table: &str, key: &str) -> rusqlite::Result<Option<String>> {
    let sql = format!("SELECT value FROM {table} WHERE key = ?1");
    conn.query_row(&sql, [key], |row| row.get(0)).optional()
}

/// Set a value in server_metadata or client_metadata (upsert).
pub fn set_metadata(
    conn: &Connection,
    table: &str,
    key: &str,
    value: &str,
) -> rusqlite::Result<()> {
    let sql = format!("INSERT OR REPLACE INTO {table} (key, value) VALUES (?1, ?2)");
    conn.execute(&sql, rusqlite::params![key, value])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

    #[test]
    fn test_metadata_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        assert_eq!(
            get_metadata(&conn, "server_metadata", "missing").unwrap(),
            None
        );

        set_metadata(&conn, "server_metadata", "test_key", "test_value").unwrap();
        assert_eq!(
            get_metadata(&conn, "server_metadata", "test_key").unwrap(),
            Some("test_value".to_string())
        );

        // Upsert overwrites
        set_metadata(&conn, "server_metadata", "test_key", "new_value").unwrap();
        assert_eq!(
            get_metadata(&conn, "server_metadata", "test_key").unwrap(),
            Some("new_value".to_string())
        );
    }

    #[test]
    fn test_client_metadata() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        set_metadata(&conn, "client_metadata", "etag", "W/\"v5\"").unwrap();
        assert_eq!(
            get_metadata(&conn, "client_metadata", "etag").unwrap(),
            Some("W/\"v5\"".to_string())
        );
    }
}
