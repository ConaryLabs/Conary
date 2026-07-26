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
        let targets_version = i64::try_from(targets.version).map_err(|_| {
            TrustError::ConsistencyError(format!(
                "Targets metadata version {} exceeds persisted range",
                targets.version
            ))
        })?;
        let rows = targets
            .targets
            .iter()
            .map(|(path, desc)| {
                let sha256 = desc.hashes.get("sha256").ok_or_else(|| {
                    TrustError::ConsistencyError(format!(
                        "Verified target {path:?} is missing its required sha256 digest"
                    ))
                })?;
                let length = i64::try_from(desc.length).map_err(|_| {
                    TrustError::ConsistencyError(format!(
                        "Verified target {path:?} length {} exceeds persisted range",
                        desc.length
                    ))
                })?;
                Ok((path, sha256, length))
            })
            .collect::<TrustResult<Vec<_>>>()?;

        // Delete old targets
        conn.execute(
            "DELETE FROM tuf_targets WHERE repository_id = ?1",
            params![self.repo_id],
        )?;

        let mut stmt = conn.prepare(
            "INSERT INTO tuf_targets (repository_id, target_path, sha256, length, custom_json, targets_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        for (path, sha256, length) in rows {
            stmt.execute(params![
                self.repo_id,
                path,
                sha256,
                length,
                Option::<String>::None, // custom_json - not used yet
                targets_version,
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
mod tests;
