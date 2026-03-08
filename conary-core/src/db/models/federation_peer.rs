// conary-core/src/db/models/federation_peer.rs

//! Federation peer model - manages peer entries in the federation_peers table

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

/// A federation peer record.
#[derive(Debug, Clone, Serialize)]
pub struct FederationPeer {
    pub id: String,
    pub endpoint: String,
    pub node_name: Option<String>,
    pub tier: String,
    pub first_seen: String,
    pub last_seen: String,
    pub latency_ms: i64,
    pub success_count: i64,
    pub failure_count: i64,
    pub consecutive_failures: i64,
    pub is_enabled: bool,
}

/// List all federation peers, ordered by node_name (nulls last), then endpoint.
pub fn list(conn: &Connection) -> Result<Vec<FederationPeer>> {
    let mut stmt = conn.prepare(
        "SELECT id, endpoint, node_name, tier, first_seen, last_seen, \
         latency_ms, success_count, failure_count, consecutive_failures, is_enabled \
         FROM federation_peers ORDER BY COALESCE(node_name, endpoint)",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(FederationPeer {
            id: row.get(0)?,
            endpoint: row.get(1)?,
            node_name: row.get(2)?,
            tier: row.get(3)?,
            first_seen: row.get(4)?,
            last_seen: row.get(5)?,
            latency_ms: row.get(6)?,
            success_count: row.get(7)?,
            failure_count: row.get(8)?,
            consecutive_failures: row.get(9)?,
            is_enabled: row.get(10)?,
        })
    })?;
    let mut peers = Vec::new();
    for row in rows {
        peers.push(row?);
    }
    Ok(peers)
}

/// Find a federation peer by its ID.
pub fn find_by_id(conn: &Connection, id: &str) -> Result<Option<FederationPeer>> {
    let mut stmt = conn.prepare(
        "SELECT id, endpoint, node_name, tier, first_seen, last_seen, \
         latency_ms, success_count, failure_count, consecutive_failures, is_enabled \
         FROM federation_peers WHERE id = ?1",
    )?;
    let result = stmt
        .query_row(params![id], |row| {
            Ok(FederationPeer {
                id: row.get(0)?,
                endpoint: row.get(1)?,
                node_name: row.get(2)?,
                tier: row.get(3)?,
                first_seen: row.get(4)?,
                last_seen: row.get(5)?,
                latency_ms: row.get(6)?,
                success_count: row.get(7)?,
                failure_count: row.get(8)?,
                consecutive_failures: row.get(9)?,
                is_enabled: row.get(10)?,
            })
        })
        .optional()?;
    Ok(result)
}

/// Insert a new federation peer.
#[allow(clippy::too_many_arguments)]
pub fn insert(
    conn: &Connection,
    id: &str,
    endpoint: &str,
    node_name: Option<&str>,
    tier: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO federation_peers (id, endpoint, node_name, tier) \
         VALUES (?1, ?2, ?3, ?4)",
        params![id, endpoint, node_name, tier],
    )?;
    Ok(())
}

/// Delete a federation peer by ID, returning whether a row was deleted.
pub fn delete(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM federation_peers WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_list() {
        let conn = test_conn();
        insert(
            &conn,
            "peer-alpha",
            "https://alpha.example.com",
            Some("Alpha Node"),
            "region_hub",
        )
        .unwrap();
        insert(
            &conn,
            "peer-beta",
            "https://beta.example.com",
            Some("Beta Node"),
            "leaf",
        )
        .unwrap();

        let peers = list(&conn).unwrap();
        assert_eq!(peers.len(), 2);
        // Ordered by COALESCE(node_name, endpoint)
        assert_eq!(peers[0].node_name, Some("Alpha Node".to_string()));
        assert_eq!(peers[1].node_name, Some("Beta Node".to_string()));
        assert_eq!(peers[0].id, "peer-alpha");
        assert_eq!(peers[0].endpoint, "https://alpha.example.com");
        assert_eq!(peers[0].tier, "region_hub");
        assert!(peers[0].is_enabled);
        assert_eq!(peers[0].latency_ms, 0);
        assert_eq!(peers[0].success_count, 0);
        assert_eq!(peers[0].failure_count, 0);
        assert_eq!(peers[0].consecutive_failures, 0);
        assert!(!peers[0].first_seen.is_empty());
    }

    #[test]
    fn test_find_by_id() {
        let conn = test_conn();
        insert(
            &conn,
            "peer-gamma",
            "https://gamma.example.com",
            Some("Gamma Node"),
            "cell_hub",
        )
        .unwrap();

        let found = find_by_id(&conn, "peer-gamma").unwrap().unwrap();
        assert_eq!(found.endpoint, "https://gamma.example.com");
        assert_eq!(found.node_name, Some("Gamma Node".to_string()));

        let missing = find_by_id(&conn, "nonexistent").unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn test_delete() {
        let conn = test_conn();
        insert(
            &conn,
            "peer-delta",
            "https://delta.example.com",
            None,
            "leaf",
        )
        .unwrap();

        assert!(delete(&conn, "peer-delta").unwrap());
        assert!(!delete(&conn, "peer-delta").unwrap());
        assert!(find_by_id(&conn, "peer-delta").unwrap().is_none());
    }
}
