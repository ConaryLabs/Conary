// apps/remi/src/server/bounded_cache.rs
//! R2-verified enforcement of the bounded local chunk cache.

use crate::server::{ChunkCache, R2Store};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use conary_core::db::models::ChunkAccess;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Mutex;

#[async_trait]
pub trait DurableCacheAuthority: Send + Sync {
    async fn contains(&self, hash: &str) -> Result<bool>;
}

#[async_trait]
impl DurableCacheAuthority for R2Store {
    async fn contains(&self, hash: &str) -> Result<bool> {
        self.head_chunk(hash).await
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BoundedCacheReport {
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub bytes_freed: u64,
    pub objects_evicted: usize,
}

#[derive(Clone)]
pub struct BoundedCache {
    cache: ChunkCache,
    gate: Arc<Mutex<()>>,
}

impl BoundedCache {
    pub fn new(cache: ChunkCache) -> Self {
        Self {
            cache,
            gate: Arc::new(Mutex::new(())),
        }
    }

    pub async fn enforce<S: DurableCacheAuthority + ?Sized>(
        &self,
        durable: &S,
    ) -> Result<BoundedCacheReport> {
        let _guard = self.gate.lock().await;
        let db_path = self.cache.db_path().to_path_buf();
        let max_bytes = self.cache.max_bytes();
        let (stats, candidates) = tokio::task::spawn_blocking(move || {
            let mut conn = crate::server::open_runtime_db(&db_path)?;
            let tx = conn.transaction()?;
            let stats = ChunkAccess::get_stats(&tx)?;
            let candidates = if stats.total_bytes > max_bytes {
                ChunkAccess::get_lru_chunks_for_bytes(&tx, stats.total_bytes - max_bytes)?
            } else {
                Vec::new()
            };
            tx.commit()?;
            Ok::<_, anyhow::Error>((stats, candidates))
        })
        .await
        .context("join bounded-cache candidate query")??;

        if stats.total_bytes <= max_bytes {
            return Ok(BoundedCacheReport {
                bytes_before: stats.total_bytes,
                bytes_after: stats.total_bytes,
                ..Default::default()
            });
        }

        let bytes_to_free = stats.total_bytes - max_bytes;
        let mut bytes_freed = 0u64;
        let mut evicted = Vec::new();
        for candidate in candidates {
            if bytes_freed >= bytes_to_free {
                break;
            }
            let path = self.cache.chunk_path(&candidate.hash);
            if !path.exists() {
                bytes_freed = bytes_freed.saturating_add(candidate.size_bytes.max(0) as u64);
                evicted.push(candidate.hash);
                continue;
            }
            if !durable.contains(&candidate.hash).await.with_context(|| {
                format!("verify durable chunk {} before eviction", candidate.hash)
            })? {
                bail!(
                    "refusing to evict local chunk {} because R2 does not contain it",
                    candidate.hash
                );
            }
            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("evict local chunk {}", candidate.hash))?;
            bytes_freed = bytes_freed.saturating_add(candidate.size_bytes.max(0) as u64);
            evicted.push(candidate.hash);
        }

        let removed = evicted.len();
        let cleanup_db_path = self.cache.db_path().to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut conn = crate::server::open_runtime_db(&cleanup_db_path)?;
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            {
                let mut delete = tx.prepare("DELETE FROM chunk_access WHERE hash = ?1")?;
                for hash in evicted {
                    delete.execute([hash])?;
                }
            }
            tx.commit()?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("join bounded-cache bookkeeping cleanup")??;

        let bytes_after = stats.total_bytes.saturating_sub(bytes_freed);
        if bytes_after > max_bytes {
            bail!("bounded cache remains above its limit: {bytes_after} > {max_bytes} bytes");
        }
        Ok(BoundedCacheReport {
            bytes_before: stats.total_bytes,
            bytes_after,
            bytes_freed,
            objects_evicted: removed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    struct FakeDurable {
        hashes: HashSet<String>,
    }

    struct UnreachableDurable;

    #[async_trait]
    impl DurableCacheAuthority for FakeDurable {
        async fn contains(&self, hash: &str) -> Result<bool> {
            Ok(self.hashes.contains(hash))
        }
    }

    #[async_trait]
    impl DurableCacheAuthority for UnreachableDurable {
        async fn contains(&self, _hash: &str) -> Result<bool> {
            bail!("simulated R2 outage")
        }
    }

    fn test_cache(max_bytes: u64) -> (tempfile::TempDir, ChunkCache) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("remi.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        drop(conn);
        let cache = ChunkCache::new(temp.path().join("chunks"), max_bytes, db_path);
        (temp, cache)
    }

    #[tokio::test]
    async fn enforcement_evicts_only_after_r2_presence_and_reaches_exact_bound() {
        let (_temp, cache) = test_cache(4);
        let first = conary_core::hash::sha256(b"aaaa");
        let second = conary_core::hash::sha256(b"bbbb");
        cache.store_chunk(&first, b"aaaa").await.unwrap();
        cache.store_chunk(&second, b"bbbb").await.unwrap();
        let durable = FakeDurable {
            hashes: HashSet::from([first.clone(), second.clone()]),
        };

        let report = BoundedCache::new(cache.clone())
            .enforce(&durable)
            .await
            .unwrap();

        assert_eq!(report.bytes_before, 8);
        assert_eq!(report.bytes_after, 4);
        assert_eq!(report.bytes_freed, 4);
        assert_eq!(report.objects_evicted, 1);
        assert_eq!(
            usize::from(cache.has_chunk(&first).await)
                + usize::from(cache.has_chunk(&second).await),
            1
        );
    }

    #[tokio::test]
    async fn enforcement_refuses_to_trade_the_only_durable_copy_for_a_bound() {
        let (_temp, cache) = test_cache(0);
        let hash = conary_core::hash::sha256(b"only-local");
        cache.store_chunk(&hash, b"only-local").await.unwrap();

        let error = BoundedCache::new(cache.clone())
            .enforce(&FakeDurable {
                hashes: HashSet::new(),
            })
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("R2 does not contain"), "{error}");
        assert!(cache.has_chunk(&hash).await);
    }

    #[tokio::test]
    async fn enforcement_preserves_local_bytes_when_r2_cannot_be_checked() {
        let (_temp, cache) = test_cache(0);
        let hash = conary_core::hash::sha256(b"outage");
        cache.store_chunk(&hash, b"outage").await.unwrap();

        let error = BoundedCache::new(cache.clone())
            .enforce(&UnreachableDurable)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("verify durable chunk"), "{error}");
        assert!(cache.has_chunk(&hash).await);
    }

    #[tokio::test]
    async fn enforcement_reports_when_protection_prevents_the_exact_bound() {
        let (_temp, cache) = test_cache(0);
        let hash = conary_core::hash::sha256(b"protected");
        cache.store_chunk(&hash, b"protected").await.unwrap();
        cache.protect_chunks(std::slice::from_ref(&hash)).await;

        let error = BoundedCache::new(cache.clone())
            .enforce(&FakeDurable {
                hashes: HashSet::from([hash.clone()]),
            })
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("remains above its limit"), "{error}");
        assert!(cache.has_chunk(&hash).await);
    }
}
