// src/repository/substituter.rs

//! Nix-style substituter chain for ordered package source resolution
//!
//! Sources are tried in order. First source to provide the requested
//! data wins. Builds on the ChunkFetcher pattern from chunk_fetcher.rs
//! but operates at a higher level with package-aware sources.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
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

/// Result of resolving through the chain
#[derive(Debug)]
pub struct SubstituterResult {
    /// Which source provided the data
    pub source_name: String,
    /// Source index in the chain
    pub source_index: usize,
}

/// Returns a human-readable name for a substituter source
pub fn source_name(source: &SubstituterSource) -> &str {
    match source {
        SubstituterSource::LocalCache { .. } => "local-cache",
        SubstituterSource::Federation { .. } => "federation",
        SubstituterSource::Remi { .. } => "remi",
        SubstituterSource::Binary { .. } => "binary",
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

    /// Try each source in order for a single chunk.
    ///
    /// Returns the chunk data and metadata about which source provided it.
    /// For `LocalCache`, performs a filesystem lookup. For other source types,
    /// returns `NotFound` (HTTP integration comes when those modules are connected).
    pub fn resolve_chunk(&self, hash: &str) -> Result<(Vec<u8>, SubstituterResult)> {
        if self.sources.is_empty() {
            return Err(Error::NotFound(format!(
                "No sources in substituter chain for chunk {hash}"
            )));
        }

        for (idx, source) in self.sources.iter().enumerate() {
            let name = source_name(source);
            debug!("Trying source {} ({}) for chunk {}", idx, name, hash);

            match Self::fetch_from_source(source, hash) {
                Ok(data) => {
                    info!(
                        "Source {} ({}) provided chunk {} ({} bytes)",
                        idx, name, hash, data.len()
                    );
                    return Ok((
                        data,
                        SubstituterResult {
                            source_name: name.to_string(),
                            source_index: idx,
                        },
                    ));
                }
                Err(e) => {
                    debug!("Source {} ({}) could not provide chunk {}: {}", idx, name, hash, e);
                }
            }
        }

        Err(Error::NotFound(format!(
            "No source could provide chunk {hash}"
        )))
    }

    /// Batch resolution of multiple chunks.
    ///
    /// Tries `LocalCache` first for all hashes, then falls through to the
    /// next source for any remaining, and so on. Returns a map of
    /// hash -> data for all resolved chunks.
    pub fn resolve_chunks(&self, hashes: &[String]) -> Result<HashMap<String, Vec<u8>>> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }

        info!(
            "Resolving {} chunks through {} sources",
            hashes.len(),
            self.sources.len()
        );

        let mut resolved: HashMap<String, Vec<u8>> = HashMap::new();
        let mut remaining: Vec<&String> = hashes.iter().collect();

        for (idx, source) in self.sources.iter().enumerate() {
            if remaining.is_empty() {
                break;
            }

            let name = source_name(source);
            debug!(
                "Trying source {} ({}) for {} remaining chunks",
                idx, name, remaining.len()
            );

            let mut newly_resolved = Vec::new();

            for hash in &remaining {
                match Self::fetch_from_source(source, hash) {
                    Ok(data) => {
                        debug!(
                            "Source {} provided chunk {} ({} bytes)",
                            name, hash, data.len()
                        );
                        resolved.insert((*hash).clone(), data);
                        newly_resolved.push((*hash).clone());
                    }
                    Err(e) => {
                        debug!("Source {} could not provide chunk {}: {}", name, hash, e);
                    }
                }
            }

            remaining.retain(|h| !newly_resolved.contains(h));
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
        Ok(resolved)
    }

    /// Attempt to fetch a single chunk from a specific source
    fn fetch_from_source(source: &SubstituterSource, hash: &str) -> Result<Vec<u8>> {
        match source {
            SubstituterSource::LocalCache { cache_dir } => {
                Self::fetch_from_local_cache(cache_dir, hash)
            }
            SubstituterSource::Federation { tier } => {
                // TODO: Connect to CAS federation module for real peer fetching
                debug!(
                    "Federation source (tier={}) not available in sync context",
                    tier
                );
                Err(Error::NotFound(format!(
                    "Federation fetch not available in sync context for chunk {hash}"
                )))
            }
            SubstituterSource::Remi { endpoint, distro } => {
                // TODO: Connect to Remi HTTP client for on-demand conversion
                debug!(
                    "Remi source ({}/{}) not available in sync context",
                    endpoint, distro
                );
                Err(Error::NotFound(format!(
                    "Remi fetch not available in sync context for chunk {hash}"
                )))
            }
            SubstituterSource::Binary { base_url } => {
                // TODO: Connect to binary repository download path
                debug!(
                    "Binary source ({}) does not serve individual chunks",
                    base_url
                );
                Err(Error::NotFound(format!(
                    "Binary source does not serve individual chunks for {hash}"
                )))
            }
        }
    }

    /// Fetch a chunk from the local CAS cache directory.
    ///
    /// Uses the same path convention as `LocalCacheFetcher` in chunk_fetcher.rs:
    /// `{cache_dir}/objects/{hash[0:2]}/{hash[2:]}`
    fn fetch_from_local_cache(cache_dir: &Path, hash: &str) -> Result<Vec<u8>> {
        if hash.len() < 2 {
            return Err(Error::NotFound(format!("Invalid chunk hash: {hash}")));
        }

        let (prefix, rest) = hash.split_at(2);
        let chunk_path = cache_dir.join("objects").join(prefix).join(rest);

        if chunk_path.exists() {
            let data = fs::read(&chunk_path).map_err(|e| {
                Error::IoError(format!(
                    "Failed to read cached chunk {}: {}",
                    chunk_path.display(),
                    e
                ))
            })?;
            Ok(data)
        } else {
            Err(Error::NotFound(format!(
                "Chunk {} not in local cache",
                hash
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper to write a chunk into a local cache directory using the CAS layout:
    /// `{cache_dir}/objects/{hash[0:2]}/{hash[2:]}`
    fn write_chunk_to_cache(cache_dir: &Path, hash: &str, data: &[u8]) {
        let (prefix, rest) = hash.split_at(2);
        let dir = cache_dir.join("objects").join(prefix);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(rest), data).unwrap();
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
        assert_eq!(source_name(&chain.sources()[0]), "local-cache");
        assert_eq!(source_name(&chain.sources()[1]), "federation");
        assert_eq!(source_name(&chain.sources()[2]), "remi");
        assert_eq!(source_name(&chain.sources()[3]), "binary");
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

    #[test]
    fn test_local_cache_resolve() {
        let tmp_dir = TempDir::new().unwrap();
        let cache_dir = tmp_dir.path();

        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let data = b"test chunk content";
        write_chunk_to_cache(cache_dir, hash, data);

        let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
            cache_dir: cache_dir.to_path_buf(),
        }]);

        let (resolved_data, result) = chain.resolve_chunk(hash).unwrap();
        assert_eq!(resolved_data, data);
        assert_eq!(result.source_name, "local-cache");
        assert_eq!(result.source_index, 0);
    }

    #[test]
    fn test_local_cache_miss() {
        let tmp_dir = TempDir::new().unwrap();

        let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
            cache_dir: tmp_dir.path().to_path_buf(),
        }]);

        let result = chain.resolve_chunk("deadbeef00112233deadbeef00112233deadbeef00112233deadbeef00112233");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_chunks_batch() {
        let tmp_dir = TempDir::new().unwrap();
        let cache_dir = tmp_dir.path();

        let hash1 = "aa11223344556677aa11223344556677aa11223344556677aa11223344556677";
        let hash2 = "bb99887766554433bb99887766554433bb99887766554433bb99887766554433";

        write_chunk_to_cache(cache_dir, hash1, b"data-one");
        write_chunk_to_cache(cache_dir, hash2, b"data-two");

        let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
            cache_dir: cache_dir.to_path_buf(),
        }]);

        let result = chain
            .resolve_chunks(&[hash1.to_string(), hash2.to_string()])
            .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[hash1], b"data-one");
        assert_eq!(result[hash2], b"data-two");
    }

    #[test]
    fn test_resolve_chunks_partial() {
        let tmp_dir = TempDir::new().unwrap();
        let cache_dir = tmp_dir.path();

        let hash1 = "cc11223344556677cc11223344556677cc11223344556677cc11223344556677";
        let hash2 = "dd99887766554433dd99887766554433dd99887766554433dd99887766554433";

        // Only write hash1
        write_chunk_to_cache(cache_dir, hash1, b"only-this-one");

        let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
            cache_dir: cache_dir.to_path_buf(),
        }]);

        let result = chain
            .resolve_chunks(&[hash1.to_string(), hash2.to_string()])
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[hash1], b"only-this-one");
        assert!(!result.contains_key(hash2));
    }

    #[test]
    fn test_resolve_chunks_falls_through_sources() {
        let empty_cache = TempDir::new().unwrap();
        let populated_cache = TempDir::new().unwrap();

        let hash = "ff00112233445566ff00112233445566ff00112233445566ff00112233445566";
        write_chunk_to_cache(populated_cache.path(), hash, b"found-in-second");

        let chain = SubstituterChain::new(vec![
            SubstituterSource::LocalCache {
                cache_dir: empty_cache.path().to_path_buf(),
            },
            SubstituterSource::LocalCache {
                cache_dir: populated_cache.path().to_path_buf(),
            },
        ]);

        let result = chain.resolve_chunks(&[hash.to_string()]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[hash], b"found-in-second");
    }

    #[test]
    fn test_source_name() {
        assert_eq!(
            source_name(&SubstituterSource::LocalCache {
                cache_dir: PathBuf::from("/tmp")
            }),
            "local-cache"
        );
        assert_eq!(
            source_name(&SubstituterSource::Federation {
                tier: "region_hub".to_string()
            }),
            "federation"
        );
        assert_eq!(
            source_name(&SubstituterSource::Remi {
                endpoint: "https://remi.example.com".to_string(),
                distro: "fedora".to_string(),
            }),
            "remi"
        );
        assert_eq!(
            source_name(&SubstituterSource::Binary {
                base_url: "https://repo.example.com".to_string()
            }),
            "binary"
        );
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

        // Verify each variant round-tripped correctly
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

    #[test]
    fn test_empty_chain() {
        let chain = SubstituterChain::new(Vec::new());
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);

        // resolve_chunk should fail on empty chain
        let result = chain.resolve_chunk("abcdef1234567890");
        assert!(result.is_err());

        // resolve_chunks with empty hashes should succeed
        let result = chain.resolve_chunks(&[]).unwrap();
        assert!(result.is_empty());

        // resolve_chunks with hashes should fail on empty chain
        let result = chain.resolve_chunks(&["abcdef1234567890".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_chunks_empty_request() {
        let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
            cache_dir: PathBuf::from("/nonexistent"),
        }]);
        let result = chain.resolve_chunks(&[]).unwrap();
        assert!(result.is_empty());
    }
}
