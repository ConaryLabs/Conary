// conary-core/src/trust/client.rs

//! TUF client for repository sync
//!
//! Implements the TUF client update workflow that verifies repository
//! metadata freshness and integrity during sync operations.
//!
//! Update flow (per TUF spec 5.3):
//! 1. Check for root rotation by probing `{version+1}.root.json`
//! 2. Fetch timestamp.json, verify with (possibly updated) root keys
//! 3. If snapshot hash changed, fetch snapshot.json
//! 4. Verify snapshot, check version monotonicity
//! 5. If targets hash changed, fetch targets.json
//! 6. Verify targets, check version monotonicity
//! 7. Persist verified state to database in a single transaction

use crate::trust::metadata::{
    Role, RootMetadata, Signed, SnapshotMetadata, TargetsMetadata, TimestampMetadata,
    VerifiedTufState,
};
use crate::trust::verify::{
    extract_role_keys, verify_metadata_hash, verify_not_expired, verify_root, verify_signatures,
    verify_snapshot_consistency, verify_version_increase,
};
use crate::trust::{TrustError, TrustResult};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tracing::{debug, info};

/// TUF client for a single repository
pub struct TufClient {
    repo_id: i64,
    tuf_base_url: String,
}

impl TufClient {
    /// Create a new TUF client for a repository
    pub fn new(repo_id: i64, repo_url: &str, tuf_root_url: Option<&str>) -> TrustResult<Self> {
        let tuf_base_url = tuf_root_url
            .map(String::from)
            .unwrap_or_else(|| format!("{}/tuf", repo_url.trim_end_matches('/')));

        Ok(Self {
            repo_id,
            tuf_base_url,
        })
    }

    /// Perform the full TUF update workflow
    ///
    /// Fetches and verifies all TUF metadata in the correct order,
    /// checking freshness, version monotonicity, and signature thresholds.
    pub fn update(&self, conn: &Connection) -> TrustResult<VerifiedTufState> {
        // Load trusted root from database
        let trusted_root = self.load_trusted_root(conn)?;

        // Step 1: Check for root rotation BEFORE any other metadata verification
        // (TUF spec 5.3). Probe for {version+1}.root.json and walk the chain
        // until no newer root is available. This ensures all subsequent metadata
        // is verified against the latest root keys.
        let current_root = self.check_root_rotation(conn, &trusted_root)?;

        // Step 2: Fetch and verify timestamp using (possibly updated) root keys
        let timestamp_bytes = self.fetch_metadata("timestamp.json")?;
        let signed_timestamp: Signed<TimestampMetadata> = serde_json::from_slice(&timestamp_bytes)?;
        verify_type_field(&signed_timestamp.signed.type_field, "timestamp")?;

        let (ts_keys, ts_threshold) = extract_role_keys(&current_root.signed, Role::Timestamp)?;
        verify_signatures(&signed_timestamp, Role::Timestamp, &ts_keys, ts_threshold)?;
        verify_not_expired(Role::Timestamp, &signed_timestamp.signed.expires)?;

        // Check version monotonicity against stored timestamp
        let stored_ts_version = self.load_metadata_version(conn, "timestamp")?;
        if let Some(stored_v) = stored_ts_version {
            verify_version_increase(Role::Timestamp, signed_timestamp.signed.version, stored_v)?;
        }

        // Step 3: Check if snapshot needs updating
        let snapshot_ref = signed_timestamp
            .signed
            .meta
            .get("snapshot.json")
            .ok_or_else(|| {
                TrustError::ConsistencyError(
                    "Timestamp missing snapshot.json reference".to_string(),
                )
            })?;

        let stored_snapshot_version = self.load_metadata_version(conn, "snapshot")?;
        let snapshot_changed = stored_snapshot_version.is_none_or(|v| snapshot_ref.version > v);

        let signed_snapshot = if snapshot_changed {
            let snapshot_bytes = self.fetch_metadata("snapshot.json")?;
            verify_metadata_hash(snapshot_ref, &snapshot_bytes)?;

            let signed: Signed<SnapshotMetadata> = serde_json::from_slice(&snapshot_bytes)?;
            verify_type_field(&signed.signed.type_field, "snapshot")?;
            let (snap_keys, snap_threshold) =
                extract_role_keys(&current_root.signed, Role::Snapshot)?;
            verify_signatures(&signed, Role::Snapshot, &snap_keys, snap_threshold)?;
            verify_not_expired(Role::Snapshot, &signed.signed.expires)?;

            if let Some(stored_v) = stored_snapshot_version {
                verify_version_increase(Role::Snapshot, signed.signed.version, stored_v)?;
            }

            signed
        } else {
            // Load from database
            self.load_stored_snapshot(conn)?
        };

        // Step 4: Check if targets needs updating
        let targets_ref = signed_snapshot.signed.meta.get("targets.json");
        let stored_targets_version = self.load_metadata_version(conn, "targets")?;

        let targets_changed =
            targets_ref.is_some_and(|tr| stored_targets_version.is_none_or(|v| tr.version > v));

        let signed_targets = if targets_changed {
            let targets_bytes = self.fetch_metadata("targets.json")?;
            if let Some(tr) = targets_ref {
                verify_metadata_hash(tr, &targets_bytes)?;
            }

            let signed: Signed<TargetsMetadata> = serde_json::from_slice(&targets_bytes)?;
            verify_type_field(&signed.signed.type_field, "targets")?;
            let (tgt_keys, tgt_threshold) = extract_role_keys(&current_root.signed, Role::Targets)?;
            verify_signatures(&signed, Role::Targets, &tgt_keys, tgt_threshold)?;
            verify_not_expired(Role::Targets, &signed.signed.expires)?;

            if let Some(stored_v) = stored_targets_version {
                verify_version_increase(Role::Targets, signed.signed.version, stored_v)?;
            }

            signed
        } else {
            self.load_stored_targets(conn)?
        };

        // Verify snapshot consistency
        verify_snapshot_consistency(
            &signed_snapshot.signed,
            current_root.signed.version,
            Some(signed_targets.signed.version),
        )?;

        // Persist verified state in a single transaction to prevent
        // inconsistent TUF state if the process crashes mid-persist.
        let tx = conn.unchecked_transaction()?;
        self.persist_metadata(&tx, "timestamp", &signed_timestamp)?;
        if snapshot_changed {
            self.persist_metadata(&tx, "snapshot", &signed_snapshot)?;
        }
        if targets_changed {
            self.persist_metadata(&tx, "targets", &signed_targets)?;
            self.persist_targets(&tx, &signed_targets.signed)?;
        }
        tx.commit()?;

        info!(
            "TUF update complete: root v{}, targets v{}, snapshot v{}, timestamp v{}",
            current_root.signed.version,
            signed_targets.signed.version,
            signed_snapshot.signed.version,
            signed_timestamp.signed.version,
        );

        Ok(VerifiedTufState {
            root_version: current_root.signed.version,
            targets_version: signed_targets.signed.version,
            snapshot_version: signed_snapshot.signed.version,
            timestamp_version: signed_timestamp.signed.version,
            targets: signed_targets.signed.targets,
        })
    }

    /// Bootstrap TUF for a repository (first-time trust-on-first-use)
    ///
    /// Fetches and stores the initial root metadata. This is the only
    /// time we accept root metadata without prior trust.
    pub fn bootstrap(&self, conn: &Connection, root_json: &[u8]) -> TrustResult<()> {
        let signed_root: Signed<RootMetadata> = serde_json::from_slice(root_json)?;
        verify_type_field(&signed_root.signed.type_field, "root")?;

        // Verify root is self-signed
        let (root_keys, root_threshold) = extract_role_keys(&signed_root.signed, Role::Root)?;
        verify_signatures(&signed_root, Role::Root, &root_keys, root_threshold)?;
        verify_not_expired(Role::Root, &signed_root.signed.expires)?;

        // Store the root
        self.persist_root(conn, &signed_root)?;
        self.persist_metadata(conn, "root", &signed_root)?;

        // Extract and store keys
        self.persist_root_keys(conn, &signed_root.signed)?;

        info!(
            "TUF bootstrapped for repo {}: root v{}",
            self.repo_id, signed_root.signed.version
        );

        Ok(())
    }

    /// Check for root rotation by probing for newer root versions
    ///
    /// Per TUF spec 5.3, root rotation must happen before any other metadata
    /// verification. Probes for `{version+1}.root.json` and walks the chain
    /// until no newer version is found (HTTP 404 or fetch error).
    fn check_root_rotation(
        &self,
        conn: &Connection,
        trusted_root: &Signed<RootMetadata>,
    ) -> TrustResult<Signed<RootMetadata>> {
        let mut current = trusted_root.clone();

        loop {
            let next_version = current.signed.version + 1;
            let filename = format!("{next_version}.root.json");

            // Probe for the next root version; if it doesn't exist, we're done
            let Some(root_bytes) = self.try_fetch_metadata(&filename)? else {
                break;
            };

            let new_root: Signed<RootMetadata> = serde_json::from_slice(&root_bytes)?;

            // Verify against the current trusted root's keys
            let (old_keys, old_threshold) = extract_role_keys(&current.signed, Role::Root)?;
            verify_root(&new_root, &old_keys, old_threshold)?;
            verify_not_expired(Role::Root, &new_root.signed.expires)?;
            verify_version_increase(Role::Root, new_root.signed.version, current.signed.version)?;

            info!(
                "Root key rotation: v{} -> v{}",
                current.signed.version, new_root.signed.version
            );

            // Store the new root
            self.persist_root(conn, &new_root)?;
            self.persist_root_keys(conn, &new_root.signed)?;

            current = new_root;
        }

        // If we rotated, persist the final root as current metadata
        if current.signed.version > trusted_root.signed.version {
            self.persist_metadata(conn, "root", &current)?;
        }

        Ok(current)
    }

    /// Try to fetch metadata, returning `None` for HTTP 404 / not found
    ///
    /// Unlike `fetch_metadata`, this does not treat a missing file as an error.
    /// Used for probing whether a newer root version exists.
    fn try_fetch_metadata(&self, filename: &str) -> TrustResult<Option<Vec<u8>>> {
        use std::io::Read;

        let url = format!("{}/{}", self.tuf_base_url, filename);
        debug!("Probing TUF metadata: {}", url);

        let response = reqwest::blocking::get(&url)
            .map_err(|e| TrustError::FetchError(format!("Failed to fetch {filename}: {e}")))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(TrustError::FetchError(format!(
                "HTTP {} fetching {filename}",
                response.status()
            )));
        }

        // Check Content-Length before downloading body
        if let Some(content_length) = response.content_length()
            && content_length > Self::MAX_TUF_METADATA_SIZE
        {
            return Err(TrustError::FetchError(format!(
                "TUF metadata {filename} exceeds size limit: {content_length} bytes \
                 (max {} bytes)",
                Self::MAX_TUF_METADATA_SIZE
            )));
        }

        let max_size = Self::MAX_TUF_METADATA_SIZE as usize;
        let mut body = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or(4096)
                .min(Self::MAX_TUF_METADATA_SIZE) as usize,
        );
        response
            .take(Self::MAX_TUF_METADATA_SIZE + 1)
            .read_to_end(&mut body)
            .map_err(|e| TrustError::FetchError(format!("Failed to read {filename}: {e}")))?;

        if body.len() > max_size {
            return Err(TrustError::FetchError(format!(
                "TUF metadata {filename} exceeds size limit: {} bytes (max {} bytes)",
                body.len(),
                Self::MAX_TUF_METADATA_SIZE
            )));
        }

        Ok(Some(body))
    }

    /// Maximum size for TUF metadata files (10 MB)
    ///
    /// Prevents DoS attacks where a malicious server returns arbitrarily large
    /// metadata files to exhaust memory.
    const MAX_TUF_METADATA_SIZE: u64 = 10 * 1024 * 1024;

    /// Fetch metadata from the TUF base URL
    ///
    /// Reads the response body in chunks to enforce the size limit during
    /// download, preventing a malicious server from streaming unbounded data
    /// when Content-Length is absent.
    fn fetch_metadata(&self, filename: &str) -> TrustResult<Vec<u8>> {
        use std::io::Read;

        let url = format!("{}/{}", self.tuf_base_url, filename);
        debug!("Fetching TUF metadata: {}", url);

        let response = reqwest::blocking::get(&url)
            .map_err(|e| TrustError::FetchError(format!("Failed to fetch {filename}: {e}")))?;

        if !response.status().is_success() {
            return Err(TrustError::FetchError(format!(
                "HTTP {} fetching {filename}",
                response.status()
            )));
        }

        // Check Content-Length before downloading body
        if let Some(content_length) = response.content_length()
            && content_length > Self::MAX_TUF_METADATA_SIZE
        {
            return Err(TrustError::FetchError(format!(
                "TUF metadata {filename} exceeds size limit: {content_length} bytes \
                 (max {} bytes)",
                Self::MAX_TUF_METADATA_SIZE
            )));
        }

        // Read body in chunks, enforcing size limit during download.
        // This prevents a malicious server from omitting Content-Length
        // and streaming unlimited data.
        let max_size = Self::MAX_TUF_METADATA_SIZE as usize;
        let mut body = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or(4096)
                .min(Self::MAX_TUF_METADATA_SIZE) as usize,
        );
        response
            .take(Self::MAX_TUF_METADATA_SIZE + 1)
            .read_to_end(&mut body)
            .map_err(|e| TrustError::FetchError(format!("Failed to read {filename}: {e}")))?;

        if body.len() > max_size {
            return Err(TrustError::FetchError(format!(
                "TUF metadata {filename} exceeds size limit: {} bytes (max {} bytes)",
                body.len(),
                Self::MAX_TUF_METADATA_SIZE
            )));
        }

        Ok(body)
    }

    /// Load the trusted root from the database
    fn load_trusted_root(&self, conn: &Connection) -> TrustResult<Signed<RootMetadata>> {
        let json: String = conn
            .query_row(
                "SELECT signed_metadata FROM tuf_roots
                 WHERE repository_id = ?1
                 ORDER BY version DESC LIMIT 1",
                params![self.repo_id],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => TrustError::ConsistencyError(
                    "No trusted root found - run 'conary trust init' first".to_string(),
                ),
                other => TrustError::Database(other),
            })?;

        let signed: Signed<RootMetadata> = serde_json::from_str(&json)?;
        Ok(signed)
    }

    /// Load the stored version number for a metadata role
    fn load_metadata_version(&self, conn: &Connection, role: &str) -> TrustResult<Option<u64>> {
        let version: Option<i64> = conn
            .query_row(
                "SELECT version FROM tuf_metadata
                 WHERE repository_id = ?1 AND role = ?2",
                params![self.repo_id, role],
                |row| row.get(0),
            )
            .optional()?;

        Ok(version.and_then(|v| u64::try_from(v).ok()))
    }

    /// Load stored snapshot metadata from the database
    fn load_stored_snapshot(&self, conn: &Connection) -> TrustResult<Signed<SnapshotMetadata>> {
        let json: String = conn
            .query_row(
                "SELECT signed_metadata FROM tuf_metadata
                 WHERE repository_id = ?1 AND role = 'snapshot'",
                params![self.repo_id],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    TrustError::ConsistencyError("No stored snapshot found".to_string())
                }
                other => TrustError::Database(other),
            })?;

        let signed: Signed<SnapshotMetadata> = serde_json::from_str(&json)?;
        Ok(signed)
    }

    /// Load stored targets metadata from the database
    fn load_stored_targets(&self, conn: &Connection) -> TrustResult<Signed<TargetsMetadata>> {
        let json: String = conn
            .query_row(
                "SELECT signed_metadata FROM tuf_metadata
                 WHERE repository_id = ?1 AND role = 'targets'",
                params![self.repo_id],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    TrustError::ConsistencyError("No stored targets found".to_string())
                }
                other => TrustError::Database(other),
            })?;

        let signed: Signed<TargetsMetadata> = serde_json::from_str(&json)?;
        Ok(signed)
    }

    /// Persist signed metadata to the tuf_metadata table
    fn persist_metadata<T: serde::Serialize + TufMetadataFields>(
        &self,
        conn: &Connection,
        role: &str,
        signed: &Signed<T>,
    ) -> TrustResult<()> {
        let json = serde_json::to_string(signed)?;
        let hash = hex::encode(Sha256::digest(json.as_bytes()));

        conn.execute(
            "INSERT OR REPLACE INTO tuf_metadata
             (repository_id, role, version, metadata_hash, signed_metadata, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                self.repo_id,
                role,
                signed.signed.version() as i64,
                hash,
                json,
                signed.signed.expires().to_rfc3339(),
            ],
        )?;

        Ok(())
    }

    /// Persist root metadata to the tuf_roots table
    fn persist_root(&self, conn: &Connection, signed: &Signed<RootMetadata>) -> TrustResult<()> {
        let json = serde_json::to_string(signed)?;
        let thresholds: BTreeMap<String, u64> = signed
            .signed
            .roles
            .iter()
            .map(|(k, v)| (k.clone(), v.threshold))
            .collect();
        let role_keys: BTreeMap<String, Vec<String>> = signed
            .signed
            .roles
            .iter()
            .map(|(k, v)| (k.clone(), v.keyids.clone()))
            .collect();

        conn.execute(
            "INSERT OR REPLACE INTO tuf_roots
             (repository_id, version, signed_metadata, spec_version, expires_at,
              thresholds_json, role_keys_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                self.repo_id,
                signed.signed.version as i64,
                json,
                &signed.signed.spec_version,
                signed.signed.expires.to_rfc3339(),
                serde_json::to_string(&thresholds)?,
                serde_json::to_string(&role_keys)?,
            ],
        )?;

        // Update repository's root version
        conn.execute(
            "UPDATE repositories SET tuf_root_version = ?1 WHERE id = ?2",
            params![signed.signed.version as i64, self.repo_id],
        )?;

        Ok(())
    }

    /// Persist keys extracted from root metadata
    fn persist_root_keys(&self, conn: &Connection, root: &RootMetadata) -> TrustResult<()> {
        // Delete old keys for this repo
        conn.execute(
            "DELETE FROM tuf_keys WHERE repository_id = ?1",
            params![self.repo_id],
        )?;

        let mut stmt = conn.prepare(
            "INSERT INTO tuf_keys (id, repository_id, key_type, public_key, roles_json, from_root_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        for (key_id, key) in &root.keys {
            // Find which roles this key is assigned to
            let key_roles: Vec<String> = root
                .roles
                .iter()
                .filter(|(_, role_def)| role_def.keyids.contains(key_id))
                .map(|(role_name, _)| role_name.clone())
                .collect();

            stmt.execute(params![
                key_id,
                self.repo_id,
                &key.keytype,
                &key.keyval.public,
                serde_json::to_string(&key_roles)?,
                root.version as i64,
            ])?;
        }

        Ok(())
    }

    /// Persist target entries from targets metadata
    fn persist_targets(&self, conn: &Connection, targets: &TargetsMetadata) -> TrustResult<()> {
        // Delete old targets
        conn.execute(
            "DELETE FROM tuf_targets WHERE repository_id = ?1",
            params![self.repo_id],
        )?;

        let mut stmt = conn.prepare(
            "INSERT INTO tuf_targets (repository_id, target_path, sha256, length, custom_json, targets_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        for (path, desc) in &targets.targets {
            let sha256 = desc.hashes.get("sha256").cloned().unwrap_or_default();

            stmt.execute(params![
                self.repo_id,
                path,
                sha256,
                desc.length as i64,
                Option::<String>::None, // custom_json - not used yet
                targets.version as i64,
            ])?;
        }

        Ok(())
    }
}

/// Verify that a metadata type_field matches the expected role name.
///
/// Prevents a server from serving the wrong metadata type (e.g., returning
/// targets.json content when snapshot.json is requested).
fn verify_type_field(type_field: &str, expected: &str) -> TrustResult<()> {
    if type_field != expected {
        return Err(TrustError::ConsistencyError(format!(
            "Metadata type mismatch: expected '{}', got '{}'",
            expected, type_field
        )));
    }
    Ok(())
}

/// Trait for extracting common fields from TUF metadata types
pub trait TufMetadataFields {
    fn version(&self) -> u64;
    fn expires(&self) -> &chrono::DateTime<chrono::Utc>;
}

impl TufMetadataFields for RootMetadata {
    fn version(&self) -> u64 {
        self.version
    }
    fn expires(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.expires
    }
}

impl TufMetadataFields for TargetsMetadata {
    fn version(&self) -> u64 {
        self.version
    }
    fn expires(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.expires
    }
}

impl TufMetadataFields for SnapshotMetadata {
    fn version(&self) -> u64 {
        self.version
    }
    fn expires(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.expires
    }
}

impl TufMetadataFields for TimestampMetadata {
    fn version(&self) -> u64 {
        self.version
    }
    fn expires(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.expires
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tuf_client_new() {
        let client = TufClient::new(1, "https://repo.example.com", None).unwrap();
        assert_eq!(client.tuf_base_url, "https://repo.example.com/tuf");

        let client2 = TufClient::new(
            1,
            "https://repo.example.com",
            Some("https://tuf.example.com"),
        )
        .unwrap();
        assert_eq!(client2.tuf_base_url, "https://tuf.example.com");
    }

    #[test]
    fn test_tuf_client_new_strips_trailing_slash() {
        let client = TufClient::new(1, "https://repo.example.com/", None).unwrap();
        assert_eq!(client.tuf_base_url, "https://repo.example.com/tuf");
    }
}
