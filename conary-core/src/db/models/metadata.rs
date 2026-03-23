// conary-core/src/db/models/metadata.rs

use rusqlite::{Connection, OptionalExtension};

/// Which metadata table to query. Using an enum instead of a raw `&str`
/// prevents SQL injection via the table-name position in the query.
#[derive(Debug, Clone, Copy)]
pub enum MetadataTable {
    Server,
    Client,
}

impl MetadataTable {
    fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server_metadata",
            Self::Client => "client_metadata",
        }
    }
}

/// Get a value from server_metadata or client_metadata.
pub fn get_metadata(
    conn: &Connection,
    table: MetadataTable,
    key: &str,
) -> rusqlite::Result<Option<String>> {
    let sql = format!("SELECT value FROM {} WHERE key = ?1", table.as_str());
    conn.query_row(&sql, [key], |row| row.get(0)).optional()
}

/// Set a value in server_metadata or client_metadata (upsert).
pub fn set_metadata(
    conn: &Connection,
    table: MetadataTable,
    key: &str,
    value: &str,
) -> rusqlite::Result<()> {
    let sql = format!(
        "INSERT OR REPLACE INTO {} (key, value) VALUES (?1, ?2)",
        table.as_str()
    );
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
            get_metadata(&conn, MetadataTable::Server, "missing").unwrap(),
            None
        );

        set_metadata(&conn, MetadataTable::Server, "test_key", "test_value").unwrap();
        assert_eq!(
            get_metadata(&conn, MetadataTable::Server, "test_key").unwrap(),
            Some("test_value".to_string())
        );

        // Upsert overwrites
        set_metadata(&conn, MetadataTable::Server, "test_key", "new_value").unwrap();
        assert_eq!(
            get_metadata(&conn, MetadataTable::Server, "test_key").unwrap(),
            Some("new_value".to_string())
        );
    }

    #[test]
    fn test_client_metadata() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        set_metadata(&conn, MetadataTable::Client, "etag", "W/\"v5\"").unwrap();
        assert_eq!(
            get_metadata(&conn, MetadataTable::Client, "etag").unwrap(),
            Some("W/\"v5\"".to_string())
        );
    }
}
