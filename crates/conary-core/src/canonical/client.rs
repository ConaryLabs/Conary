// conary-core/src/canonical/client.rs

//! Client-side canonical map fetching from Remi.

use crate::canonical::exchange::{expected_checksum, parse_snapshot, replace_remi_snapshot};
use crate::db::models::{MetadataTable, get_metadata, set_metadata};
use crate::error::{Error, Result};
use crate::repository::{MAX_BYTES_RESPONSE_SIZE, read_response_bytes_with_limit};
use rusqlite::Connection;

/// Fetch the canonical map from a Remi endpoint.
/// Returns Ok(Some(count)) if new data was fetched, Ok(None) if 304, Err on failure.
pub async fn fetch_canonical_map(conn: &Connection, endpoint: &str) -> Result<Option<usize>> {
    let url = format!("{}/v1/canonical/map", endpoint.trim_end_matches('/'));
    let etag = get_metadata(conn, MetadataTable::Client, "canonical_etag")?;

    let client = reqwest::Client::builder()
        .user_agent(concat!(
            "conary/",
            env!("CARGO_PKG_VERSION"),
            " (https://conary.io)"
        ))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| Error::DownloadError(e.to_string()))?;

    let mut request = client.get(&url);
    if let Some(ref etag_val) = etag {
        request = request.header("If-None-Match", etag_val.as_str());
    }

    let response = request
        .send()
        .await
        .map_err(|e| Error::DownloadError(e.to_string()))?;

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
    let expected_checksum = expected_checksum(response.headers())?;

    let body = read_response_bytes_with_limit(response, MAX_BYTES_RESPONSE_SIZE, &url).await?;
    crate::hash::verify_sha256(&body, &expected_checksum).map_err(|e| Error::ChecksumMismatch {
        expected: e.expected,
        actual: e.actual,
    })?;
    let count = ingest_canonical_map_bytes(conn, &body)?;

    if let Some(etag_val) = new_etag {
        let _ = set_metadata(conn, MetadataTable::Client, "canonical_etag", &etag_val);
    }

    Ok(Some(count))
}

/// Parse a canonical map JSON response and replace the local canonical DB.
pub fn ingest_canonical_map_json(conn: &Connection, json: &str) -> Result<usize> {
    ingest_canonical_map_bytes(conn, json.as_bytes())
}

fn ingest_canonical_map_bytes(conn: &Connection, bytes: &[u8]) -> Result<usize> {
    let map = parse_snapshot(bytes)?;
    replace_remi_snapshot(conn, &map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{CanonicalMappingAuthority, CanonicalPackage, PackageImplementation};

    #[test]
    fn test_ingest_canonical_map_response() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();

        let json = r#"{
            "schema_version": 1,
            "revision": 5,
            "generated_at": "2026-03-19T00:00:00Z",
            "entries": [
                {
                    "canonical": "python",
                    "kind": "package",
                    "implementations": {"fedora-44": "python3", "arch": "python"}
                },
                {
                    "canonical": "curl",
                    "kind": "package",
                    "implementations": {
                        "fedora-44": "curl",
                        "arch": "curl",
                        "ubuntu-26.04": "curl"
                    }
                }
            ]
        }"#;

        let count = ingest_canonical_map_json(&conn, json).unwrap();
        assert_eq!(count, 2);

        let pkg = crate::db::models::CanonicalPackage::find_by_name(&conn, "python").unwrap();
        assert!(pkg.is_some());

        let pkg = pkg.unwrap();
        let impls =
            crate::db::models::PackageImplementation::find_by_canonical(&conn, pkg.id.unwrap())
                .unwrap();
        assert_eq!(impls.len(), 2);
    }

    #[test]
    fn test_ingest_replaces_existing_data() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();

        let json1 = r#"{"schema_version": 1, "revision": 1, "generated_at": "2026-03-19T00:00:00Z", "entries": [{"canonical": "old-pkg", "kind": "package", "implementations": {"fedora-44": "old"}}]}"#;
        let json2 = r#"{"schema_version": 1, "revision": 2, "generated_at": "2026-03-19T00:00:00Z", "entries": [{"canonical": "new-pkg", "kind": "package", "implementations": {"arch": "new"}}]}"#;

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
    fn remote_snapshot_preserves_exact_kind_and_category() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        let json = r#"{
            "schema_version": 1,
            "revision": 1,
            "generated_at": "2026-07-26T00:00:00Z",
            "entries": [{
                "canonical": "development-tools",
                "kind": "group",
                "category": "development",
                "implementations": {"fedora-44": "@development-tools"}
            }]
        }"#;

        ingest_canonical_map_json(&conn, json).unwrap();
        let group = CanonicalPackage::find_by_name(&conn, "development-tools")
            .unwrap()
            .unwrap();
        assert_eq!(group.kind, "group");
        assert_eq!(group.category.as_deref(), Some("development"));
    }

    #[test]
    fn test_ingest_empty_map() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();

        let json = r#"{"schema_version": 1, "revision": 1, "generated_at": "2026-03-19T00:00:00Z", "entries": []}"#;
        let count = ingest_canonical_map_json(&conn, json).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_ingest_invalid_json() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();

        let result = ingest_canonical_map_json(&conn, "not json");
        assert!(result.is_err());
    }

    #[test]
    fn explicit_remote_contract_does_not_use_name_similarity_as_authority() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();

        let json = r#"{
            "schema_version": 1,
            "revision": 1,
            "generated_at": "2026-03-19T00:00:00Z",
            "entries": [
                {
                    "canonical": "openssl",
                    "kind": "package",
                    "implementations": {"fedora-44": "totally-malicious-package"}
                }
            ]
        }"#;

        assert_eq!(ingest_canonical_map_json(&conn, json).unwrap(), 1);
        let canonical = CanonicalPackage::find_by_name(&conn, "openssl")
            .unwrap()
            .unwrap();
        let implementations =
            PackageImplementation::find_by_canonical(&conn, canonical.id.unwrap()).unwrap();
        assert_eq!(implementations[0].distro_name, "totally-malicious-package");
        assert_eq!(implementations[0].source, CanonicalMappingAuthority::Remi);
    }

    #[test]
    fn test_ingest_rejects_structurally_invalid_remote_mapping() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();

        let json = r#"{
            "schema_version": 1,
            "revision": 1,
            "generated_at": "2026-03-19T00:00:00Z",
            "entries": [
                {
                    "canonical": "openssl",
                    "kind": "package",
                    "implementations": {"": "libssl3"}
                }
            ]
        }"#;

        let err = ingest_canonical_map_json(&conn, json).unwrap_err();
        assert!(err.to_string().contains("unsupported public profile"));
    }

    #[test]
    fn remote_snapshot_cannot_replace_local_contract_mapping() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        let mut canonical = CanonicalPackage::new("openssl".into(), "package".into());
        let canonical_id = canonical.insert(&conn).unwrap();
        PackageImplementation::new(
            canonical_id,
            "fedora-44".into(),
            "openssl".into(),
            CanonicalMappingAuthority::Contract,
        )
        .insert(&conn)
        .unwrap();

        let json = r#"{
            "schema_version": 1,
            "revision": 1,
            "generated_at": "2026-03-19T00:00:00Z",
            "entries": [{
                "canonical": "openssl",
                "kind": "package",
                "implementations": {"fedora-44": "openssl-libs"}
            }]
        }"#;
        let error = ingest_canonical_map_json(&conn, json).unwrap_err();
        assert!(error.to_string().contains("not incoming 'openssl-libs'"));

        let stored = PackageImplementation::find_for_distro(&conn, canonical_id, "fedora-44")
            .unwrap()
            .unwrap();
        assert_eq!(stored.distro_name, "openssl");
        assert_eq!(stored.source, CanonicalMappingAuthority::Contract);
    }

    #[test]
    fn test_extract_expected_checksum_from_header_accepts_sha256() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::HeaderName::from_static("x-conary-canonical-sha256"),
            reqwest::header::HeaderValue::from_static(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
        );

        let checksum = expected_checksum(&headers).unwrap();
        assert_eq!(
            checksum,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }
}
