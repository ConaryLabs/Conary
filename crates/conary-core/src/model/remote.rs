// conary-core/src/model/remote.rs

//! Remote collection fetching for model include resolution
//!
//! When a system model includes a remote collection (e.g. `group-base@myrepo:stable`),
//! this module resolves the label to a repository URL, fetches the collection data
//! from a Remi `/v1/models/:name` endpoint, caches it in SQLite, and returns it
//! as a `FetchedCollection`.

use std::collections::{BTreeMap, HashSet};

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
#[serde(deny_unknown_fields)]
pub struct CollectionData {
    pub name: String,
    pub version: String,
    pub members: Vec<CollectionMemberData>,
    pub includes: Vec<String>,
    pub pins: BTreeMap<String, String>,
    pub exclude: Vec<String>,
    pub content_hash: String,
    pub published_at: String,
}

/// Signed publication envelope accepted by Remi's model collection API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedCollectionEnvelope {
    pub collection: CollectionData,
    /// Ed25519 signature over canonical `collection` JSON, base64 encoded.
    pub signature: String,
    /// Ed25519 public key, hex encoded.
    pub public_key: String,
}

/// Wire format served by Remi's collection signature endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionSignature {
    /// Ed25519 signature over canonical `CollectionData` JSON, base64 encoded.
    pub signature: String,
    /// Hex-encoded first eight bytes of the verifying public key.
    pub key_id: String,
}

/// A member entry in the wire format
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
            pins: self
                .pins
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            exclude: self.exclude.clone(),
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

    Err(ModelError::InvalidSearchPath(format!(
        "Cannot resolve label '{}': no exact label contract for repository '{}'",
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

/// Fetch a signed remote collection, using cache when available.
///
/// When `offline` is true, only returns cached and reverified data.
pub async fn fetch_remote_collection(
    conn: &Connection,
    name: &str,
    label: &str,
    offline: bool,
    trusted_keys: &[String],
) -> ModelResult<FetchedCollection> {
    fetch_and_verify_remote_collection(conn, name, label, offline, trusted_keys).await
}

/// Fetch a remote collection with mandatory Ed25519 signature verification.
pub async fn fetch_and_verify_remote_collection(
    conn: &Connection,
    name: &str,
    label: &str,
    offline: bool,
    trusted_keys: &[String],
) -> ModelResult<FetchedCollection> {
    if trusted_keys.is_empty() {
        return Err(ModelError::RemoteFetchError(format!(
            "Collection '{name}' requires a trusted signing key, but no trusted keys are configured"
        )));
    }

    // Check cache first
    if let Some(cached) = RemoteCollection::find_cached(conn, name, Some(label))
        .map_err(|e| ModelError::DatabaseError(e.to_string()))?
    {
        debug!(name = %name, label = %label, "Using cached remote collection");
        let data: CollectionData = serde_json::from_str(&cached.data_json)
            .map_err(|e| ModelError::RemoteFetchError(format!("Corrupt cache entry: {e}")))?;

        validate_collection_data(&data, name)?;
        let computed_hash = collection_content_hash(&data)?;
        if computed_hash != data.content_hash || cached.content_hash != data.content_hash {
            return Err(ModelError::RemoteFetchError(format!(
                "Cached content hash for collection '{name}' does not match its signed data"
            )));
        }
        let sig_bytes = cached.signature.as_deref().ok_or_else(|| {
            ModelError::RemoteFetchError(format!("No cached signature for collection '{name}'"))
        })?;
        let verified_key_id = verify_against_trusted_keys(&data, sig_bytes, trusted_keys)?
            .ok_or_else(|| {
                ModelError::RemoteFetchError(format!(
                    "Cached signature for collection '{name}' did not match any trusted key"
                ))
            })?;
        if cached.signer_key_id.as_deref() != Some(verified_key_id.as_str()) {
            return Err(ModelError::RemoteFetchError(format!(
                "Cached signer identity for collection '{name}' does not match the verifying key"
            )));
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
    validate_collection_data(&data, name)?;

    let computed_hash = collection_content_hash(&data)?;
    if computed_hash != data.content_hash {
        return Err(ModelError::RemoteFetchError(format!(
            "Content hash mismatch for remote collection '{name}': expected {}, computed {}",
            data.content_hash, computed_hash
        )));
    }

    // Attempt to fetch signature
    let sig_url = format!("{}/signature", url.trim_end_matches('/'));
    let (cached_signature, cached_key_id) = match client.download_to_bytes(&sig_url).await {
        Ok(sig_bytes) => {
            use base64::Engine;
            use base64::engine::general_purpose::STANDARD as BASE64;

            let response: CollectionSignature =
                serde_json::from_slice(&sig_bytes).map_err(|error| {
                    ModelError::RemoteFetchError(format!(
                        "Invalid signature response for '{name}': {error}"
                    ))
                })?;
            let signature = BASE64.decode(&response.signature).map_err(|error| {
                ModelError::RemoteFetchError(format!(
                    "Invalid base64 signature for '{name}': {error}"
                ))
            })?;
            let verified_key_id = verify_against_trusted_keys(&data, &signature, trusted_keys)?
                .ok_or_else(|| {
                    ModelError::RemoteFetchError(format!(
                        "Signature for collection '{name}' did not match any trusted key"
                    ))
                })?;
            if response.key_id != verified_key_id {
                return Err(ModelError::RemoteFetchError(format!(
                    "Signer identity for collection '{name}' does not match the verifying key"
                )));
            }
            info!(name = %name, "Signature verified successfully");
            (Some(signature), Some(verified_key_id))
        }
        Err(_) => {
            return Err(ModelError::RemoteFetchError(format!(
                "No signature available for collection '{name}'"
            )));
        }
    };

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
) -> ModelResult<Option<String>> {
    use super::signing;

    if trusted_keys.is_empty() {
        return Err(ModelError::RemoteFetchError(
            "Remote signature verification requested, but no trusted keys are configured"
                .to_string(),
        ));
    }

    for key_hex in trusted_keys {
        let key_bytes = hex::decode(key_hex).map_err(|e| {
            ModelError::RemoteFetchError(format!("Invalid trusted key hex '{key_hex}': {e}"))
        })?;

        match signing::verify_collection(data, signature, &key_bytes) {
            Ok(true) => return Ok(Some(hex::encode(&key_bytes[..8]))),
            Ok(false) => continue,
            Err(error) => {
                return Err(ModelError::RemoteFetchError(format!(
                    "Invalid trusted key '{key_hex}': {error}"
                )));
            }
        }
    }

    Ok(None)
}

/// Validate the exact signed collection wire contract.
pub fn validate_collection_data(data: &CollectionData, expected_name: &str) -> ModelResult<()> {
    if data.name != expected_name {
        return Err(ModelError::RemoteFetchError(format!(
            "Remote collection name mismatch: requested '{expected_name}', received '{}'",
            data.name
        )));
    }
    if data.version.is_empty() {
        return Err(ModelError::RemoteFetchError(format!(
            "Remote collection '{expected_name}' has no version"
        )));
    }
    if data.content_hash.is_empty() {
        return Err(ModelError::RemoteFetchError(format!(
            "Remote collection '{expected_name}' has no content hash"
        )));
    }
    if data.published_at.is_empty() {
        return Err(ModelError::RemoteFetchError(format!(
            "Remote collection '{expected_name}' has no publication timestamp"
        )));
    }
    if data.members.iter().any(|member| member.name.is_empty()) {
        return Err(ModelError::RemoteFetchError(format!(
            "Remote collection '{expected_name}' contains an unnamed member"
        )));
    }
    if data.includes.iter().any(String::is_empty) {
        return Err(ModelError::RemoteFetchError(format!(
            "Remote collection '{expected_name}' contains an empty include"
        )));
    }
    if data.exclude.iter().any(String::is_empty) {
        return Err(ModelError::RemoteFetchError(format!(
            "Remote collection '{expected_name}' contains an empty exclusion"
        )));
    }
    if data
        .pins
        .iter()
        .any(|(package, constraint)| package.is_empty() || constraint.is_empty())
    {
        return Err(ModelError::RemoteFetchError(format!(
            "Remote collection '{expected_name}' contains an incomplete pin"
        )));
    }
    Ok(())
}

/// Compute the signed collection's canonical content hash.
pub fn collection_content_hash(data: &CollectionData) -> ModelResult<String> {
    let mut verification_data = data.clone();
    verification_data.content_hash.clear();
    let verification_json = crate::json::canonical_json(&verification_data)
        .map_err(|error| ModelError::SerializationError(error.to_string()))?;
    Ok(hash::sha256_prefixed(&verification_json))
}

/// Publish a collection to a remote Remi server via HTTP PUT
///
/// Sends the serialized `CollectionData` to `PUT {base_url}/v1/admin/models/{name}`.
/// Returns Ok(()) on success (201), or an error on failure.
pub async fn publish_remote_collection(
    base_url: &str,
    data: &CollectionData,
    signature: &[u8],
    public_key: &[u8],
    bearer_token: &str,
    force: bool,
) -> ModelResult<()> {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;

    validate_collection_data(data, &data.name)?;
    let computed_hash = collection_content_hash(data)?;
    if computed_hash != data.content_hash {
        return Err(ModelError::RemoteFetchError(format!(
            "Collection '{}' has content hash '{}', expected '{}'",
            data.name, data.content_hash, computed_hash
        )));
    }
    if !super::signing::verify_collection(data, signature, public_key)? {
        return Err(ModelError::RemoteFetchError(format!(
            "Collection '{}' signature does not match its publishing key",
            data.name
        )));
    }
    if bearer_token.is_empty() {
        return Err(ModelError::RemoteFetchError(
            "Remote collection publication requires a Remi admin token".to_string(),
        ));
    }

    let envelope = PublishedCollectionEnvelope {
        collection: data.clone(),
        signature: BASE64.encode(signature),
        public_key: hex::encode(public_key),
    };
    let json = serde_json::to_vec(&envelope).map_err(|e| {
        ModelError::RemoteFetchError(format!("Failed to serialize signed collection: {e}"))
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
        .bearer_auth(bearer_token)
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
            let body = response.text().await.map_err(|error| {
                ModelError::RemoteFetchError(format!(
                    "Remote publish failed (HTTP {status}) and its response body could not be read: {error}"
                ))
            })?;
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
) -> Result<CollectionData, ModelError> {
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
        pins: model
            .pin
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        exclude: model.config.exclude.clone(),
        content_hash: String::new(),
        published_at: Utc::now().to_rfc3339(),
    };

    data.content_hash = collection_content_hash(&data)?;
    validate_collection_data(&data, name)?;

    Ok(data)
}

#[cfg(test)]
mod tests;
