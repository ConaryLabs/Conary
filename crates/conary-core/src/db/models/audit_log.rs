// conary-core/src/db/models/audit_log.rs

//! Admin audit log model - tracks admin API operations

use crate::error::Result;
use rusqlite::{Connection, params};
use serde::Serialize;

/// A single audit log entry.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub timestamp: String,
    pub token_name: Option<String>,
    pub action: String,
    pub method: String,
    pub path: String,
    pub status_code: i32,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub source_ip: Option<String>,
    pub duration_ms: Option<i64>,
}

/// Insert a new audit log entry.
#[allow(clippy::too_many_arguments)]
pub fn insert(
    conn: &Connection,
    token_name: Option<&str>,
    action: &str,
    method: &str,
    path: &str,
    status_code: i32,
    request_body: Option<&str>,
    response_body: Option<&str>,
    source_ip: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO admin_audit_log \
         (token_name, action, method, path, status_code, request_body, response_body, source_ip, duration_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![token_name, action, method, path, status_code, request_body, response_body, source_ip, duration_ms],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Query audit log entries with optional filters.
///
/// Filters:
/// - `limit`: Max entries to return (default 50, max 500)
/// - `action`: Filter by action prefix (e.g., "repo" matches "repo.create", "repo.delete")
/// - `since`: Only entries after this ISO 8601 timestamp
/// - `token_name`: Filter by token name
pub fn query(
    conn: &Connection,
    limit: Option<i64>,
    action: Option<&str>,
    since: Option<&str>,
    token_name: Option<&str>,
) -> Result<Vec<AuditEntry>> {
    let limit = limit.unwrap_or(50).min(500);

    let mut sql = String::from(
        "SELECT id, timestamp, token_name, action, method, path, status_code, \
         request_body, response_body, source_ip, duration_ms \
         FROM admin_audit_log WHERE 1=1",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1;

    if let Some(action) = action {
        sql.push_str(&format!(" AND action LIKE ?{param_idx}"));
        param_values.push(Box::new(format!("{action}%")));
        param_idx += 1;
    }
    if let Some(since) = since {
        sql.push_str(&format!(" AND timestamp >= ?{param_idx}"));
        param_values.push(Box::new(since.to_string()));
        param_idx += 1;
    }
    if let Some(name) = token_name {
        sql.push_str(&format!(" AND token_name = ?{param_idx}"));
        param_values.push(Box::new(name.to_string()));
        param_idx += 1;
    }
    sql.push_str(&format!(" ORDER BY id DESC LIMIT ?{param_idx}"));
    param_values.push(Box::new(limit));

    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let entries = stmt
        .query_map(params_ref.as_slice(), |row| {
            Ok(AuditEntry {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                token_name: row.get(2)?,
                action: row.get(3)?,
                method: row.get(4)?,
                path: row.get(5)?,
                status_code: row.get(6)?,
                request_body: row.get(7)?,
                response_body: row.get(8)?,
                source_ip: row.get(9)?,
                duration_ms: row.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(entries)
}

/// Delete audit log entries older than the given timestamp.
///
/// Returns the number of entries deleted.
pub fn purge(conn: &Connection, before: &str) -> Result<usize> {
    let deleted = conn.execute("DELETE FROM admin_audit_log WHERE timestamp < ?1", [before])?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_query() {
        let conn = test_conn();
        let id = insert(
            &conn,
            Some("test-admin"),
            "token.create",
            "POST",
            "/v1/admin/tokens",
            201,
            Some(r#"{"name":"new-token"}"#),
            Some(r#"{"id":1}"#),
            Some("127.0.0.1"),
            Some(42),
        )
        .unwrap();
        assert!(id > 0);

        let entries = query(&conn, Some(10), None, None, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "token.create");
        assert_eq!(entries[0].token_name, Some("test-admin".to_string()));
        assert_eq!(entries[0].status_code, 201);
        assert_eq!(entries[0].duration_ms, Some(42));
    }

    #[test]
    fn test_query_filters() {
        let conn = test_conn();
        insert(
            &conn,
            Some("admin"),
            "token.create",
            "POST",
            "/v1/admin/tokens",
            201,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        insert(
            &conn,
            Some("admin"),
            "repo.create",
            "POST",
            "/v1/admin/repos",
            201,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        insert(
            &conn,
            Some("ci-reader"),
            "ci.list",
            "GET",
            "/v1/admin/ci/workflows",
            200,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Filter by action prefix
        let entries = query(&conn, None, Some("repo"), None, None).unwrap();
        assert_eq!(entries.len(), 1);

        // Filter by token_name
        let entries = query(&conn, None, None, None, Some("ci-reader")).unwrap();
        assert_eq!(entries.len(), 1);

        // Filter by since -- insert an old entry and verify it's excluded
        conn.execute(
            "INSERT INTO admin_audit_log (timestamp, action, method, path, status_code) \
             VALUES ('2020-01-01T00:00:00', 'old.action', 'GET', '/old', 200)",
            [],
        )
        .unwrap();
        // All 3 entries from above have recent timestamps; only the old one is before 2025
        let entries = query(&conn, None, None, Some("2025-01-01T00:00:00"), None).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_purge() {
        let conn = test_conn();
        // Insert with explicit old timestamp
        conn.execute(
            "INSERT INTO admin_audit_log (timestamp, action, method, path, status_code) \
             VALUES ('2020-01-01T00:00:00', 'old.action', 'GET', '/old', 200)",
            [],
        )
        .unwrap();
        insert(
            &conn,
            None,
            "new.action",
            "GET",
            "/new",
            200,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let deleted = purge(&conn, "2025-01-01T00:00:00").unwrap();
        assert_eq!(deleted, 1);

        let remaining = query(&conn, None, None, None, None).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].action, "new.action");
    }
}
