// conary-core/src/repository/substituter.rs

//! Nix-style substituter chain for ordered package source resolution
//!
//! Sources are tried in order. First source to provide the requested
//! data wins. Builds on the ChunkFetcher pattern from chunk_fetcher.rs
//! but operates at a higher level with package-aware sources.

use crate::db::models::federation_peer::{self, FederationPeer};
use crate::error::{Error, Result};
use crate::repository::chunk_fetcher::{ChunkFetcher, HttpChunkFetcher, LocalCacheFetcher};
use rusqlite::Connection;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, info, warn};

/// A source in the substituter chain
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubstituterSource {
    /// Local filesystem cache
    LocalCache { cache_dir: PathBuf },
    /// CAS federation peers
    Federation { tier: String },
    /// Remi server (converts on demand)
    Remi { endpoint: String, distro: String },
}

/// Ordered chain of package sources
pub struct SubstituterChain {
    sources: Vec<SubstituterSource>,
}

/// Prepared federation peers keyed by tier for async substituter use.
pub type PreparedFederationPeers = HashMap<String, Vec<FederationPeer>>;

/// Success/failure telemetry emitted by federation fetch attempts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerFetchMetric {
    pub peer_id: String,
    pub latency_ms: i64,
    pub succeeded: bool,
}

/// Result of resolving a single chunk through the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstituterResult {
    /// Which source provided the data
    pub source_name: String,
    /// Source index in the chain
    pub source_index: usize,
    /// Federation peer telemetry gathered during resolution
    pub peer_metrics: Vec<PeerFetchMetric>,
}

/// Result of resolving multiple chunks through the chain.
#[derive(Debug, Default)]
pub struct SubstituterBatchResult {
    pub chunks: HashMap<String, Vec<u8>>,
    pub peer_metrics: Vec<PeerFetchMetric>,
}

impl SubstituterBatchResult {
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

struct SourceFetchAttempt {
    result: Result<Vec<u8>>,
    peer_metrics: Vec<PeerFetchMetric>,
}

impl SubstituterSource {
    /// Returns a human-readable name for this source
    pub fn name(&self) -> &str {
        match self {
            Self::LocalCache { .. } => "local-cache",
            Self::Federation { .. } => "federation",
            Self::Remi { .. } => "remi",
        }
    }
}

impl SubstituterChain {
    /// Create a new substituter chain with the given sources
    pub fn new(sources: Vec<SubstituterSource>) -> Self {
        Self { sources }
    }

    /// Append a source to the end of the chain
    pub fn add_source(&mut self, source: SubstituterSource) {
        self.sources.push(source);
    }

    /// List the configured sources
    pub fn sources(&self) -> &[SubstituterSource] {
        &self.sources
    }

    /// Number of sources in the chain
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Whether the chain has no sources
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Preload federation peers from the database into an owned async-safe map.
    pub fn prepare_federation_peers(&self, conn: &Connection) -> Result<PreparedFederationPeers> {
        let tiers = self
            .sources
            .iter()
            .filter_map(|source| match source {
                SubstituterSource::Federation { tier } => Some(tier.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();

        let mut prepared = PreparedFederationPeers::new();
        for tier in tiers {
            prepared.insert(
                tier.clone(),
                federation_peer::list_enabled_for_tier(conn, &tier)?,
            );
        }

        Ok(prepared)
    }

    /// Apply federation peer metrics emitted by async resolution.
    pub fn apply_peer_metrics(conn: &Connection, metrics: &[PeerFetchMetric]) -> Result<()> {
        for metric in metrics {
            if metric.succeeded {
                federation_peer::record_success(conn, &metric.peer_id, metric.latency_ms)?;
            } else {
                federation_peer::record_failure(conn, &metric.peer_id)?;
            }
        }
        Ok(())
    }

    /// Try each source in order for a single chunk.
    pub async fn resolve_chunk(
        &self,
        hash: &str,
        federation_peers: Option<&PreparedFederationPeers>,
    ) -> Result<(Vec<u8>, SubstituterResult)> {
        if self.sources.is_empty() {
            return Err(Error::NotFound(format!(
                "No sources in substituter chain for chunk {hash}"
            )));
        }

        let mut peer_metrics = Vec::new();
        let mut last_error = None;

        for (idx, source) in self.sources.iter().enumerate() {
            let name = source.name();
            debug!("Trying source {} ({}) for chunk {}", idx, name, hash);

            let attempt = self.fetch_from_source(source, hash, federation_peers).await;
            peer_metrics.extend(attempt.peer_metrics);

            match attempt.result {
                Ok(data) => {
                    info!(
                        "Source {} ({}) provided chunk {} ({} bytes)",
                        idx,
                        name,
                        hash,
                        data.len()
                    );
                    return Ok((
                        data,
                        SubstituterResult {
                            source_name: name.to_string(),
                            source_index: idx,
                            peer_metrics,
                        },
                    ));
                }
                Err(e) => {
                    if matches!(e, Error::DurableChunkUnavailable { .. }) {
                        return Err(e);
                    }
                    debug!(
                        "Source {} ({}) could not provide chunk {}: {}",
                        idx, name, hash, e
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| Error::NotFound(format!("No source could provide chunk {hash}"))))
    }

    /// Batch resolution of multiple chunks with ordered source fallback.
    pub async fn resolve_chunks(
        &self,
        hashes: &[String],
        federation_peers: Option<&PreparedFederationPeers>,
    ) -> Result<SubstituterBatchResult> {
        if hashes.is_empty() {
            return Ok(SubstituterBatchResult::default());
        }

        info!(
            "Resolving {} chunks through {} sources",
            hashes.len(),
            self.sources.len()
        );

        let mut resolved: HashMap<String, Vec<u8>> = HashMap::new();
        let mut peer_metrics = Vec::new();
        let mut remaining: Vec<&String> = hashes.iter().collect();

        for (idx, source) in self.sources.iter().enumerate() {
            if remaining.is_empty() {
                break;
            }

            let name = source.name();
            debug!(
                "Trying source {} ({}) for {} remaining chunks",
                idx,
                name,
                remaining.len()
            );

            let mut newly_resolved = HashSet::new();

            for hash in &remaining {
                let attempt = self.fetch_from_source(source, hash, federation_peers).await;
                peer_metrics.extend(attempt.peer_metrics);

                match attempt.result {
                    Ok(data) => {
                        debug!(
                            "Source {} provided chunk {} ({} bytes)",
                            name,
                            hash,
                            data.len()
                        );
                        resolved.insert((*hash).clone(), data);
                        newly_resolved.insert((*hash).as_str());
                    }
                    Err(e) => {
                        if matches!(e, Error::DurableChunkUnavailable { .. }) {
                            return Err(e);
                        }
                        debug!("Source {} could not provide chunk {}: {}", name, hash, e);
                    }
                }
            }

            remaining.retain(|h| !newly_resolved.contains(h.as_str()));
        }

        if resolved.is_empty() && !hashes.is_empty() {
            return Err(Error::NotFound(format!(
                "No source could provide any of the {} requested chunks",
                hashes.len()
            )));
        }

        if !remaining.is_empty() {
            warn!(
                "{} of {} chunks could not be resolved from any source",
                remaining.len(),
                hashes.len()
            );
        }

        info!("Resolved {}/{} chunks", resolved.len(), hashes.len());
        Ok(SubstituterBatchResult {
            chunks: resolved,
            peer_metrics,
        })
    }

    async fn fetch_from_source(
        &self,
        source: &SubstituterSource,
        hash: &str,
        federation_peers: Option<&PreparedFederationPeers>,
    ) -> SourceFetchAttempt {
        match source {
            SubstituterSource::LocalCache { cache_dir } => {
                self.fetch_from_local_cache(cache_dir, hash).await
            }
            SubstituterSource::Federation { tier } => {
                self.fetch_from_federation(tier, hash, federation_peers)
                    .await
            }
            SubstituterSource::Remi {
                endpoint,
                distro: _,
            } => self.fetch_from_remi(endpoint, hash).await,
        }
    }

    async fn fetch_from_local_cache(&self, cache_dir: &Path, hash: &str) -> SourceFetchAttempt {
        if hash.len() < 2 {
            return SourceFetchAttempt {
                result: Err(Error::NotFound(format!("Invalid chunk hash: {hash}"))),
                peer_metrics: Vec::new(),
            };
        }

        let fetcher = LocalCacheFetcher::new(cache_dir);
        SourceFetchAttempt {
            result: fetcher.fetch(hash).await,
            peer_metrics: Vec::new(),
        }
    }

    async fn fetch_from_remi(&self, endpoint: &str, hash: &str) -> SourceFetchAttempt {
        let result = async {
            let fetcher = HttpChunkFetcher::new(endpoint)?;
            let data = fetcher.fetch(hash).await?;
            self.cache_remote_hit(hash, &data).await?;
            Ok(data)
        }
        .await;

        SourceFetchAttempt {
            result,
            peer_metrics: Vec::new(),
        }
    }

    async fn fetch_from_federation(
        &self,
        tier: &str,
        hash: &str,
        federation_peers: Option<&PreparedFederationPeers>,
    ) -> SourceFetchAttempt {
        let Some(prepared) = federation_peers else {
            return SourceFetchAttempt {
                result: Err(Error::NotFound(format!(
                    "Federation source requires prepared peer data for tier {tier}"
                ))),
                peer_metrics: Vec::new(),
            };
        };

        let Some(peers) = prepared.get(tier) else {
            return SourceFetchAttempt {
                result: Err(Error::NotFound(format!(
                    "No prepared federation peers available for tier {tier}"
                ))),
                peer_metrics: Vec::new(),
            };
        };

        let mut peer_metrics = Vec::new();

        for peer in peers {
            if peer.consecutive_failures > 5 {
                debug!(
                    "Skipping federation peer {} due to open circuit ({} consecutive failures)",
                    peer.endpoint, peer.consecutive_failures
                );
                continue;
            }

            let fetcher = match HttpChunkFetcher::new(&peer.endpoint) {
                Ok(fetcher) => fetcher,
                Err(e) => {
                    debug!(
                        "Could not construct HTTP fetcher for federation peer {}: {}",
                        peer.endpoint, e
                    );
                    peer_metrics.push(PeerFetchMetric {
                        peer_id: peer.id.clone(),
                        latency_ms: 0,
                        succeeded: false,
                    });
                    continue;
                }
            };

            let started = Instant::now();
            match fetcher.fetch(hash).await {
                Ok(data) => {
                    let latency_ms = duration_to_i64_ms(started.elapsed());
                    if let Err(e) = self.cache_remote_hit(hash, &data).await {
                        warn!(
                            "Federation peer {} returned chunk {}, but cache write failed: {}",
                            peer.endpoint, hash, e
                        );
                        return SourceFetchAttempt {
                            result: Err(e),
                            peer_metrics,
                        };
                    }

                    peer_metrics.push(PeerFetchMetric {
                        peer_id: peer.id.clone(),
                        latency_ms,
                        succeeded: true,
                    });

                    return SourceFetchAttempt {
                        result: Ok(data),
                        peer_metrics,
                    };
                }
                Err(e) => {
                    let latency_ms = duration_to_i64_ms(started.elapsed());
                    debug!(
                        "Federation peer {} could not provide chunk {}: {}",
                        peer.endpoint, hash, e
                    );
                    peer_metrics.push(PeerFetchMetric {
                        peer_id: peer.id.clone(),
                        latency_ms,
                        succeeded: false,
                    });
                }
            }
        }

        SourceFetchAttempt {
            result: Err(Error::NotFound(format!(
                "No federation peer in tier {tier} could provide chunk {hash}"
            ))),
            peer_metrics,
        }
    }

    async fn cache_remote_hit(&self, hash: &str, data: &[u8]) -> Result<()> {
        let Some(cache_dir) = self.sources.iter().find_map(|source| match source {
            SubstituterSource::LocalCache { cache_dir } => Some(cache_dir),
            _ => None,
        }) else {
            debug!(
                "No local cache source configured; remote hit for {} will not be cached",
                hash
            );
            return Ok(());
        };

        let cache = LocalCacheFetcher::new(cache_dir);
        cache.store(hash, data).await
    }
}

fn duration_to_i64_ms(duration: std::time::Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "substituter/tests.rs"]
mod tests;
