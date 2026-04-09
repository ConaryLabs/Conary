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

impl FederationPeer {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, endpoint, node_name, tier, first_seen, last_seen, \
         latency_ms, success_count, failure_count, consecutive_failures, is_enabled";

    /// Convert a database row to a FederationPeer
    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Self {
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
    }
}

/// List all federation peers, ordered by node_name (nulls last), then endpoint.
pub fn list(conn: &Connection) -> Result<Vec<FederationPeer>> {
    let sql = format!(
        "SELECT {} FROM federation_peers ORDER BY COALESCE(node_name, endpoint)",
        FederationPeer::COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let peers = stmt
        .query_map([], FederationPeer::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(peers)
}

/// List enabled federation peers for a given tier ordered for substituter lookup.
pub fn list_enabled_for_tier(conn: &Connection, tier: &str) -> Result<Vec<FederationPeer>> {
    let sql = format!(
        "SELECT {} FROM federation_peers \
         WHERE tier = ?1 AND is_enabled = 1 \
         ORDER BY latency_ms ASC, success_count DESC, endpoint ASC",
        FederationPeer::COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let peers = stmt
        .query_map(params![tier], FederationPeer::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(peers)
}

/// Find a federation peer by its ID.
pub fn find_by_id(conn: &Connection, id: &str) -> Result<Option<FederationPeer>> {
    let sql = format!(
        "SELECT {} FROM federation_peers WHERE id = ?1",
        FederationPeer::COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let result = stmt
        .query_row(params![id], FederationPeer::from_row)
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

/// Record a successful federation peer fetch.
pub fn record_success(conn: &Connection, id: &str, latency_ms: i64) -> Result<()> {
    conn.execute(
        "UPDATE federation_peers \
         SET success_count = success_count + 1, \
             consecutive_failures = 0, \
             latency_ms = ?2 \
         WHERE id = ?1",
        params![id, latency_ms],
    )?;
    Ok(())
}

/// Record a failed federation peer fetch.
pub fn record_failure(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE federation_peers \
         SET failure_count = failure_count + 1, \
             consecutive_failures = consecutive_failures + 1 \
         WHERE id = ?1",
        params![id],
    )?;
    Ok(())
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

    #[test]
    fn test_list_enabled_for_tier_orders_by_latency_then_success() {
        let conn = test_conn();
        insert(
            &conn,
            "peer-slow",
            "https://slow.example.com",
            Some("Slow"),
            "leaf",
        )
        .unwrap();
        insert(
            &conn,
            "peer-fast-low-success",
            "https://fast-low.example.com",
            Some("Fast Low"),
            "leaf",
        )
        .unwrap();
        insert(
            &conn,
            "peer-fast-high-success",
            "https://fast-high.example.com",
            Some("Fast High"),
            "leaf",
        )
        .unwrap();

        conn.execute(
            "UPDATE federation_peers SET latency_ms = 200, success_count = 1 WHERE id = 'peer-slow'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE federation_peers SET latency_ms = 10, success_count = 1 WHERE id = 'peer-fast-low-success'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE federation_peers SET latency_ms = 10, success_count = 5 WHERE id = 'peer-fast-high-success'",
            [],
        )
        .unwrap();

        let peers = list_enabled_for_tier(&conn, "leaf").unwrap();
        let ids: Vec<_> = peers.iter().map(|peer| peer.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "peer-fast-high-success",
                "peer-fast-low-success",
                "peer-slow"
            ]
        );
    }

    #[test]
    fn test_list_enabled_for_tier_skips_disabled_peers() {
        let conn = test_conn();
        insert(
            &conn,
            "peer-enabled",
            "https://enabled.example.com",
            Some("Enabled"),
            "leaf",
        )
        .unwrap();
        insert(
            &conn,
            "peer-disabled",
            "https://disabled.example.com",
            Some("Disabled"),
            "leaf",
        )
        .unwrap();
        conn.execute(
            "UPDATE federation_peers SET is_enabled = 0 WHERE id = 'peer-disabled'",
            [],
        )
        .unwrap();

        let peers = list_enabled_for_tier(&conn, "leaf").unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].id, "peer-enabled");
    }

    #[test]
    fn test_record_success_updates_latency_and_resets_consecutive_failures() {
        let conn = test_conn();
        insert(
            &conn,
            "peer-ok",
            "https://ok.example.com",
            Some("Okay"),
            "leaf",
        )
        .unwrap();
        conn.execute(
            "UPDATE federation_peers SET success_count = 2, consecutive_failures = 4, latency_ms = 999 WHERE id = 'peer-ok'",
            [],
        )
        .unwrap();

        record_success(&conn, "peer-ok", 42).unwrap();

        let peer = find_by_id(&conn, "peer-ok").unwrap().unwrap();
        assert_eq!(peer.success_count, 3);
        assert_eq!(peer.consecutive_failures, 0);
        assert_eq!(peer.latency_ms, 42);
    }

    #[test]
    fn test_record_failure_increments_failure_counters() {
        let conn = test_conn();
        insert(
            &conn,
            "peer-bad",
            "https://bad.example.com",
            Some("Bad"),
            "leaf",
        )
        .unwrap();
        conn.execute(
            "UPDATE federation_peers SET failure_count = 2, consecutive_failures = 3 WHERE id = 'peer-bad'",
            [],
        )
        .unwrap();

        record_failure(&conn, "peer-bad").unwrap();

        let peer = find_by_id(&conn, "peer-bad").unwrap().unwrap();
        assert_eq!(peer.failure_count, 3);
        assert_eq!(peer.consecutive_failures, 4);
    }
}
