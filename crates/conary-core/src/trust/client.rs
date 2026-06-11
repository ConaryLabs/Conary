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

use crate::hash;
use crate::repository::static_repo::RepoLocation;
use crate::trust::metadata::{
    MetaFile, Role, RootMetadata, Signed, SnapshotMetadata, TargetsMetadata, TimestampMetadata,
    VerifiedTufState,
};
use crate::trust::verify::{
    extract_role_keys, verify_metadata_hash, verify_not_expired, verify_root, verify_signatures,
    verify_snapshot_consistency, verify_static_snapshot_consistency, verify_version_increase,
};
use crate::trust::{TrustError, TrustResult};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;
use tracing::{debug, info};

/// TUF update behavior for repository-specific invariants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TufUpdateMode {
    /// Generic TUF client behavior for existing Remi/trust callers.
    Generic,
    /// Static repository behavior requiring complete root/targets snapshot pins.
    StaticRepo,
}

/// TUF client for a single repository
pub struct TufClient {
    repo_id: i64,
    tuf_base_url: String,
    tuf_location: RepoLocation,
    update_mode: TufUpdateMode,
}

/// Blocking DB state required before an async TUF update.
pub(crate) struct TufUpdateState {
    trusted_root: Signed<RootMetadata>,
    stored_timestamp_version: Option<u64>,
    stored_timestamp_hash: Option<String>,
    stored_snapshot_version: Option<u64>,
    stored_targets_version: Option<u64>,
    stored_snapshot: Option<Signed<SnapshotMetadata>>,
    stored_targets: Option<Signed<TargetsMetadata>>,
}

/// Fully verified TUF metadata ready to persist in a blocking DB phase.
pub(crate) struct TufUpdateSnapshot {
    current_root: Signed<RootMetadata>,
    rotated_roots: Vec<Signed<RootMetadata>>,
    signed_timestamp: Signed<TimestampMetadata>,
    signed_snapshot: Signed<SnapshotMetadata>,
    signed_targets: Signed<TargetsMetadata>,
    snapshot_changed: bool,
    targets_changed: bool,
}

impl TufClient {
    /// Create a new TUF client for a repository
    pub fn new(repo_id: i64, repo_url: &str, tuf_root_url: Option<&str>) -> TrustResult<Self> {
        Self::new_with_mode(repo_id, repo_url, tuf_root_url, TufUpdateMode::Generic)
    }

    /// Create a new static-repository TUF client.
    pub fn new_static(
        repo_id: i64,
        repo_url: &str,
        tuf_root_url: Option<&str>,
    ) -> TrustResult<Self> {
        Self::new_with_mode(repo_id, repo_url, tuf_root_url, TufUpdateMode::StaticRepo)
    }

    /// Create a new TUF client with an explicit update mode.
    pub fn new_with_mode(
        repo_id: i64,
        repo_url: &str,
        tuf_root_url: Option<&str>,
        update_mode: TufUpdateMode,
    ) -> TrustResult<Self> {
        let tuf_base_url = tuf_root_url
            .map(String::from)
            .unwrap_or_else(|| format!("{}/tuf", repo_url.trim_end_matches('/')));
        let tuf_location = RepoLocation::parse(&tuf_base_url).map_err(|error| {
            TrustError::FetchError(format!(
                "Invalid TUF metadata location {tuf_base_url}: {error}"
            ))
        })?;

        Ok(Self {
            repo_id,
            tuf_base_url,
            tuf_location,
            update_mode,
        })
    }

    /// Perform the full TUF update workflow
    ///
    /// Fetches and verifies all TUF metadata in the correct order,
    /// checking freshness, version monotonicity, and signature thresholds.
    pub async fn update(&self, conn: &Connection) -> TrustResult<VerifiedTufState> {
        let state = self.load_update_state(conn)?;
        let snapshot = self.fetch_update_snapshot(state).await?;
        self.persist_update_snapshot(conn, snapshot)
    }

    /// Load the DB-backed state needed before performing async TUF fetches.
    pub(crate) fn load_update_state(&self, conn: &Connection) -> TrustResult<TufUpdateState> {
        Ok(TufUpdateState {
            trusted_root: self.load_trusted_root(conn)?,
            stored_timestamp_version: self.load_metadata_version(conn, "timestamp")?,
            stored_timestamp_hash: self.load_metadata_hash(conn, "timestamp")?,
            stored_snapshot_version: self.load_metadata_version(conn, "snapshot")?,
            stored_targets_version: self.load_metadata_version(conn, "targets")?,
            stored_snapshot: self.load_stored_snapshot_optional(conn)?,
            stored_targets: self.load_stored_targets_optional(conn)?,
        })
    }

    /// Fetch and verify TUF metadata using owned state only.
    pub(crate) async fn fetch_update_snapshot(
        &self,
        state: TufUpdateState,
    ) -> TrustResult<TufUpdateSnapshot> {
        let TufUpdateState {
            trusted_root,
            stored_timestamp_version,
            stored_timestamp_hash,
            stored_snapshot_version,
            stored_targets_version,
            stored_snapshot,
            stored_targets,
        } = state;

        // Step 1: Check for root rotation BEFORE any other metadata verification
        // (TUF spec 5.3). Probe for {version+1}.root.json and walk the chain
        // until no newer root is available. This ensures all subsequent metadata
        // is verified against the latest root keys.
        let (current_root, rotated_roots) = self.check_root_rotation(&trusted_root).await?;
        if self.update_mode == TufUpdateMode::StaticRepo {
            self.verify_signed_metadata_not_expired(Role::Root, &current_root)?;
        }

        // Step 2: Fetch and verify timestamp using (possibly updated) root keys
        let timestamp_bytes = self.fetch_metadata("timestamp.json").await?;
        let signed_timestamp: Signed<TimestampMetadata> = serde_json::from_slice(&timestamp_bytes)?;
        verify_type_field(&signed_timestamp.signed.type_field, "timestamp")?;

        let (ts_keys, ts_threshold) = extract_role_keys(&current_root.signed, Role::Timestamp)?;
        verify_signatures(&signed_timestamp, Role::Timestamp, &ts_keys, ts_threshold)?;
        self.verify_signed_metadata_not_expired(Role::Timestamp, &signed_timestamp)?;

        // Check version monotonicity against stored timestamp
        if let Some(stored_v) = stored_timestamp_version {
            match signed_timestamp.signed.version.cmp(&stored_v) {
                std::cmp::Ordering::Greater => {}
                std::cmp::Ordering::Equal => {
                    if self.update_mode != TufUpdateMode::StaticRepo {
                        verify_version_increase(
                            Role::Timestamp,
                            signed_timestamp.signed.version,
                            stored_v,
                        )?;
                    }
                    let offered_hash = metadata_hash_for_persistence(&signed_timestamp)?;
                    if stored_timestamp_hash.as_deref() != Some(offered_hash.as_str()) {
                        return Err(TrustError::ConsistencyError(
                            "Timestamp version matches stored version but metadata bytes/hash differ"
                                .to_string(),
                        ));
                    }
                    let signed_snapshot = stored_snapshot.ok_or_else(|| {
                        TrustError::ConsistencyError("No stored snapshot found".to_string())
                    })?;
                    let signed_targets = stored_targets.ok_or_else(|| {
                        TrustError::ConsistencyError("No stored targets found".to_string())
                    })?;
                    let snapshot_ref = signed_timestamp
                        .signed
                        .meta
                        .get("snapshot.json")
                        .ok_or_else(|| {
                            TrustError::ConsistencyError(
                                "Timestamp missing snapshot.json reference".to_string(),
                            )
                        })?;
                    self.verify_cached_metadata_ref(
                        snapshot_ref,
                        Role::Snapshot,
                        &signed_snapshot,
                    )?;
                    let targets_ref =
                        signed_snapshot
                            .signed
                            .meta
                            .get("targets.json")
                            .ok_or_else(|| {
                                TrustError::ConsistencyError(
                                    "Snapshot missing targets.json reference".to_string(),
                                )
                            })?;
                    self.verify_cached_metadata_ref(targets_ref, Role::Targets, &signed_targets)?;
                    self.verify_signed_metadata_not_expired(Role::Snapshot, &signed_snapshot)?;
                    self.verify_signed_metadata_not_expired(Role::Targets, &signed_targets)?;
                    self.verify_snapshot_consistency(
                        &signed_snapshot.signed,
                        current_root.signed.version,
                        signed_targets.signed.version,
                    )?;
                    return Ok(TufUpdateSnapshot {
                        current_root,
                        rotated_roots,
                        signed_timestamp,
                        signed_snapshot,
                        signed_targets,
                        snapshot_changed: false,
                        targets_changed: false,
                    });
                }
                std::cmp::Ordering::Less => {
                    verify_version_increase(
                        Role::Timestamp,
                        signed_timestamp.signed.version,
                        stored_v,
                    )?;
                }
            }
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

        let snapshot_changed = stored_snapshot_version.is_none_or(|v| snapshot_ref.version > v);

        let signed_snapshot = if snapshot_changed {
            let snapshot_bytes = self.fetch_metadata("snapshot.json").await?;
            verify_metadata_hash(snapshot_ref, &snapshot_bytes, true)?;

            let signed: Signed<SnapshotMetadata> = serde_json::from_slice(&snapshot_bytes)?;
            verify_type_field(&signed.signed.type_field, "snapshot")?;
            let (snap_keys, snap_threshold) =
                extract_role_keys(&current_root.signed, Role::Snapshot)?;
            verify_signatures(&signed, Role::Snapshot, &snap_keys, snap_threshold)?;
            self.verify_signed_metadata_not_expired(Role::Snapshot, &signed)?;

            if let Some(stored_v) = stored_snapshot_version {
                verify_version_increase(Role::Snapshot, signed.signed.version, stored_v)?;
            }

            signed
        } else {
            let signed = stored_snapshot.ok_or_else(|| {
                TrustError::ConsistencyError("No stored snapshot found".to_string())
            })?;
            self.verify_cached_metadata_ref(snapshot_ref, Role::Snapshot, &signed)?;
            self.verify_signed_metadata_not_expired(Role::Snapshot, &signed)?;
            signed
        };

        // Step 4: Check if targets needs updating
        let targets_ref = signed_snapshot.signed.meta.get("targets.json");

        let targets_changed =
            targets_ref.is_some_and(|tr| stored_targets_version.is_none_or(|v| tr.version > v));

        let signed_targets = if targets_changed {
            let targets_bytes = self.fetch_metadata("targets.json").await?;
            if let Some(tr) = targets_ref {
                verify_metadata_hash(tr, &targets_bytes, true)?;
            }

            let signed: Signed<TargetsMetadata> = serde_json::from_slice(&targets_bytes)?;
            verify_type_field(&signed.signed.type_field, "targets")?;
            let (tgt_keys, tgt_threshold) = extract_role_keys(&current_root.signed, Role::Targets)?;
            verify_signatures(&signed, Role::Targets, &tgt_keys, tgt_threshold)?;
            self.verify_signed_metadata_not_expired(Role::Targets, &signed)?;

            if let Some(stored_v) = stored_targets_version {
                verify_version_increase(Role::Targets, signed.signed.version, stored_v)?;
            }

            signed
        } else {
            let signed = stored_targets.ok_or_else(|| {
                TrustError::ConsistencyError("No stored targets found".to_string())
            })?;
            if let Some(tr) = targets_ref {
                self.verify_cached_metadata_ref(tr, Role::Targets, &signed)?;
            }
            self.verify_signed_metadata_not_expired(Role::Targets, &signed)?;
            signed
        };

        self.verify_snapshot_consistency(
            &signed_snapshot.signed,
            current_root.signed.version,
            signed_targets.signed.version,
        )?;

        Ok(TufUpdateSnapshot {
            current_root,
            rotated_roots,
            signed_timestamp,
            signed_snapshot,
            signed_targets,
            snapshot_changed,
            targets_changed,
        })
    }

    /// Persist a verified TUF update in a single transaction.
    pub(crate) fn persist_update_snapshot(
        &self,
        conn: &Connection,
        snapshot: TufUpdateSnapshot,
    ) -> TrustResult<VerifiedTufState> {
        let tx = conn.unchecked_transaction()?;

        for root in &snapshot.rotated_roots {
            self.persist_root(&tx, root)?;
            self.persist_root_keys(&tx, &root.signed)?;
        }
        if !snapshot.rotated_roots.is_empty() {
            self.persist_metadata(&tx, "root", &snapshot.current_root)?;
        }

        self.persist_metadata(&tx, "timestamp", &snapshot.signed_timestamp)?;
        if snapshot.snapshot_changed {
            self.persist_metadata(&tx, "snapshot", &snapshot.signed_snapshot)?;
        }
        if snapshot.targets_changed {
            self.persist_metadata(&tx, "targets", &snapshot.signed_targets)?;
            self.persist_targets(&tx, &snapshot.signed_targets.signed)?;
        }
        tx.commit()?;

        info!(
            "TUF update complete: root v{}, targets v{}, snapshot v{}, timestamp v{}",
            snapshot.current_root.signed.version,
            snapshot.signed_targets.signed.version,
            snapshot.signed_snapshot.signed.version,
            snapshot.signed_timestamp.signed.version,
        );

        Ok(VerifiedTufState {
            root_version: snapshot.current_root.signed.version,
            targets_version: snapshot.signed_targets.signed.version,
            snapshot_version: snapshot.signed_snapshot.signed.version,
            timestamp_version: snapshot.signed_timestamp.signed.version,
            targets: snapshot.signed_targets.signed.targets,
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
        self.verify_signed_metadata_not_expired(Role::Root, &signed_root)?;

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
    async fn check_root_rotation(
        &self,
        trusted_root: &Signed<RootMetadata>,
    ) -> TrustResult<(Signed<RootMetadata>, Vec<Signed<RootMetadata>>)> {
        let mut current = trusted_root.clone();
        let mut rotated_roots = Vec::new();

        loop {
            let next_version = current.signed.version + 1;
            let filename = format!("{next_version}.root.json");

            // Probe for the next root version; if it doesn't exist, we're done
            let Some(root_bytes) = self.try_fetch_metadata(&filename).await? else {
                break;
            };

            let new_root: Signed<RootMetadata> = serde_json::from_slice(&root_bytes)?;

            // Verify against the current trusted root's keys
            let (old_keys, old_threshold) = extract_role_keys(&current.signed, Role::Root)?;
            verify_root(&new_root, &old_keys, old_threshold)?;
            self.verify_signed_metadata_not_expired(Role::Root, &new_root)?;
            verify_version_increase(Role::Root, new_root.signed.version, current.signed.version)?;

            info!(
                "Root key rotation: v{} -> v{}",
                current.signed.version, new_root.signed.version
            );

            current = new_root;
            rotated_roots.push(current.clone());
        }

        Ok((current, rotated_roots))
    }

    /// Maximum size for TUF metadata files (10 MB)
    ///
    /// Prevents DoS attacks where a malicious server returns arbitrarily large
    /// metadata files to exhaust memory.
    const MAX_TUF_METADATA_SIZE: u64 = 10 * 1024 * 1024;

    /// Fetch metadata from the TUF base URL, optionally treating 404 as `None`.
    ///
    /// When `allow_not_found` is true, returns `Ok(None)` for HTTP 404 responses
    /// (used for probing whether a newer root version exists). When false, 404 is
    /// treated as a fetch error like any other non-success status.
    ///
    /// Enforces `MAX_TUF_METADATA_SIZE` via both Content-Length header checks
    /// and post-download body size validation.
    async fn fetch_metadata_inner(
        &self,
        filename: &str,
        allow_not_found: bool,
    ) -> TrustResult<Option<Vec<u8>>> {
        if let Ok(metadata_display) = self.tuf_location.join_display(filename) {
            debug!(
                "Fetching TUF metadata from {}: {}",
                self.tuf_base_url, metadata_display
            );
        } else {
            debug!(
                "Fetching TUF metadata from {}: {}",
                self.tuf_base_url, filename
            );
        }

        if allow_not_found {
            return self
                .tuf_location
                .try_fetch_bytes(filename, Self::MAX_TUF_METADATA_SIZE)
                .await
                .map_err(|error| {
                    TrustError::FetchError(format!("Failed to fetch {filename}: {error}"))
                });
        }

        self.tuf_location
            .fetch_bytes(filename, Self::MAX_TUF_METADATA_SIZE)
            .await
            .map(Some)
            .map_err(|error| TrustError::FetchError(format!("Failed to fetch {filename}: {error}")))
    }

    fn verify_snapshot_consistency(
        &self,
        snapshot: &SnapshotMetadata,
        expected_root_version: u64,
        expected_targets_version: u64,
    ) -> TrustResult<()> {
        match self.update_mode {
            TufUpdateMode::Generic => verify_snapshot_consistency(
                snapshot,
                expected_root_version,
                Some(expected_targets_version),
            ),
            TufUpdateMode::StaticRepo => verify_static_snapshot_consistency(
                snapshot,
                expected_root_version,
                expected_targets_version,
            ),
        }
    }

    fn verify_signed_metadata_not_expired<T: TufMetadataFields>(
        &self,
        role: Role,
        signed: &Signed<T>,
    ) -> TrustResult<()> {
        self.verify_not_expired(role, signed.signed.expires())
    }

    fn verify_cached_metadata_ref<T: serde::Serialize + TufMetadataFields>(
        &self,
        meta_ref: &MetaFile,
        role: Role,
        signed: &Signed<T>,
    ) -> TrustResult<()> {
        let cached_version = signed.signed.version();
        if meta_ref.version != cached_version {
            return Err(TrustError::ConsistencyError(format!(
                "Cached {role}.json version {} does not match parent reference v{}",
                cached_version, meta_ref.version
            )));
        }

        let json = serde_json::to_string(signed)?;
        verify_metadata_hash(meta_ref, json.as_bytes(), true)
    }

    fn verify_not_expired(
        &self,
        role: Role,
        expires: &chrono::DateTime<chrono::Utc>,
    ) -> TrustResult<()> {
        verify_not_expired(role, expires).map_err(|error| match (self.update_mode, error) {
            (TufUpdateMode::StaticRepo, TrustError::MetadataExpired { role, expires }) => {
                TrustError::VerificationFailed(format!(
                    "TUF metadata expired: {role} expired at {expires}; \
                 refresh static repository metadata with `conary publish --refresh`"
                ))
            }
            (_, error) => error,
        })
    }

    /// Try to fetch metadata, returning `None` for HTTP 404 / not found.
    ///
    /// Unlike `fetch_metadata`, this does not treat a missing file as an error.
    /// Used for probing whether a newer root version exists.
    async fn try_fetch_metadata(&self, filename: &str) -> TrustResult<Option<Vec<u8>>> {
        self.fetch_metadata_inner(filename, true).await
    }

    /// Fetch metadata from the TUF base URL.
    ///
    /// Returns an error for any non-success HTTP status, including 404.
    async fn fetch_metadata(&self, filename: &str) -> TrustResult<Vec<u8>> {
        self.fetch_metadata_inner(filename, false).await.map(|opt| {
            opt.expect(
                "fetch_metadata_inner with allow_not_found=false always returns Some on success",
            )
        })
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

    /// Load the stored persistence hash for a metadata role.
    fn load_metadata_hash(&self, conn: &Connection, role: &str) -> TrustResult<Option<String>> {
        let hash = conn
            .query_row(
                "SELECT metadata_hash FROM tuf_metadata
                 WHERE repository_id = ?1 AND role = ?2",
                params![self.repo_id, role],
                |row| row.get(0),
            )
            .optional()?;

        Ok(hash)
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

    fn load_stored_snapshot_optional(
        &self,
        conn: &Connection,
    ) -> TrustResult<Option<Signed<SnapshotMetadata>>> {
        match self.load_stored_snapshot(conn) {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(TrustError::ConsistencyError(message)) if message == "No stored snapshot found" => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
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

    fn load_stored_targets_optional(
        &self,
        conn: &Connection,
    ) -> TrustResult<Option<Signed<TargetsMetadata>>> {
        match self.load_stored_targets(conn) {
            Ok(targets) => Ok(Some(targets)),
            Err(TrustError::ConsistencyError(message)) if message == "No stored targets found" => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Persist signed metadata to the tuf_metadata table
    fn persist_metadata<T: serde::Serialize + TufMetadataFields>(
        &self,
        conn: &Connection,
        role: &str,
        signed: &Signed<T>,
    ) -> TrustResult<()> {
        let json = serde_json::to_string(signed)?;
        let hash = metadata_hash_for_persistence(signed)?;

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

fn metadata_hash_for_persistence<T: serde::Serialize + TufMetadataFields>(
    signed: &Signed<T>,
) -> TrustResult<String> {
    let json = serde_json::to_string(signed)?;
    Ok(hash::sha256(json.as_bytes()))
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
    use crate::ccs::signing::SigningKeyPair;
    use crate::db::testing::create_test_db;
    use crate::trust::ceremony::create_initial_root_single_key;
    use crate::trust::generate::{generate_snapshot, generate_targets, generate_timestamp};
    use crate::trust::keys::sign_tuf_metadata;
    use std::path::{Path, PathBuf};

    struct StaticMetadataFixture {
        _tempdir: tempfile::TempDir,
        metadata_dir: PathBuf,
        key: SigningKeyPair,
        root: Signed<RootMetadata>,
        snapshot: Signed<SnapshotMetadata>,
        targets: Signed<TargetsMetadata>,
    }

    impl StaticMetadataFixture {
        fn new() -> Self {
            let tempdir = tempfile::tempdir().unwrap();
            let metadata_dir = tempdir.path().join("metadata");
            std::fs::create_dir_all(&metadata_dir).unwrap();

            let key = SigningKeyPair::generate();
            let root = create_initial_root_single_key(&key, 365).unwrap();
            let targets = generate_targets(&[], &key, 1, 30).unwrap();
            let snapshot = generate_snapshot(root.signed.version, &targets, &key, 1, 7).unwrap();
            let timestamp = generate_timestamp(&snapshot, &key, 1, 6).unwrap();

            write_signed_metadata(&metadata_dir, "root.json", &root);
            write_signed_metadata(&metadata_dir, "targets.json", &targets);
            write_signed_metadata(&metadata_dir, "snapshot.json", &snapshot);
            write_signed_metadata(&metadata_dir, "timestamp.json", &timestamp);

            Self {
                _tempdir: tempdir,
                metadata_dir,
                key,
                root,
                snapshot,
                targets,
            }
        }

        fn client(&self, repo_id: i64) -> TufClient {
            let metadata_url = format!("file://{}", self.metadata_dir.display());
            TufClient::new_static(repo_id, "file:///unused", Some(&metadata_url)).unwrap()
        }

        fn generic_client(&self, repo_id: i64) -> TufClient {
            let metadata_url = format!("file://{}", self.metadata_dir.display());
            TufClient::new(repo_id, "file:///unused", Some(&metadata_url)).unwrap()
        }

        fn bootstrap(&self, client: &TufClient, conn: &Connection) {
            let root_json = serde_json::to_vec(&self.root).unwrap();
            client.bootstrap(conn, &root_json).unwrap();
        }

        fn write_greater_snapshot_without_root(&self) {
            let mut snapshot =
                generate_snapshot(self.root.signed.version, &self.targets, &self.key, 2, 7)
                    .unwrap();
            snapshot.signed.meta.remove("root.json");
            snapshot.signatures = vec![sign_tuf_metadata(&self.key, &snapshot.signed).unwrap()];
            let timestamp = generate_timestamp(&snapshot, &self.key, 2, 6).unwrap();

            write_signed_metadata(&self.metadata_dir, "snapshot.json", &snapshot);
            write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
        }

        fn write_expired_timestamp(&self) {
            let snapshot =
                generate_snapshot(self.root.signed.version, &self.targets, &self.key, 1, 7)
                    .unwrap();
            let timestamp = generate_timestamp(&snapshot, &self.key, 1, -1).unwrap();
            write_signed_metadata(&self.metadata_dir, "snapshot.json", &snapshot);
            write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
        }

        fn write_same_version_timestamp_with_different_bytes(&self) {
            let snapshot =
                generate_snapshot(self.root.signed.version, &self.targets, &self.key, 1, 7)
                    .unwrap();
            let mut timestamp = generate_timestamp(&snapshot, &self.key, 1, 6).unwrap();
            timestamp.signed.expires += chrono::Duration::seconds(1);
            timestamp.signatures = vec![sign_tuf_metadata(&self.key, &timestamp.signed).unwrap()];
            write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
        }

        fn write_timestamp_for_stored_snapshot_version(
            &self,
            conn: &Connection,
            repo_id: i64,
            version: u64,
        ) {
            let snapshot: Signed<SnapshotMetadata> =
                load_stored_signed_metadata(conn, repo_id, "snapshot");
            let timestamp = generate_timestamp(&snapshot, &self.key, version, 6).unwrap();
            write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
        }

        fn write_timestamp_for_cached_snapshot_with_bad_hash(&self, version: u64) {
            let mut timestamp = generate_timestamp(&self.snapshot, &self.key, version, 6).unwrap();
            set_bad_hash(timestamp.signed.meta.get_mut("snapshot.json").unwrap());
            timestamp.signatures = vec![sign_tuf_metadata(&self.key, &timestamp.signed).unwrap()];
            write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
        }

        fn write_timestamp_for_cached_snapshot_without_hash(&self, version: u64) {
            let mut timestamp = generate_timestamp(&self.snapshot, &self.key, version, 6).unwrap();
            timestamp
                .signed
                .meta
                .get_mut("snapshot.json")
                .unwrap()
                .hashes = None;
            timestamp.signatures = vec![sign_tuf_metadata(&self.key, &timestamp.signed).unwrap()];
            write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
        }

        fn write_snapshot_for_cached_targets_with_bad_hash(&self) {
            let mut snapshot =
                generate_snapshot(self.root.signed.version, &self.targets, &self.key, 2, 7)
                    .unwrap();
            set_bad_hash(snapshot.signed.meta.get_mut("targets.json").unwrap());
            snapshot.signatures = vec![sign_tuf_metadata(&self.key, &snapshot.signed).unwrap()];
            let timestamp = generate_timestamp(&snapshot, &self.key, 2, 6).unwrap();

            write_signed_metadata(&self.metadata_dir, "snapshot.json", &snapshot);
            write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
        }

        fn write_snapshot_for_cached_targets_without_hash(&self) {
            let mut snapshot =
                generate_snapshot(self.root.signed.version, &self.targets, &self.key, 2, 7)
                    .unwrap();
            snapshot.signed.meta.get_mut("targets.json").unwrap().hashes = None;
            snapshot.signatures = vec![sign_tuf_metadata(&self.key, &snapshot.signed).unwrap()];
            let timestamp = generate_timestamp(&snapshot, &self.key, 2, 6).unwrap();

            write_signed_metadata(&self.metadata_dir, "snapshot.json", &snapshot);
            write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
        }

        fn expire_stored_snapshot(&self, conn: &Connection, repo_id: i64) {
            let mut snapshot: Signed<SnapshotMetadata> =
                load_stored_signed_metadata(conn, repo_id, "snapshot");
            snapshot.signed.expires = chrono::Utc::now() - chrono::Duration::hours(1);
            snapshot.signatures = vec![sign_tuf_metadata(&self.key, &snapshot.signed).unwrap()];
            update_stored_signed_metadata(conn, repo_id, "snapshot", &snapshot);

            let timestamp = generate_timestamp(&snapshot, &self.key, 1, 6).unwrap();
            write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
            update_stored_signed_metadata(conn, repo_id, "timestamp", &timestamp);
        }

        fn expire_stored_targets(&self, conn: &Connection, repo_id: i64) {
            let mut targets: Signed<TargetsMetadata> =
                load_stored_signed_metadata(conn, repo_id, "targets");
            targets.signed.expires = chrono::Utc::now() - chrono::Duration::hours(1);
            targets.signatures = vec![sign_tuf_metadata(&self.key, &targets.signed).unwrap()];
            update_stored_signed_metadata(conn, repo_id, "targets", &targets);

            let mut snapshot: Signed<SnapshotMetadata> =
                load_stored_signed_metadata(conn, repo_id, "snapshot");
            let targets_json = serde_json::to_string(&targets).unwrap();
            let mut hashes = std::collections::BTreeMap::new();
            hashes.insert(
                "sha256".to_string(),
                metadata_hash_for_persistence(&targets).unwrap(),
            );
            let targets_ref = snapshot.signed.meta.get_mut("targets.json").unwrap();
            targets_ref.length = Some(targets_json.len() as u64);
            targets_ref.hashes = Some(hashes);
            snapshot.signatures = vec![sign_tuf_metadata(&self.key, &snapshot.signed).unwrap()];
            update_stored_signed_metadata(conn, repo_id, "snapshot", &snapshot);
        }

        fn alter_stored_targets_same_version(&self, conn: &Connection, repo_id: i64) {
            let mut targets: Signed<TargetsMetadata> =
                load_stored_signed_metadata(conn, repo_id, "targets");
            targets.signed.expires += chrono::Duration::seconds(1);
            targets.signatures = vec![sign_tuf_metadata(&self.key, &targets.signed).unwrap()];
            update_stored_signed_metadata(conn, repo_id, "targets", &targets);
        }

        fn expire_stored_root(&self, conn: &Connection, repo_id: i64) {
            let mut root: Signed<RootMetadata> = conn
                .query_row(
                    "SELECT signed_metadata FROM tuf_roots
                     WHERE repository_id = ?1
                     ORDER BY version DESC LIMIT 1",
                    params![repo_id],
                    |row| {
                        let json: String = row.get(0)?;
                        Ok(serde_json::from_str(&json).unwrap())
                    },
                )
                .unwrap();
            root.signed.expires = chrono::Utc::now() - chrono::Duration::hours(1);
            root.signatures = vec![sign_tuf_metadata(&self.key, &root.signed).unwrap()];
            let json = serde_json::to_string(&root).unwrap();
            conn.execute(
                "UPDATE tuf_roots
                 SET signed_metadata = ?1, expires_at = ?2
                 WHERE repository_id = ?3 AND version = ?4",
                params![
                    json,
                    root.signed.expires.to_rfc3339(),
                    repo_id,
                    root.signed.version as i64,
                ],
            )
            .unwrap();
            update_stored_signed_metadata(conn, repo_id, "root", &root);
        }
    }

    fn write_signed_metadata<T: serde::Serialize>(
        metadata_dir: &Path,
        filename: &str,
        signed: &Signed<T>,
    ) {
        let bytes = serde_json::to_vec(signed).unwrap();
        std::fs::write(metadata_dir.join(filename), bytes).unwrap();
    }

    fn set_bad_hash(meta: &mut crate::trust::MetaFile) {
        let mut hashes = std::collections::BTreeMap::new();
        hashes.insert("sha256".to_string(), "bad-hash".to_string());
        meta.hashes = Some(hashes);
    }

    fn load_stored_signed_metadata<T: serde::de::DeserializeOwned>(
        conn: &Connection,
        repo_id: i64,
        role: &str,
    ) -> Signed<T> {
        let json: String = conn
            .query_row(
                "SELECT signed_metadata FROM tuf_metadata WHERE repository_id = ?1 AND role = ?2",
                params![repo_id, role],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_str(&json).unwrap()
    }

    fn update_stored_signed_metadata<T: serde::Serialize + TufMetadataFields>(
        conn: &Connection,
        repo_id: i64,
        role: &str,
        signed: &Signed<T>,
    ) {
        let json = serde_json::to_string(signed).unwrap();
        let hash = metadata_hash_for_persistence(signed).unwrap();
        conn.execute(
            "UPDATE tuf_metadata
             SET signed_metadata = ?1, metadata_hash = ?2, expires_at = ?3
             WHERE repository_id = ?4 AND role = ?5",
            params![
                json,
                hash,
                signed.signed.expires().to_rfc3339(),
                repo_id,
                role,
            ],
        )
        .unwrap();
    }

    fn stored_metadata_version(conn: &Connection, repo_id: i64, role: &str) -> i64 {
        conn.query_row(
            "SELECT version FROM tuf_metadata WHERE repository_id = ?1 AND role = ?2",
            params![repo_id, role],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn insert_test_repository(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO repositories (name, url) VALUES (?1, ?2)",
            params!["static-test", "file:///static-test"],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

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

    #[tokio::test]
    async fn static_file_repo_update_accepts_identical_timestamp_bytes_without_rollback() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);

        let first = client.update(&conn).await.unwrap();
        assert_eq!(first.timestamp_version, 1);

        let second = client.update(&conn).await.unwrap();
        assert_eq!(
            (
                second.timestamp_version,
                second.snapshot_version,
                second.targets_version
            ),
            (1, 1, 1)
        );
    }

    #[tokio::test]
    async fn static_equal_timestamp_rejects_altered_cached_targets_hash() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);
        client.update(&conn).await.unwrap();

        fixture.alter_stored_targets_same_version(&conn, repo_id);
        let err = client.update(&conn).await.unwrap_err();
        assert!(err.to_string().contains("Hash mismatch"));
    }

    #[tokio::test]
    async fn static_update_rejects_equal_timestamp_version_with_different_bytes() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);
        client.update(&conn).await.unwrap();

        fixture.write_same_version_timestamp_with_different_bytes();
        let err = client.update(&conn).await.unwrap_err();
        assert!(err.to_string().contains("metadata bytes/hash differ"));
    }

    #[tokio::test]
    async fn generic_equal_timestamp_version_remains_rollback_even_when_hash_matches() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.generic_client(repo_id);
        fixture.bootstrap(&client, &conn);
        client.update(&conn).await.unwrap();

        let err = client.update(&conn).await.unwrap_err();
        assert!(matches!(
            err,
            TrustError::RollbackAttack {
                role,
                new: 1,
                stored: 1
            } if role == "timestamp"
        ));
    }

    #[tokio::test]
    async fn static_update_rejects_greater_snapshot_missing_root_before_persistence() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);
        client.update(&conn).await.unwrap();

        fixture.write_greater_snapshot_without_root();
        let err = client.update(&conn).await.unwrap_err();
        assert!(err.to_string().contains("root.json"));

        let stored_timestamp_version: i64 = conn
            .query_row(
                "SELECT version FROM tuf_metadata WHERE repository_id = ?1 AND role = 'timestamp'",
                params![repo_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_timestamp_version, 1);
    }

    #[tokio::test]
    async fn static_update_expired_metadata_names_publish_refresh_remedy() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        fixture.write_expired_timestamp();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);

        let err = client.update(&conn).await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("timestamp"));
        assert!(message.contains("conary publish --refresh"));
    }

    #[tokio::test]
    async fn static_update_rechecks_cached_root_expiry_with_refresh_remedy() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);

        fixture.expire_stored_root(&conn, repo_id);
        let err = client.update(&conn).await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("root"));
        assert!(message.contains("conary publish --refresh"));
    }

    #[tokio::test]
    async fn static_equal_timestamp_rechecks_cached_snapshot_expiry_with_refresh_remedy() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);
        client.update(&conn).await.unwrap();

        fixture.expire_stored_snapshot(&conn, repo_id);
        let err = client.update(&conn).await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("snapshot"));
        assert!(message.contains("conary publish --refresh"));
    }

    #[tokio::test]
    async fn static_greater_timestamp_rechecks_cached_targets_expiry_before_persistence() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);
        client.update(&conn).await.unwrap();

        fixture.expire_stored_targets(&conn, repo_id);
        fixture.write_timestamp_for_stored_snapshot_version(&conn, repo_id, 2);
        let err = client.update(&conn).await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("targets"));
        assert!(message.contains("conary publish --refresh"));

        assert_eq!(stored_metadata_version(&conn, repo_id, "timestamp"), 1);
    }

    #[tokio::test]
    async fn static_update_rejects_bad_timestamp_hash_for_cached_snapshot_without_persistence() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);
        client.update(&conn).await.unwrap();

        fixture.write_timestamp_for_cached_snapshot_with_bad_hash(2);
        let err = client.update(&conn).await.unwrap_err();
        assert!(err.to_string().contains("Hash mismatch"));
        assert_eq!(stored_metadata_version(&conn, repo_id, "timestamp"), 1);
    }

    #[tokio::test]
    async fn static_update_rejects_missing_timestamp_hash_for_cached_snapshot_without_persistence()
    {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);
        client.update(&conn).await.unwrap();

        fixture.write_timestamp_for_cached_snapshot_without_hash(2);
        let err = client.update(&conn).await.unwrap_err();
        assert!(err.to_string().contains("missing required sha256 hash"));
        assert_eq!(stored_metadata_version(&conn, repo_id, "timestamp"), 1);
    }

    #[tokio::test]
    async fn static_update_rejects_bad_snapshot_hash_for_cached_targets_without_persistence() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);
        client.update(&conn).await.unwrap();

        fixture.write_snapshot_for_cached_targets_with_bad_hash();
        let err = client.update(&conn).await.unwrap_err();
        assert!(err.to_string().contains("Hash mismatch"));
        assert_eq!(stored_metadata_version(&conn, repo_id, "timestamp"), 1);
        assert_eq!(stored_metadata_version(&conn, repo_id, "snapshot"), 1);
    }

    #[tokio::test]
    async fn static_update_rejects_missing_snapshot_hash_for_cached_targets_without_persistence() {
        let (_db, conn) = create_test_db();
        let repo_id = insert_test_repository(&conn);
        let fixture = StaticMetadataFixture::new();
        let client = fixture.client(repo_id);
        fixture.bootstrap(&client, &conn);
        client.update(&conn).await.unwrap();

        fixture.write_snapshot_for_cached_targets_without_hash();
        let err = client.update(&conn).await.unwrap_err();
        assert!(err.to_string().contains("missing required sha256 hash"));
        assert_eq!(stored_metadata_version(&conn, repo_id, "timestamp"), 1);
        assert_eq!(stored_metadata_version(&conn, repo_id, "snapshot"), 1);
    }
}
