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
    /// Binary package repository
    Binary { base_url: String },
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
            Self::Binary { .. } => "binary",
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
            SubstituterSource::Binary { base_url } => SourceFetchAttempt {
                result: Err(Error::NotFound(format!(
                    "Binary source {} does not serve individual chunks for {hash}",
                    base_url
                ))),
                peer_metrics: Vec::new(),
            },
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
mod tests {
    use super::*;
    use crate::db::schema;
    use crate::hash::sha256;
    use rusqlite::Connection;
    use std::fs;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Helper to write a chunk into a local cache directory using the CAS layout:
    /// `{cache_dir}/objects/{hash[0:2]}/{hash[2:]}`
    fn write_chunk_to_cache(cache_dir: &Path, hash: &str, data: &[u8]) {
        let (prefix, rest) = hash.split_at(2);
        let dir = cache_dir.join("objects").join(prefix);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(rest), data).unwrap();
    }

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    async fn spawn_chunk_server(
        routes: HashMap<String, Vec<u8>>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen_paths = Arc::new(Mutex::new(Vec::new()));
        let seen_paths_task = Arc::clone(&seen_paths);
        let routes = Arc::new(routes);

        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0_u8; 4096];
                let bytes_read = match stream.read(&mut buf).await {
                    Ok(bytes_read) => bytes_read,
                    Err(_) => continue,
                };
                if bytes_read == 0 {
                    continue;
                }

                let request = String::from_utf8_lossy(&buf[..bytes_read]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                seen_paths_task.lock().unwrap().push(path.clone());

                let response = if let Some(hash) = path.strip_prefix("/v1/chunks/") {
                    if let Some(body) = routes.get(hash) {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes()
                        .into_iter()
                        .chain(body.iter().copied())
                        .collect::<Vec<_>>()
                    } else {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                } else {
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec()
                };

                let _ = stream.write_all(&response).await;
            }
        });

        (format!("http://{addr}"), seen_paths)
    }

    #[test]
    fn test_chain_ordering() {
        let chain = SubstituterChain::new(vec![
            SubstituterSource::LocalCache {
                cache_dir: PathBuf::from("/tmp/cache"),
            },
            SubstituterSource::Federation {
                tier: "cell_hub".to_string(),
            },
            SubstituterSource::Remi {
                endpoint: "https://remi.example.com".to_string(),
                distro: "fedora-41".to_string(),
            },
            SubstituterSource::Binary {
                base_url: "https://repo.example.com".to_string(),
            },
        ]);

        assert_eq!(chain.len(), 4);
        assert_eq!(chain.sources()[0].name(), "local-cache");
        assert_eq!(chain.sources()[1].name(), "federation");
        assert_eq!(chain.sources()[2].name(), "remi");
        assert_eq!(chain.sources()[3].name(), "binary");
    }

    #[test]
    fn test_add_source() {
        let mut chain = SubstituterChain::new(Vec::new());
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);

        chain.add_source(SubstituterSource::Binary {
            base_url: "https://repo.example.com".to_string(),
        });
        assert!(!chain.is_empty());
        assert_eq!(chain.len(), 1);
        assert_eq!(chain.sources().len(), 1);
    }

    #[tokio::test]
    async fn test_local_cache_resolve_async() {
        let tmp_dir = TempDir::new().unwrap();
        let cache_dir = tmp_dir.path();

        let hash = sha256(b"test chunk content");
        let data = b"test chunk content";
        write_chunk_to_cache(cache_dir, &hash, data);

        let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
            cache_dir: cache_dir.to_path_buf(),
        }]);

        let (resolved_data, result) = chain.resolve_chunk(&hash, None).await.unwrap();
        assert_eq!(resolved_data, data);
        assert_eq!(result.source_name, "local-cache");
        assert_eq!(result.source_index, 0);
        assert!(result.peer_metrics.is_empty());
    }

    #[tokio::test]
    async fn test_local_cache_miss() {
        let tmp_dir = TempDir::new().unwrap();

        let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
            cache_dir: tmp_dir.path().to_path_buf(),
        }]);

        let result = chain
            .resolve_chunk(
                "deadbeef00112233deadbeef00112233deadbeef00112233deadbeef00112233",
                None,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resolve_chunks_batch_async() {
        let tmp_dir = TempDir::new().unwrap();
        let cache_dir = tmp_dir.path();

        let hash1 = sha256(b"data-one");
        let hash2 = sha256(b"data-two");

        write_chunk_to_cache(cache_dir, &hash1, b"data-one");
        write_chunk_to_cache(cache_dir, &hash2, b"data-two");

        let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
            cache_dir: cache_dir.to_path_buf(),
        }]);

        let result = chain
            .resolve_chunks(&[hash1.clone(), hash2.clone()], None)
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result.chunks[&hash1], b"data-one");
        assert_eq!(result.chunks[&hash2], b"data-two");
        assert!(result.peer_metrics.is_empty());
    }

    #[tokio::test]
    async fn test_resolve_chunks_partial() {
        let tmp_dir = TempDir::new().unwrap();
        let cache_dir = tmp_dir.path();

        let hash1 = sha256(b"only-this-one");
        let hash2 = sha256(b"missing-one");

        write_chunk_to_cache(cache_dir, &hash1, b"only-this-one");

        let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
            cache_dir: cache_dir.to_path_buf(),
        }]);

        let result = chain
            .resolve_chunks(&[hash1.clone(), hash2.clone()], None)
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result.chunks[&hash1], b"only-this-one");
        assert!(!result.chunks.contains_key(&hash2));
    }

    #[tokio::test]
    async fn test_resolve_chunks_falls_through_sources() {
        let empty_cache = TempDir::new().unwrap();
        let populated_cache = TempDir::new().unwrap();

        let hash = sha256(b"found-in-second");
        write_chunk_to_cache(populated_cache.path(), &hash, b"found-in-second");

        let chain = SubstituterChain::new(vec![
            SubstituterSource::LocalCache {
                cache_dir: empty_cache.path().to_path_buf(),
            },
            SubstituterSource::LocalCache {
                cache_dir: populated_cache.path().to_path_buf(),
            },
        ]);

        let result = chain
            .resolve_chunks(std::slice::from_ref(&hash), None)
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result.chunks[&hash], b"found-in-second");
    }

    #[test]
    fn test_source_name() {
        assert_eq!(
            SubstituterSource::LocalCache {
                cache_dir: PathBuf::from("/tmp")
            }
            .name(),
            "local-cache"
        );
        assert_eq!(
            SubstituterSource::Federation {
                tier: "region_hub".to_string()
            }
            .name(),
            "federation"
        );
        assert_eq!(
            SubstituterSource::Remi {
                endpoint: "https://remi.example.com".to_string(),
                distro: "fedora".to_string(),
            }
            .name(),
            "remi"
        );
        assert_eq!(
            SubstituterSource::Binary {
                base_url: "https://repo.example.com".to_string()
            }
            .name(),
            "binary"
        );
    }

    #[test]
    fn test_legacy_binary_source_still_deserializes() {
        let json = r#"[{"type":"binary","base_url":"https://repo.example.com"}]"#;
        let deserialized: Vec<SubstituterSource> = serde_json::from_str(json).unwrap();

        assert_eq!(deserialized.len(), 1);
        assert!(matches!(
            &deserialized[0],
            SubstituterSource::Binary { base_url } if base_url == "https://repo.example.com"
        ));
    }

    #[test]
    fn test_serde_roundtrip() {
        let sources = vec![
            SubstituterSource::LocalCache {
                cache_dir: PathBuf::from("/var/cache/conary"),
            },
            SubstituterSource::Federation {
                tier: "cell_hub".to_string(),
            },
            SubstituterSource::Remi {
                endpoint: "https://remi.example.com".to_string(),
                distro: "fedora-41".to_string(),
            },
            SubstituterSource::Binary {
                base_url: "https://repo.example.com".to_string(),
            },
        ];

        let json = serde_json::to_string(&sources).unwrap();
        let deserialized: Vec<SubstituterSource> = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.len(), 4);
        assert!(matches!(
            &deserialized[0],
            SubstituterSource::LocalCache { cache_dir } if cache_dir == Path::new("/var/cache/conary")
        ));
        assert!(matches!(
            &deserialized[1],
            SubstituterSource::Federation { tier } if tier == "cell_hub"
        ));
        assert!(matches!(
            &deserialized[2],
            SubstituterSource::Remi { endpoint, distro }
                if endpoint == "https://remi.example.com" && distro == "fedora-41"
        ));
        assert!(matches!(
            &deserialized[3],
            SubstituterSource::Binary { base_url } if base_url == "https://repo.example.com"
        ));
    }

    #[tokio::test]
    async fn test_binary_source_is_ignored_for_chunk_resolution() {
        let tmp_dir = TempDir::new().unwrap();
        let hash = sha256(b"local-data");
        write_chunk_to_cache(tmp_dir.path(), &hash, b"local-data");

        let chain = SubstituterChain::new(vec![
            SubstituterSource::Binary {
                base_url: "https://repo.example.com".to_string(),
            },
            SubstituterSource::LocalCache {
                cache_dir: tmp_dir.path().to_path_buf(),
            },
        ]);

        let (data, result) = chain.resolve_chunk(&hash, None).await.unwrap();
        assert_eq!(data, b"local-data");
        assert_eq!(result.source_name, "local-cache");
        assert!(result.peer_metrics.is_empty());
    }

    #[tokio::test]
    async fn test_empty_chain_async() {
        let chain = SubstituterChain::new(Vec::new());
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);

        let result = chain.resolve_chunk("abcdef1234567890", None).await;
        assert!(result.is_err());

        let result = chain.resolve_chunks(&[], None).await.unwrap();
        assert!(result.is_empty());

        let result = chain
            .resolve_chunks(&["abcdef1234567890".to_string()], None)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resolve_chunks_empty_request() {
        let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
            cache_dir: PathBuf::from("/nonexistent"),
        }]);
        let result = chain.resolve_chunks(&[], None).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_remi_source_fetches_chunk_and_populates_cache() {
        let tmp_dir = TempDir::new().unwrap();
        let data = b"remi-data".to_vec();
        let hash = sha256(&data);
        let (endpoint, seen_paths) =
            spawn_chunk_server(HashMap::from([(hash.clone(), data.clone())])).await;

        let chain = SubstituterChain::new(vec![
            SubstituterSource::LocalCache {
                cache_dir: tmp_dir.path().to_path_buf(),
            },
            SubstituterSource::Remi {
                endpoint,
                distro: "fedora-42".to_string(),
            },
        ]);

        let (resolved, result) = chain.resolve_chunk(&hash, None).await.unwrap();
        assert_eq!(resolved, data);
        assert_eq!(result.source_name, "remi");

        let cached = chain.resolve_chunk(&hash, None).await.unwrap();
        assert_eq!(cached.1.source_name, "local-cache");
        assert_eq!(seen_paths.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_resolve_chunk_uses_cached_copy_after_remi_hit() {
        let tmp_dir = TempDir::new().unwrap();
        let data = b"cached-after-remi".to_vec();
        let hash = sha256(&data);
        let (endpoint, seen_paths) =
            spawn_chunk_server(HashMap::from([(hash.clone(), data.clone())])).await;

        let chain = SubstituterChain::new(vec![
            SubstituterSource::LocalCache {
                cache_dir: tmp_dir.path().to_path_buf(),
            },
            SubstituterSource::Remi {
                endpoint,
                distro: "fedora-42".to_string(),
            },
        ]);

        let first = chain.resolve_chunk(&hash, None).await.unwrap();
        assert_eq!(first.1.source_name, "remi");
        assert_eq!(seen_paths.lock().unwrap().len(), 1);

        let second = chain.resolve_chunk(&hash, None).await.unwrap();
        assert_eq!(second.1.source_name, "local-cache");
        assert_eq!(seen_paths.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_federation_source_skips_disabled_and_open_circuit_peers() {
        let conn = test_conn();
        let data = b"federation-data".to_vec();
        let hash = sha256(&data);
        let (healthy_endpoint, seen_paths) =
            spawn_chunk_server(HashMap::from([(hash.clone(), data.clone())])).await;

        federation_peer::insert(
            &conn,
            "peer-disabled",
            "http://127.0.0.1:1",
            Some("Disabled"),
            "leaf",
        )
        .unwrap();
        federation_peer::insert(
            &conn,
            "peer-open-circuit",
            "http://127.0.0.1:2",
            Some("Open Circuit"),
            "leaf",
        )
        .unwrap();
        federation_peer::insert(
            &conn,
            "peer-healthy",
            &healthy_endpoint,
            Some("Healthy"),
            "leaf",
        )
        .unwrap();
        conn.execute(
            "UPDATE federation_peers SET is_enabled = 0 WHERE id = 'peer-disabled'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE federation_peers SET consecutive_failures = 6 WHERE id = 'peer-open-circuit'",
            [],
        )
        .unwrap();

        let chain = SubstituterChain::new(vec![SubstituterSource::Federation {
            tier: "leaf".to_string(),
        }]);
        let prepared = chain.prepare_federation_peers(&conn).unwrap();

        let (resolved, result) = chain.resolve_chunk(&hash, Some(&prepared)).await.unwrap();
        assert_eq!(resolved, data);
        assert_eq!(result.source_name, "federation");
        assert_eq!(result.peer_metrics.len(), 1);
        assert_eq!(result.peer_metrics[0].peer_id, "peer-healthy");
        assert!(result.peer_metrics[0].succeeded);
        assert_eq!(seen_paths.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_federation_source_falls_through_after_failed_peer() {
        let conn = test_conn();
        let data = b"healthy-peer-data".to_vec();
        let hash = sha256(&data);
        let (healthy_endpoint, _healthy_seen) =
            spawn_chunk_server(HashMap::from([(hash.clone(), data.clone())])).await;
        let (failing_endpoint, _failing_seen) = spawn_chunk_server(HashMap::new()).await;

        federation_peer::insert(
            &conn,
            "peer-fail",
            &failing_endpoint,
            Some("Failing"),
            "leaf",
        )
        .unwrap();
        federation_peer::insert(&conn, "peer-ok", &healthy_endpoint, Some("Healthy"), "leaf")
            .unwrap();
        conn.execute(
            "UPDATE federation_peers SET latency_ms = 5 WHERE id = 'peer-fail'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE federation_peers SET latency_ms = 50 WHERE id = 'peer-ok'",
            [],
        )
        .unwrap();

        let chain = SubstituterChain::new(vec![SubstituterSource::Federation {
            tier: "leaf".to_string(),
        }]);
        let prepared = chain.prepare_federation_peers(&conn).unwrap();

        let (resolved, result) = chain.resolve_chunk(&hash, Some(&prepared)).await.unwrap();
        assert_eq!(resolved, data);
        assert_eq!(result.peer_metrics.len(), 2);
        assert_eq!(result.peer_metrics[0].peer_id, "peer-fail");
        assert!(!result.peer_metrics[0].succeeded);
        assert_eq!(result.peer_metrics[1].peer_id, "peer-ok");
        assert!(result.peer_metrics[1].succeeded);

        SubstituterChain::apply_peer_metrics(&conn, &result.peer_metrics).unwrap();
        let failed = federation_peer::find_by_id(&conn, "peer-fail")
            .unwrap()
            .unwrap();
        let healthy = federation_peer::find_by_id(&conn, "peer-ok")
            .unwrap()
            .unwrap();
        assert_eq!(failed.failure_count, 1);
        assert_eq!(healthy.success_count, 1);
    }

    #[tokio::test]
    async fn test_federation_source_records_success_metrics_and_caches_chunk() {
        let conn = test_conn();
        let tmp_dir = TempDir::new().unwrap();
        let data = b"cached-federation-hit".to_vec();
        let hash = sha256(&data);
        let (healthy_endpoint, seen_paths) =
            spawn_chunk_server(HashMap::from([(hash.clone(), data.clone())])).await;

        federation_peer::insert(
            &conn,
            "peer-cache",
            &healthy_endpoint,
            Some("Cache Peer"),
            "leaf",
        )
        .unwrap();

        let chain = SubstituterChain::new(vec![
            SubstituterSource::LocalCache {
                cache_dir: tmp_dir.path().to_path_buf(),
            },
            SubstituterSource::Federation {
                tier: "leaf".to_string(),
            },
        ]);
        let prepared = chain.prepare_federation_peers(&conn).unwrap();

        let first = chain.resolve_chunk(&hash, Some(&prepared)).await.unwrap();
        assert_eq!(first.1.source_name, "federation");
        SubstituterChain::apply_peer_metrics(&conn, &first.1.peer_metrics).unwrap();

        let peer = federation_peer::find_by_id(&conn, "peer-cache")
            .unwrap()
            .unwrap();
        assert_eq!(peer.success_count, 1);

        let second = chain.resolve_chunk(&hash, None).await.unwrap();
        assert_eq!(second.1.source_name, "local-cache");
        assert_eq!(seen_paths.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_federation_source_without_prepared_peers_is_skipped() {
        let hash = sha256(b"never-fetched");
        let chain = SubstituterChain::new(vec![SubstituterSource::Federation {
            tier: "leaf".to_string(),
        }]);

        let err = chain.resolve_chunk(&hash, None).await.unwrap_err();
        assert!(format!("{err}").contains("prepared peer data"));
    }
}
