// apps/remi/src/server/conversion_crawl/operator.rs

//! Stopped-runtime adapter for a complete production conversion crawl.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};

use super::{ConversionCrawlConfig, RemiConversionCrawlV4, run_conversion_crawl};
use crate::server::catalog_authority::ProfileRevisionSelection;
use crate::server::config::RemiConfig;
use crate::server::r2::{R2Config, R2Store};
use crate::server::{BoundedCache, ChunkCache, acquire_existing_runtime_storage};

const R2_REACHABILITY_PROBE: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// Run one exact candidate crawl under the deployed storage and durability policy.
///
/// The runtime-root lock excludes the serving process and its refresh scheduler
/// for the complete operation. When R2 is configured, every newly produced CCS
/// transport reaches that durable authority before conversion state commits.
pub async fn run_conversion_crawl_from_config(
    remi_config: &RemiConfig,
    candidates: Vec<ProfileRevisionSelection>,
    output_path: PathBuf,
    concurrency: usize,
) -> Result<RemiConversionCrawlV4> {
    remi_config.validate()?;
    let server_config = remi_config.to_server_config()?;
    let _runtime_lock = acquire_existing_runtime_storage(remi_config, &server_config)?;
    let repository_keys_dir = server_config
        .release_publish
        .repository_keys_dir
        .clone()
        .context("release_publish.repository_keys_dir is required for conversion crawl")?;

    let r2_store = configured_r2_store(remi_config).await?;
    let bounded_cache = BoundedCache::new(ChunkCache::new(
        server_config.chunk_dir.clone(),
        server_config.cache_max_bytes,
        server_config.db_path.clone(),
    ));
    let config = ConversionCrawlConfig {
        db_path: server_config.db_path,
        catalog_dir: server_config.catalog_dir,
        chunk_dir: server_config.chunk_dir,
        cache_dir: server_config.cache_dir,
        repository_keys_dir,
        output_path,
        concurrency,
        candidates,
    };
    run_conversion_crawl(&config, r2_store, bounded_cache).await
}

async fn configured_r2_store(remi_config: &RemiConfig) -> Result<Option<Arc<R2Store>>> {
    if !remi_config.r2.enabled {
        return Ok(None);
    }
    let endpoint = remi_config
        .r2
        .endpoint
        .as_ref()
        .context("r2.endpoint is required when R2 authority is enabled")?;
    let store = Arc::new(
        R2Store::new(&R2Config {
            endpoint: endpoint.clone(),
            bucket: remi_config.r2.bucket.clone(),
            prefix: remi_config.r2.prefix.clone(),
            region: "auto".to_string(),
        })
        .context("initialize mandatory conversion-crawl R2 authority")?,
    );
    store
        .head_chunk(R2_REACHABILITY_PROBE)
        .await
        .context("probe mandatory conversion-crawl R2 authority")?;
    Ok(Some(store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::runtime_lock::RuntimeRootLock;

    #[tokio::test]
    async fn live_runtime_is_rejected_before_crawl_output() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = RemiConfig::default();
        config.storage.root = temp.path().to_path_buf();
        config.release_publish.repository_keys_dir = Some(temp.path().join("repository-keys"));
        let server = config.to_server_config().unwrap();
        std::fs::create_dir_all(server.db_path.parent().unwrap()).unwrap();
        conary_core::db::init(&server.db_path).unwrap();
        let _owner = RuntimeRootLock::acquire(config.storage_root()).unwrap();
        let output = temp.path().join("crawl.json");

        let error = run_conversion_crawl_from_config(&config, Vec::new(), output.clone(), 1)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("already owned"), "{error:#}");
        assert!(!output.exists());
    }
}
