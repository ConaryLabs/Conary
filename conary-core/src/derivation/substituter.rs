// conary-core/src/derivation/substituter.rs

//! Derivation substituter client.
//!
//! Queries remote Remi peers for pre-built derivation outputs by derivation ID.
//! On a cache hit the caller receives an `OutputManifest`; it can then call
//! `fetch_missing_objects` to pull any CAS blobs that are not yet local.
//!
//! Peer selection uses priority ordering combined with exponential backoff so
//! that transiently-failing peers are skipped without being permanently removed.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::StatusCode;
use rusqlite::Connection;
use tracing::{debug, warn};

use crate::filesystem::CasStore;

use super::output::OutputManifest;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors produced by the substituter client.
#[derive(Debug, thiserror::Error)]
pub enum SubstituterError {
    /// An HTTP-level error (connection refused, timeout, non-success status).
    #[error("HTTP error: {0}")]
    Http(String),

    /// A parse error (invalid TOML body or unexpected JSON shape).
    #[error("parse error: {0}")]
    Parse(String),

    /// An I/O error while writing fetched objects to the CAS.
    #[error("I/O error: {0}")]
    Io(String),

    /// No substituter peers are configured in the database.
    #[error("no substituter peers configured")]
    NoPeers,
}

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------

/// A remote peer that may hold pre-built derivation outputs.
pub struct SubstituterPeer {
    /// Base URL of the remote Remi server (e.g. `https://packages.conary.io`).
    pub endpoint: String,
    /// Lower value = higher priority (peers are tried in ascending order).
    pub priority: u32,
}

/// Result of querying the substituter cache for a single derivation.
pub enum CacheQueryResult {
    /// A pre-built manifest was found on `peer`.
    Hit { manifest: OutputManifest, peer: String },
    /// No peer holds a pre-built output for this derivation.
    Miss,
}

/// Summary of objects fetched during `fetch_missing_objects`.
pub struct FetchReport {
    /// Number of CAS objects that were actually downloaded (already-present
    /// objects are not counted).
    pub objects_fetched: u64,
    /// Total bytes transferred across all object downloads.
    pub bytes_transferred: u64,
}

// ---------------------------------------------------------------------------
// Internal health tracking
// ---------------------------------------------------------------------------

struct PeerHealth {
    consecutive_failures: u32,
    last_failure: Option<Instant>,
}

impl PeerHealth {
    fn new() -> Self {
        Self {
            consecutive_failures: 0,
            last_failure: None,
        }
    }

    /// Returns `true` when the peer is in its back-off window and should be
    /// skipped.  The back-off is `2^failures` seconds, capped at 64 s (2^6).
    fn is_backed_off(&self) -> bool {
        if let Some(last) = self.last_failure {
            let backoff = Duration::from_secs(2u64.pow(self.consecutive_failures.min(6)));
            last.elapsed() < backoff
        } else {
            false
        }
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_failure = None;
    }

    fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        self.last_failure = Some(Instant::now());
    }
}

// ---------------------------------------------------------------------------
// Main type
// ---------------------------------------------------------------------------

/// Client that queries remote Remi servers for pre-built derivation outputs.
pub struct DerivationSubstituter {
    client: Client,
    peers: Vec<SubstituterPeer>,
    peer_health: HashMap<String, PeerHealth>,
}

impl DerivationSubstituter {
    /// Seed the `substituter_peers` table with a list of endpoints.
    ///
    /// Endpoints that are already present are silently ignored (upsert by
    /// primary key).  This is safe to call on every startup.
    ///
    /// # Errors
    ///
    /// Returns a `SubstituterError::Io` if the database write fails.
    pub fn seed_peers(conn: &Connection, endpoints: &[String]) -> Result<(), SubstituterError> {
        for endpoint in endpoints {
            conn.execute(
                "INSERT OR IGNORE INTO substituter_peers (endpoint, priority) VALUES (?1, 0)",
                rusqlite::params![endpoint],
            )
            .map_err(|e| SubstituterError::Io(e.to_string()))?;
        }
        Ok(())
    }

    /// Load peers from the `substituter_peers` table and construct the client.
    ///
    /// # Errors
    ///
    /// Returns `SubstituterError::NoPeers` when the table is empty.
    /// Returns `SubstituterError::Io` on a database error.
    pub fn from_db(conn: &Connection) -> Result<Self, SubstituterError> {
        let mut stmt = conn
            .prepare(
                "SELECT endpoint, priority FROM substituter_peers ORDER BY priority ASC",
            )
            .map_err(|e| SubstituterError::Io(e.to_string()))?;

        let peers: Vec<SubstituterPeer> = stmt
            .query_map([], |row| {
                Ok(SubstituterPeer {
                    endpoint: row.get(0)?,
                    priority: row.get::<_, i64>(1)? as u32,
                })
            })
            .map_err(|e| SubstituterError::Io(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| SubstituterError::Io(e.to_string()))?;

        if peers.is_empty() {
            return Err(SubstituterError::NoPeers);
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| SubstituterError::Http(e.to_string()))?;

        Ok(Self {
            client,
            peers,
            peer_health: HashMap::new(),
        })
    }

    /// Query peers in priority order for a pre-built output.
    ///
    /// Returns `CacheQueryResult::Hit` on the first peer that has the manifest,
    /// or `CacheQueryResult::Miss` if no peer has it.
    pub fn query(&mut self, derivation_id: &str) -> CacheQueryResult {
        for peer in &self.peers {
            let health = self
                .peer_health
                .entry(peer.endpoint.clone())
                .or_insert_with(PeerHealth::new);

            if health.is_backed_off() {
                debug!(
                    "Skipping backed-off peer {} for derivation {}",
                    peer.endpoint, derivation_id
                );
                continue;
            }

            let url = format!("{}/v1/derivations/{}", peer.endpoint, derivation_id);
            let endpoint = peer.endpoint.clone();

            match self.client.get(&url).send() {
                Ok(resp) if resp.status() == StatusCode::OK => {
                    match resp.text() {
                        Ok(body) => match toml::from_str::<OutputManifest>(&body) {
                            Ok(manifest) => {
                                self.peer_health
                                    .entry(endpoint.clone())
                                    .or_insert_with(PeerHealth::new)
                                    .record_success();
                                return CacheQueryResult::Hit {
                                    manifest,
                                    peer: endpoint,
                                };
                            }
                            Err(e) => {
                                warn!("Failed to parse manifest from {}: {}", endpoint, e);
                                self.peer_health
                                    .entry(endpoint)
                                    .or_insert_with(PeerHealth::new)
                                    .record_failure();
                            }
                        },
                        Err(e) => {
                            warn!("Failed to read response body from {}: {}", endpoint, e);
                            self.peer_health
                                .entry(endpoint)
                                .or_insert_with(PeerHealth::new)
                                .record_failure();
                        }
                    }
                }
                Ok(resp) if resp.status() == StatusCode::NOT_FOUND => {
                    debug!(
                        "Derivation {} not found on peer {}",
                        derivation_id, endpoint
                    );
                    // 404 is a clean miss -- do not count as a failure.
                }
                Ok(resp) => {
                    warn!(
                        "Unexpected status {} from peer {} for derivation {}",
                        resp.status(),
                        endpoint,
                        derivation_id
                    );
                    self.peer_health
                        .entry(endpoint)
                        .or_insert_with(PeerHealth::new)
                        .record_failure();
                }
                Err(e) => {
                    warn!("HTTP request to {} failed: {}", endpoint, e);
                    self.peer_health
                        .entry(endpoint)
                        .or_insert_with(PeerHealth::new)
                        .record_failure();
                }
            }
        }

        CacheQueryResult::Miss
    }

    /// Download any CAS objects referenced by `manifest` that are not already
    /// present in `cas`.
    ///
    /// Objects are fetched from `peer_endpoint` at
    /// `GET {peer_endpoint}/v1/chunks/{hash}`.
    ///
    /// # Errors
    ///
    /// Returns `SubstituterError::Http` on network failure and
    /// `SubstituterError::Io` if the CAS write fails.
    pub fn fetch_missing_objects(
        &self,
        manifest: &OutputManifest,
        cas: &CasStore,
        peer_endpoint: &str,
    ) -> Result<FetchReport, SubstituterError> {
        let mut report = FetchReport {
            objects_fetched: 0,
            bytes_transferred: 0,
        };

        for file in &manifest.files {
            if cas.exists(&file.hash) {
                debug!("CAS hit for chunk {}", file.hash);
                continue;
            }

            let url = format!("{}/v1/chunks/{}", peer_endpoint, file.hash);
            let resp = self
                .client
                .get(&url)
                .send()
                .map_err(|e| SubstituterError::Http(e.to_string()))?;

            if !resp.status().is_success() {
                return Err(SubstituterError::Http(format!(
                    "chunk fetch returned {} for hash {}",
                    resp.status(),
                    file.hash
                )));
            }

            let bytes = resp
                .bytes()
                .map_err(|e| SubstituterError::Http(e.to_string()))?;

            let byte_count = bytes.len() as u64;

            cas.store(&bytes)
                .map_err(|e| SubstituterError::Io(e.to_string()))?;

            report.objects_fetched += 1;
            report.bytes_transferred += byte_count;

            debug!(
                "Fetched chunk {} ({} bytes) from {}",
                file.hash, byte_count, peer_endpoint
            );
        }

        Ok(report)
    }

    /// Upload an `OutputManifest` to a remote peer.
    ///
    /// Sends `PUT {endpoint}/v1/derivations/{derivation_id}` with the
    /// TOML-serialized manifest as the body and a bearer `token` for auth.
    ///
    /// # Errors
    ///
    /// Returns `SubstituterError::Parse` if the manifest cannot be serialized.
    /// Returns `SubstituterError::Http` on network or server errors.
    pub fn publish(
        &self,
        derivation_id: &str,
        manifest: &OutputManifest,
        endpoint: &str,
        token: &str,
    ) -> Result<(), SubstituterError> {
        let body =
            toml::to_string(manifest).map_err(|e| SubstituterError::Parse(e.to_string()))?;

        let url = format!("{}/v1/derivations/{}", endpoint, derivation_id);
        let resp = self
            .client
            .put(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/toml")
            .body(body)
            .send()
            .map_err(|e| SubstituterError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(SubstituterError::Http(format!(
                "publish returned {} for derivation {}",
                resp.status(),
                derivation_id
            )));
        }

        debug!(
            "Published derivation {} to {}",
            derivation_id, endpoint
        );

        Ok(())
    }

    /// Query a single peer for multiple derivation IDs in one round trip.
    ///
    /// Sends `POST {endpoint}/v1/derivations/probe` with a JSON array of IDs
    /// and parses the response as `HashMap<String, bool>`.
    ///
    /// # Errors
    ///
    /// Returns `SubstituterError::Http` on network or server errors.
    /// Returns `SubstituterError::Parse` if the response body is not valid JSON.
    pub fn batch_probe(
        &self,
        derivation_ids: &[String],
        endpoint: &str,
    ) -> Result<HashMap<String, bool>, SubstituterError> {
        let url = format!("{}/v1/derivations/probe", endpoint);
        let resp = self
            .client
            .post(&url)
            .json(derivation_ids)
            .send()
            .map_err(|e| SubstituterError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(SubstituterError::Http(format!(
                "batch_probe returned {} from {}",
                resp.status(),
                endpoint
            )));
        }

        let result: HashMap<String, bool> = resp
            .json()
            .map_err(|e| SubstituterError::Parse(e.to_string()))?;

        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_health_backoff() {
        let mut h = PeerHealth {
            consecutive_failures: 0,
            last_failure: None,
        };
        assert!(!h.is_backed_off());
        h.record_failure();
        assert!(h.is_backed_off());
        h.record_success();
        assert!(!h.is_backed_off());
    }

    #[test]
    fn cache_query_result_variants() {
        let miss = CacheQueryResult::Miss;
        assert!(matches!(miss, CacheQueryResult::Miss));
    }

    #[test]
    fn seed_peers_inserts_new() {
        use rusqlite::Connection;
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::migrate(&conn).unwrap();

        DerivationSubstituter::seed_peers(&conn, &["https://a.com".to_owned()]).unwrap();
        DerivationSubstituter::seed_peers(
            &conn,
            &[
                "https://a.com".to_owned(),
                "https://b.com".to_owned(),
            ],
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM substituter_peers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "should have 2 peers (a.com not duplicated)");
    }

    #[test]
    fn from_db_returns_no_peers_error_when_empty() {
        use rusqlite::Connection;
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::migrate(&conn).unwrap();
        let result = DerivationSubstituter::from_db(&conn);
        assert!(matches!(result, Err(SubstituterError::NoPeers)));
    }
}
