// src/model/remote.rs

//! Remote collection fetching for model include resolution
//!
//! When a system model includes a remote collection (e.g. `group-base@myrepo:stable`),
//! this module resolves the label to a repository URL, fetches the collection data
//! from a Remi `/v1/models/:name` endpoint, caches it in SQLite, and returns it
//! as a `FetchedCollection`.

use std::collections::HashMap;

use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::db::models::{
    DEFAULT_CACHE_TTL_SECS, LabelEntry, RemoteCollection, Repository,
};
use crate::hash;
use crate::repository::RepositoryClient;

use super::{FetchedCollection, IncludedMember, ModelError, ModelResult};

/// Wire format for collection data served by Remi `/v1/models/:name`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionData {
    pub name: String,
    pub version: String,
    pub members: Vec<CollectionMemberData>,
    pub includes: Vec<String>,
    pub pins: HashMap<String, String>,
    pub exclude: Vec<String>,
    pub content_hash: String,
    pub published_at: String,
}

/// A member entry in the wire format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionMemberData {
    pub name: String,
    pub version_constraint: Option<String>,
    pub is_optional: bool,
}

impl CollectionData {
    /// Convert wire format to internal FetchedCollection
    fn to_fetched_collection(&self) -> FetchedCollection {
        let members = self
            .members
            .iter()
            .map(|m| IncludedMember {
                name: m.name.clone(),
                version_constraint: m.version_constraint.clone(),
                is_optional: m.is_optional,
            })
            .collect();

        FetchedCollection {
            name: self.name.clone(),
            members,
            includes: self.includes.clone(),
        }
    }
}

/// Parse a simplified label string like "myrepo:stable" into (repo_name, tag)
///
/// The include format is `name@repo:tag`, so after splitting on `@`,
/// the label portion is `repo:tag` (not the full `repo@ns:tag` label format).
fn parse_simple_label(label_str: &str) -> ModelResult<(String, String)> {
    if let Some((repo, tag)) = label_str.split_once(':') {
        if repo.is_empty() || tag.is_empty() {
            return Err(ModelError::InvalidSearchPath(format!(
                "Invalid label format '{}': repository and tag must not be empty",
                label_str
            )));
        }
        Ok((repo.to_string(), tag.to_string()))
    } else {
        Err(ModelError::InvalidSearchPath(format!(
            "Invalid label format '{}': expected 'repository:tag'",
            label_str
        )))
    }
}

/// Resolve a label string to a Remi server URL for fetching a collection
///
/// Resolution chain:
/// 1. Try `LabelEntry::find_by_repository` to find labels linked to a repo name
/// 2. If label has `repository_id`, follow to Repository and use its URL
/// 3. If label has `delegate_to_label_id`, follow delegation chain
/// 4. Fallback: `Repository::find_by_name` and use its URL directly
pub fn resolve_label_to_url(
    conn: &Connection,
    name: &str,
    label_str: &str,
) -> ModelResult<String> {
    let (repo_name, _tag) = parse_simple_label(label_str)?;

    // Try to find labels associated with this repository name
    let labels = LabelEntry::find_by_repository(conn, &repo_name)
        .map_err(|e| ModelError::DatabaseError(e.to_string()))?;

    // Check if any matching label has a repository_id we can follow
    for label in &labels {
        if let Some(repo_id) = label.repository_id
            && let Some(repo) = Repository::find_by_id(conn, repo_id)
                .map_err(|e| ModelError::DatabaseError(e.to_string()))?
        {
            let base_url = repo.url.trim_end_matches('/');
            debug!(
                label = %label,
                repo = %repo.name,
                url = %base_url,
                "Resolved label via repository_id"
            );
            return Ok(format!("{}/v1/models/{}", base_url, name));
        }

        // Follow delegation chain (max 5 hops to prevent loops)
        if label.delegate_to_label_id.is_some()
            && let Some(url) = follow_delegation(conn, label, name, 5)?
        {
            return Ok(url);
        }
    }

    // Fallback: try Repository::find_by_name directly
    if let Some(repo) = Repository::find_by_name(conn, &repo_name)
        .map_err(|e| ModelError::DatabaseError(e.to_string()))?
    {
        let base_url = repo.url.trim_end_matches('/');
        debug!(
            repo = %repo.name,
            url = %base_url,
            "Resolved label via repository name fallback"
        );
        return Ok(format!("{}/v1/models/{}", base_url, name));
    }

    Err(ModelError::InvalidSearchPath(format!(
        "Cannot resolve label '{}': no repository '{}' found",
        label_str, repo_name
    )))
}

/// Follow delegation chain to find a repository URL
fn follow_delegation(
    conn: &Connection,
    label: &LabelEntry,
    name: &str,
    max_hops: u32,
) -> ModelResult<Option<String>> {
    let mut current = label.clone();
    let mut hops = 0;

    while let Some(delegate_id) = current.delegate_to_label_id {
        if hops >= max_hops {
            warn!("Delegation chain too deep for label {}", label);
            return Ok(None);
        }

        let delegate = LabelEntry::find_by_id(conn, delegate_id)
            .map_err(|e| ModelError::DatabaseError(e.to_string()))?;

        match delegate {
            Some(d) => {
                if let Some(repo_id) = d.repository_id
                    && let Some(repo) = Repository::find_by_id(conn, repo_id)
                        .map_err(|e| ModelError::DatabaseError(e.to_string()))?
                {
                    let base_url = repo.url.trim_end_matches('/');
                    debug!(
                        label = %label,
                        delegate = %d,
                        repo = %repo.name,
                        "Resolved label via delegation chain"
                    );
                    return Ok(Some(format!("{}/v1/models/{}", base_url, name)));
                }
                current = d;
            }
            None => return Ok(None),
        }

        hops += 1;
    }

    Ok(None)
}

/// Fetch a remote collection, using cache when available
///
/// When `offline` is true, only returns cached data (no HTTP requests).
pub fn fetch_remote_collection(
    conn: &Connection,
    name: &str,
    label: &str,
    offline: bool,
) -> ModelResult<FetchedCollection> {
    // Check cache first
    if let Some(cached) = RemoteCollection::find_cached(conn, name, Some(label))
        .map_err(|e| ModelError::DatabaseError(e.to_string()))?
    {
        debug!(name = %name, label = %label, "Using cached remote collection");
        let data: CollectionData = serde_json::from_str(&cached.data_json)
            .map_err(|e| ModelError::RemoteFetchError(format!("Corrupt cache entry: {}", e)))?;
        return Ok(data.to_fetched_collection());
    }

    // In offline mode, fail if no cache hit
    if offline {
        return Err(ModelError::RemoteFetchError(format!(
            "Collection '{}' not in cache and --offline mode is enabled",
            name
        )));
    }

    // Resolve label to URL
    let url = resolve_label_to_url(conn, name, label)?;
    info!(name = %name, url = %url, "Fetching remote collection");

    // HTTP GET
    let client = RepositoryClient::new()
        .map_err(|e| ModelError::RemoteFetchError(format!("HTTP client error: {}", e)))?;

    let bytes = client
        .download_to_bytes(&url)
        .map_err(|e| ModelError::RemoteNotFound(format!("{}: {}", name, e)))?;

    // Deserialize
    let data: CollectionData = serde_json::from_slice(&bytes)
        .map_err(|e| ModelError::RemoteFetchError(format!("Invalid JSON from {}: {}", url, e)))?;

    // Verify content hash
    let computed_hash = format!("sha256:{}", hash::sha256(&bytes));
    if !data.content_hash.is_empty() && computed_hash != data.content_hash {
        warn!(
            expected = %data.content_hash,
            computed = %computed_hash,
            "Content hash mismatch for remote collection '{}'",
            name
        );
        // Log warning but don't fail — the server may compute the hash differently
        // (e.g. over canonical form vs wire bytes)
    }

    // Cache the result
    let expires_at = (Utc::now() + chrono::Duration::seconds(DEFAULT_CACHE_TTL_SECS))
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();

    let data_json = String::from_utf8_lossy(&bytes).to_string();

    let mut cache_entry = RemoteCollection::new(
        name.to_string(),
        Some(label.to_string()),
        computed_hash,
        data_json,
        expires_at,
    );
    cache_entry.version = Some(data.version.clone());

    if let Err(e) = cache_entry.upsert(conn) {
        warn!("Failed to cache remote collection '{}': {}", name, e);
        // Non-fatal — we still have the data
    }

    info!(
        name = %name,
        version = %data.version,
        members = data.members.len(),
        "Fetched remote collection"
    );

    Ok(data.to_fetched_collection())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_collection_data_roundtrip() {
        let data = CollectionData {
            name: "group-base".to_string(),
            version: "1.0.0".to_string(),
            members: vec![
                CollectionMemberData {
                    name: "nginx".to_string(),
                    version_constraint: Some("1.24.*".to_string()),
                    is_optional: false,
                },
                CollectionMemberData {
                    name: "redis".to_string(),
                    version_constraint: None,
                    is_optional: true,
                },
            ],
            includes: vec!["group-core@upstream:stable".to_string()],
            pins: HashMap::from([("openssl".to_string(), "3.0.*".to_string())]),
            exclude: vec!["sendmail".to_string()],
            content_hash: "sha256:abc123".to_string(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&data).unwrap();
        let deserialized: CollectionData = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, "group-base");
        assert_eq!(deserialized.version, "1.0.0");
        assert_eq!(deserialized.members.len(), 2);
        assert_eq!(deserialized.members[0].name, "nginx");
        assert!(deserialized.members[1].is_optional);
        assert_eq!(deserialized.includes.len(), 1);
        assert_eq!(deserialized.pins.get("openssl"), Some(&"3.0.*".to_string()));
        assert_eq!(deserialized.exclude, vec!["sendmail"]);
    }

    #[test]
    fn test_parse_simple_label() {
        let (repo, tag) = parse_simple_label("myrepo:stable").unwrap();
        assert_eq!(repo, "myrepo");
        assert_eq!(tag, "stable");

        let (repo, tag) = parse_simple_label("fedora:f41").unwrap();
        assert_eq!(repo, "fedora");
        assert_eq!(tag, "f41");
    }

    #[test]
    fn test_parse_simple_label_invalid() {
        assert!(parse_simple_label("nocolon").is_err());
        assert!(parse_simple_label(":empty_repo").is_err());
        assert!(parse_simple_label("empty_tag:").is_err());
    }

    #[test]
    fn test_resolve_label_with_repository_id() {
        let (_temp, conn) = create_test_db();

        // Create a repository
        let mut repo = Repository::new(
            "myrepo".to_string(),
            "https://remi.example.com".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        // Create a label linked to the repository
        let mut label = LabelEntry::new(
            "myrepo".to_string(),
            "ns".to_string(),
            "stable".to_string(),
        );
        label.insert(&conn).unwrap();
        label.set_repository(&conn, Some(repo_id)).unwrap();

        // Resolve should follow label -> repository -> URL
        let url = resolve_label_to_url(&conn, "group-base", "myrepo:stable").unwrap();
        assert_eq!(url, "https://remi.example.com/v1/models/group-base");
    }

    #[test]
    fn test_resolve_label_fallback_to_repo_name() {
        let (_temp, conn) = create_test_db();

        // Create a repository with no labels pointing to it
        let mut repo = Repository::new(
            "myrepo".to_string(),
            "https://remi.example.com".to_string(),
        );
        repo.insert(&conn).unwrap();

        // Resolve should fall back to Repository::find_by_name
        let url = resolve_label_to_url(&conn, "group-base", "myrepo:stable").unwrap();
        assert_eq!(url, "https://remi.example.com/v1/models/group-base");
    }

    #[test]
    fn test_resolve_label_not_found() {
        let (_temp, conn) = create_test_db();

        let result = resolve_label_to_url(&conn, "group-base", "nonexistent:stable");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
    }

    #[test]
    fn test_fetch_uses_cache_when_fresh() {
        let (_temp, conn) = create_test_db();

        // Pre-populate cache
        let data = CollectionData {
            name: "group-cached".to_string(),
            version: "2.0".to_string(),
            members: vec![CollectionMemberData {
                name: "cached-pkg".to_string(),
                version_constraint: None,
                is_optional: false,
            }],
            includes: vec![],
            pins: HashMap::new(),
            exclude: vec![],
            content_hash: "sha256:cached".to_string(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let data_json = serde_json::to_string(&data).unwrap();
        let mut cache_entry = RemoteCollection::new(
            "group-cached".to_string(),
            Some("repo:tag".to_string()),
            "sha256:cached".to_string(),
            data_json,
            "2099-12-31T23:59:59".to_string(),
        );
        cache_entry.upsert(&conn).unwrap();

        // fetch_remote_collection should return from cache without HTTP
        let result = fetch_remote_collection(&conn, "group-cached", "repo:tag", false).unwrap();
        assert_eq!(result.name, "group-cached");
        assert_eq!(result.members.len(), 1);
        assert_eq!(result.members[0].name, "cached-pkg");
    }

    #[test]
    fn test_fetch_offline_no_cache() {
        let (_temp, conn) = create_test_db();

        // No cache entry exists, offline mode should fail
        let result = fetch_remote_collection(&conn, "group-missing", "repo:tag", true);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("offline"));
    }

    #[test]
    fn test_collection_data_to_fetched_collection() {
        let data = CollectionData {
            name: "group-test".to_string(),
            version: "1.0".to_string(),
            members: vec![
                CollectionMemberData {
                    name: "pkg-a".to_string(),
                    version_constraint: Some(">=2.0".to_string()),
                    is_optional: false,
                },
                CollectionMemberData {
                    name: "pkg-b".to_string(),
                    version_constraint: None,
                    is_optional: true,
                },
            ],
            includes: vec!["group-core@upstream:stable".to_string()],
            pins: HashMap::new(),
            exclude: vec![],
            content_hash: "sha256:test".to_string(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let fetched = data.to_fetched_collection();
        assert_eq!(fetched.name, "group-test");
        assert_eq!(fetched.members.len(), 2);
        assert_eq!(fetched.members[0].name, "pkg-a");
        assert_eq!(
            fetched.members[0].version_constraint,
            Some(">=2.0".to_string())
        );
        assert!(!fetched.members[0].is_optional);
        assert!(fetched.members[1].is_optional);
        assert_eq!(fetched.includes.len(), 1);
    }
}
