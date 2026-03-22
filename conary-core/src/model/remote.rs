// conary-core/src/model/remote.rs

//! Remote collection fetching for model include resolution
//!
//! When a system model includes a remote collection (e.g. `group-base@myrepo:stable`),
//! this module resolves the label to a repository URL, fetches the collection data
//! from a Remi `/v1/models/:name` endpoint, caches it in SQLite, and returns it
//! as a `FetchedCollection`.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::db::models::{DEFAULT_CACHE_TTL_SECS, LabelEntry, RemoteCollection, Repository};
use crate::hash;
use crate::repository::RepositoryClient;

use super::{FetchedCollection, IncludedMember, ModelError, ModelResult};

/// Maximum size for fetched remote collection data (1MB)
const MAX_INCLUDE_SIZE: usize = 1_048_576;

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
pub fn resolve_label_to_url(conn: &Connection, name: &str, label_str: &str) -> ModelResult<String> {
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
/// This delegates to `fetch_and_verify_remote_collection` with signature
/// verification disabled.
pub async fn fetch_remote_collection(
    conn: &Connection,
    name: &str,
    label: &str,
    offline: bool,
) -> ModelResult<FetchedCollection> {
    fetch_and_verify_remote_collection(conn, name, label, offline, false, &[]).await
}

/// Fetch a remote collection with optional Ed25519 signature verification
///
/// When `require_signatures` is true, the collection must have a valid signature
/// from one of the `trusted_keys`. When false, signatures are verified
/// opportunistically (warn on failure but don't block).
pub async fn fetch_and_verify_remote_collection(
    conn: &Connection,
    name: &str,
    label: &str,
    offline: bool,
    require_signatures: bool,
    trusted_keys: &[String],
) -> ModelResult<FetchedCollection> {
    // Check cache first
    if let Some(cached) = RemoteCollection::find_cached(conn, name, Some(label))
        .map_err(|e| ModelError::DatabaseError(e.to_string()))?
    {
        debug!(name = %name, label = %label, "Using cached remote collection");
        let data: CollectionData = serde_json::from_str(&cached.data_json)
            .map_err(|e| ModelError::RemoteFetchError(format!("Corrupt cache entry: {e}")))?;

        // Re-verify signature on cached data when signatures are required
        if require_signatures {
            if let Some(ref sig_bytes) = cached.signature {
                let verified = verify_against_trusted_keys(&data, sig_bytes, trusted_keys)?;
                if !verified {
                    return Err(ModelError::RemoteFetchError(format!(
                        "Cached signature for collection '{name}' did not match any trusted key"
                    )));
                }
            } else {
                return Err(ModelError::RemoteFetchError(format!(
                    "No cached signature for collection '{name}' and signatures are required"
                )));
            }
        }

        return Ok(data.to_fetched_collection());
    }

    if offline {
        return Err(ModelError::RemoteFetchError(format!(
            "Collection '{name}' not in cache and --offline mode is enabled",
        )));
    }

    // Resolve label to URL
    let url = resolve_label_to_url(conn, name, label)?;
    info!(name = %name, url = %url, "Fetching remote collection with signature verification");

    let client = RepositoryClient::new()
        .map_err(|e| ModelError::RemoteFetchError(format!("HTTP client error: {e}")))?;

    let bytes = client
        .download_to_bytes(&url)
        .await
        .map_err(|e| ModelError::RemoteNotFound(format!("{name}: {e}")))?;

    // Enforce size limit
    if bytes.len() > MAX_INCLUDE_SIZE {
        return Err(ModelError::RemoteFetchError(format!(
            "Remote collection '{name}' exceeds size limit ({} bytes > {} bytes)",
            bytes.len(),
            MAX_INCLUDE_SIZE
        )));
    }

    let data: CollectionData = serde_json::from_slice(&bytes)
        .map_err(|e| ModelError::RemoteFetchError(format!("Invalid JSON from {url}: {e}")))?;

    // Verify content hash.
    // The content_hash is computed over JSON with content_hash set to "".
    // To verify, we must zero out content_hash before hashing, otherwise
    // we'd be hashing JSON that includes the hash itself (chicken-and-egg).
    let computed_hash = if !data.content_hash.is_empty() {
        let mut verification_data = data.clone();
        verification_data.content_hash = String::new();
        let verification_json = serde_json::to_vec(&verification_data)
            .map_err(|e| ModelError::RemoteFetchError(format!("Re-serialize failed: {e}")))?;
        let hash = hash::sha256_prefixed(&verification_json);
        if hash != data.content_hash {
            return Err(ModelError::RemoteFetchError(format!(
                "Content hash mismatch for remote collection '{name}': expected {}, computed {}",
                data.content_hash, hash
            )));
        }
        hash
    } else {
        hash::sha256_prefixed(&bytes)
    };

    // Attempt to fetch signature
    let sig_url = format!("{}/signature", url.trim_end_matches('/'));
    let signature_result = client.download_to_bytes(&sig_url).await;

    let mut cached_signature: Option<Vec<u8>> = None;
    let mut cached_key_id: Option<String> = None;

    match signature_result {
        Ok(sig_bytes) => {
            // Parse signature JSON response
            if let Ok(sig_json) = serde_json::from_slice::<serde_json::Value>(&sig_bytes) {
                let sig_b64 = sig_json.get("signature").and_then(|v| v.as_str());
                let key_id = sig_json.get("key_id").and_then(|v| v.as_str());

                if let Some(sig_b64) = sig_b64 {
                    use base64::Engine;
                    use base64::engine::general_purpose::STANDARD as BASE64;

                    match BASE64.decode(sig_b64) {
                        Ok(sig_raw) => {
                            // Try to verify against trusted keys
                            let verified =
                                verify_against_trusted_keys(&data, &sig_raw, trusted_keys);

                            match verified {
                                Ok(true) => {
                                    info!(name = %name, "Signature verified successfully");
                                    cached_signature = Some(sig_raw);
                                    cached_key_id = key_id.map(String::from);
                                }
                                Ok(false) => {
                                    if require_signatures {
                                        return Err(ModelError::RemoteFetchError(format!(
                                            "Signature for collection '{name}' did not match any trusted key"
                                        )));
                                    }
                                    warn!(name = %name, "Signature did not match any trusted key");
                                }
                                Err(e) => {
                                    if require_signatures {
                                        return Err(e);
                                    }
                                    warn!(name = %name, error = %e, "Signature verification error");
                                }
                            }
                        }
                        Err(e) => {
                            if require_signatures {
                                return Err(ModelError::RemoteFetchError(format!(
                                    "Invalid base64 signature for '{name}': {e}"
                                )));
                            }
                            warn!(name = %name, "Invalid base64 signature: {e}");
                        }
                    }
                } else if require_signatures {
                    return Err(ModelError::RemoteFetchError(format!(
                        "Signature response for '{name}' missing 'signature' field"
                    )));
                }
            } else if require_signatures {
                return Err(ModelError::RemoteFetchError(format!(
                    "Invalid signature JSON for '{name}'"
                )));
            }
        }
        Err(_) if require_signatures => {
            return Err(ModelError::RemoteFetchError(format!(
                "No signature available for collection '{name}' and signatures are required"
            )));
        }
        Err(_) => {
            debug!(name = %name, "No signature available (optional)");
        }
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
    cache_entry.signature = cached_signature;
    cache_entry.signer_key_id = cached_key_id;

    if let Err(e) = cache_entry.upsert(conn) {
        warn!("Failed to cache remote collection '{name}': {e}");
    }

    info!(
        name = %name,
        version = %data.version,
        members = data.members.len(),
        "Fetched remote collection (with signature check)"
    );

    Ok(data.to_fetched_collection())
}

/// Verify a collection against a list of trusted public key hex strings
fn verify_against_trusted_keys(
    data: &CollectionData,
    signature: &[u8],
    trusted_keys: &[String],
) -> ModelResult<bool> {
    use super::signing;

    if trusted_keys.is_empty() {
        return Ok(false);
    }

    for key_hex in trusted_keys {
        let key_bytes = hex::decode(key_hex).map_err(|e| {
            ModelError::RemoteFetchError(format!("Invalid trusted key hex '{key_hex}': {e}"))
        })?;

        match signing::verify_collection(data, signature, &key_bytes) {
            Ok(true) => return Ok(true),
            Ok(false) => continue,
            Err(_) => continue, // Try next key
        }
    }

    Ok(false)
}

/// Publish a collection to a remote Remi server via HTTP PUT
///
/// Sends the serialized `CollectionData` to `PUT {base_url}/v1/admin/models/{name}`.
/// Returns Ok(()) on success (201), or an error on failure.
pub async fn publish_remote_collection(
    base_url: &str,
    data: &CollectionData,
    force: bool,
) -> ModelResult<()> {
    let json = serde_json::to_vec(data).map_err(|e| {
        ModelError::RemoteFetchError(format!("Failed to serialize collection: {}", e))
    })?;

    let base = base_url.trim_end_matches('/');
    let mut url = format!("{}/v1/admin/models/{}", base, data.name);
    if force {
        url.push_str("?force=true");
    }

    info!(name = %data.name, url = %url, "Publishing collection to remote");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ModelError::RemoteFetchError(format!("HTTP client error: {}", e)))?;

    let response = client
        .put(&url)
        .header("Content-Type", "application/json")
        .body(json)
        .send()
        .await
        .map_err(|e| ModelError::RemoteFetchError(format!("Failed to PUT {}: {}", url, e)))?;

    match response.status().as_u16() {
        201 => {
            info!(name = %data.name, "Published collection to remote");
            Ok(())
        }
        409 => Err(ModelError::RemoteFetchError(format!(
            "Collection '{}' already exists on remote (use --force to overwrite)",
            data.name
        ))),
        status => {
            let body = response.text().await.unwrap_or_default();
            Err(ModelError::RemoteFetchError(format!(
                "Remote publish failed (HTTP {}): {}",
                status, body
            )))
        }
    }
}

/// Build a `CollectionData` from a parsed model's install list and pins
pub fn build_collection_data_from_model(
    model: &super::SystemModel,
    name: &str,
    version: &str,
) -> CollectionData {
    let optional_set: HashSet<&str> = model.optional.packages.iter().map(|s| s.as_str()).collect();
    let install_set: HashSet<&str> = model.config.install.iter().map(|s| s.as_str()).collect();

    let mut members: Vec<CollectionMemberData> = Vec::new();

    // Add install list packages
    for pkg_name in &model.config.install {
        let version_constraint = model.pin.get(pkg_name).cloned();
        let is_optional = optional_set.contains(pkg_name.as_str());

        members.push(CollectionMemberData {
            name: pkg_name.clone(),
            version_constraint,
            is_optional,
        });
    }

    // Add optional packages not already in the install list
    for pkg_name in &model.optional.packages {
        if !install_set.contains(pkg_name.as_str()) {
            let version_constraint = model.pin.get(pkg_name).cloned();
            members.push(CollectionMemberData {
                name: pkg_name.clone(),
                version_constraint,
                is_optional: true,
            });
        }
    }

    // Compute content hash over the serialized body
    // We serialize the whole struct first, then hash
    let mut data = CollectionData {
        name: name.to_string(),
        version: version.to_string(),
        members,
        includes: model.include.models.clone(),
        pins: model.pin.clone(),
        exclude: model.config.exclude.clone(),
        content_hash: String::new(),
        published_at: Utc::now().to_rfc3339(),
    };

    // Compute content hash over the full JSON (with empty content_hash)
    let json_bytes = serde_json::to_vec(&data).unwrap_or_default();
    data.content_hash = hash::sha256_prefixed(&json_bytes);

    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

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
        let mut repo =
            Repository::new("myrepo".to_string(), "https://remi.example.com".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        // Create a label linked to the repository
        let mut label =
            LabelEntry::new("myrepo".to_string(), "ns".to_string(), "stable".to_string());
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
        let mut repo =
            Repository::new("myrepo".to_string(), "https://remi.example.com".to_string());
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
    fn test_model_to_collection_data() {
        use crate::model::parser::parse_model_string;

        let toml = r#"
[model]
version = 1
install = ["nginx", "redis", "postgresql"]
exclude = ["sendmail"]

[pin]
nginx = "1.24.*"
openssl = "3.0.*"

[optional]
packages = ["nginx-module-geoip", "redis"]

[include]
models = ["group-core@upstream:stable"]
"#;

        let model = parse_model_string(toml).unwrap();
        let data = build_collection_data_from_model(&model, "group-web", "2.0.0");

        assert_eq!(data.name, "group-web");
        assert_eq!(data.version, "2.0.0");
        assert!(data.content_hash.starts_with("sha256:"));
        assert!(!data.content_hash.is_empty());

        // nginx is in install and has a pin
        let nginx = data.members.iter().find(|m| m.name == "nginx").unwrap();
        assert_eq!(nginx.version_constraint, Some("1.24.*".to_string()));
        assert!(!nginx.is_optional);

        // redis is in install AND optional
        let redis = data.members.iter().find(|m| m.name == "redis").unwrap();
        assert!(redis.is_optional);

        // postgresql is in install, not optional, no pin
        let pg = data
            .members
            .iter()
            .find(|m| m.name == "postgresql")
            .unwrap();
        assert!(!pg.is_optional);
        assert!(pg.version_constraint.is_none());

        // nginx-module-geoip is optional-only (not in install)
        let geoip = data
            .members
            .iter()
            .find(|m| m.name == "nginx-module-geoip")
            .unwrap();
        assert!(geoip.is_optional);

        // Includes are passed through
        assert_eq!(data.includes, vec!["group-core@upstream:stable"]);

        // Pins are passed through
        assert_eq!(data.pins.get("openssl"), Some(&"3.0.*".to_string()));

        // Exclude is passed through
        assert_eq!(data.exclude, vec!["sendmail"]);
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
