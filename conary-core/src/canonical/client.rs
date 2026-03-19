// conary-core/src/canonical/client.rs

//! Client-side canonical map fetching from Remi.
//!
//! Uses `reqwest::blocking` for CLI commands. Server-side callers must wrap
//! in `tokio::task::spawn_blocking` if called from async context.

use crate::db::models::{CanonicalPackage, PackageImplementation, get_metadata, set_metadata};
use crate::error::{Error, Result};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
struct CanonicalMapResponse {
    #[allow(dead_code)]
    version: u32,
    #[allow(dead_code)]
    generated_at: String,
    entries: Vec<CanonicalMapEntry>,
}

#[derive(Debug, Deserialize)]
struct CanonicalMapEntry {
    canonical: String,
    implementations: BTreeMap<String, String>,
}

/// Fetch the canonical map from a Remi endpoint.
/// Returns Ok(Some(count)) if new data was fetched, Ok(None) if 304, Err on failure.
pub fn fetch_canonical_map(conn: &Connection, endpoint: &str) -> Result<Option<usize>> {
    let url = format!("{}/v1/canonical/map", endpoint.trim_end_matches('/'));
    let etag = get_metadata(conn, "client_metadata", "canonical_etag")
        .unwrap_or(None);

    let client = reqwest::blocking::Client::builder()
        .user_agent("conary/0.6.0 (https://conary.io)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| Error::DownloadError(e.to_string()))?;

    let mut request = client.get(&url);
    if let Some(ref etag_val) = etag {
        request = request.header("If-None-Match", etag_val.as_str());
    }

    let response = request.send().map_err(|e| Error::DownloadError(e.to_string()))?;

    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(None);
    }

    if !response.status().is_success() {
        return Err(Error::DownloadError(format!(
            "canonical map fetch failed: HTTP {}",
            response.status()
        )));
    }

    let new_etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let body = response.text().map_err(|e| Error::DownloadError(e.to_string()))?;
    let count = ingest_canonical_map_json(conn, &body)?;

    if let Some(etag_val) = new_etag {
        let _ = set_metadata(conn, "client_metadata", "canonical_etag", &etag_val);
    }

    Ok(Some(count))
}

/// Parse a canonical map JSON response and replace the local canonical DB.
pub fn ingest_canonical_map_json(conn: &Connection, json: &str) -> Result<usize> {
    let map: CanonicalMapResponse =
        serde_json::from_str(json).map_err(|e| Error::ParseError(e.to_string()))?;

    let tx = conn.unchecked_transaction()?;

    // Full replace -- clear existing data
    tx.execute("DELETE FROM package_implementations", [])?;
    tx.execute("DELETE FROM canonical_packages", [])?;

    let mut count = 0;
    for entry in &map.entries {
        let mut canonical = CanonicalPackage::new(entry.canonical.clone(), "package".to_string());
        let id = canonical.insert_or_ignore(&tx)?;
        let canonical_id = match id {
            Some(cid) => cid,
            None => continue,
        };

        for (distro, distro_name) in &entry.implementations {
            let mut imp = PackageImplementation::new(
                canonical_id,
                distro.clone(),
                distro_name.clone(),
                "server".to_string(),
            );
            imp.insert_or_ignore(&tx)?;
        }
        count += 1;
    }

    tx.commit()?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ingest_canonical_map_response() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::migrate(&conn).unwrap();

        let json = r#"{
            "version": 5,
            "generated_at": "2026-03-19T00:00:00Z",
            "entries": [
                {
                    "canonical": "python",
                    "implementations": {"fedora": "python3", "arch": "python"}
                },
                {
                    "canonical": "curl",
                    "implementations": {"fedora": "curl", "arch": "curl", "ubuntu": "curl"}
                }
            ]
        }"#;

        let count = ingest_canonical_map_json(&conn, json).unwrap();
        assert_eq!(count, 2);

        let pkg = crate::db::models::CanonicalPackage::find_by_name(&conn, "python").unwrap();
        assert!(pkg.is_some());

        let pkg = pkg.unwrap();
        let impls = crate::db::models::PackageImplementation::find_by_canonical(&conn, pkg.id.unwrap()).unwrap();
        assert_eq!(impls.len(), 2);
    }

    #[test]
    fn test_ingest_replaces_existing_data() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::migrate(&conn).unwrap();

        let json1 = r#"{"version": 1, "generated_at": "2026-03-19", "entries": [{"canonical": "old-pkg", "implementations": {"fedora": "old"}}]}"#;
        let json2 = r#"{"version": 2, "generated_at": "2026-03-19", "entries": [{"canonical": "new-pkg", "implementations": {"arch": "new"}}]}"#;

        ingest_canonical_map_json(&conn, json1).unwrap();
        let count = ingest_canonical_map_json(&conn, json2).unwrap();
        assert_eq!(count, 1);

        // Old data should be gone
        let old = crate::db::models::CanonicalPackage::find_by_name(&conn, "old-pkg").unwrap();
        assert!(old.is_none());

        // New data should be present
        let new = crate::db::models::CanonicalPackage::find_by_name(&conn, "new-pkg").unwrap();
        assert!(new.is_some());
    }

    #[test]
    fn test_ingest_empty_map() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::migrate(&conn).unwrap();

        let json = r#"{"version": 1, "generated_at": "2026-03-19", "entries": []}"#;
        let count = ingest_canonical_map_json(&conn, json).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_ingest_invalid_json() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::migrate(&conn).unwrap();

        let result = ingest_canonical_map_json(&conn, "not json");
        assert!(result.is_err());
    }
}
