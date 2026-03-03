// src/server/lite.rs
//! Remi Lite - Zero-config LAN proxy for Conary
//!
//! A single-command proxy for CI, air-gapped networks, and fleet deployment.
//! Provides pull-through caching of chunks and index data with mDNS
//! auto-discovery and advertisement.
//!
//! # Usage
//!
//! ```text
//! conary remi-proxy                                    # Start with auto-discovery
//! conary remi-proxy --upstream https://remi.example.com
//! conary remi-proxy --offline --cache-dir /mnt/usb     # Air-gapped mode
//! ```
//!
//! # Architecture
//!
//! ```text
//! ┌──────────┐       ┌──────────────┐       ┌──────────────┐
//! │  Client   │──────>│  Remi Lite   │──────>│  Upstream    │
//! │  (LAN)   │<──────│  (this node) │<──────│  Remi Server │
//! └──────────┘       └──────────────┘       └──────────────┘
//!                    pull-through cache
//! ```

use crate::federation::PeerTier;
use crate::federation::mdns::MdnsDiscovery;
use crate::repository::chunk_fetcher::{
    ChunkFetcher, CompositeChunkFetcher, HttpChunkFetcher, LocalCacheFetcher,
};
use anyhow::Result;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Default port for Remi Lite proxy
const DEFAULT_PORT: u16 = 7891;

/// Default cache directory
const DEFAULT_CACHE_DIR: &str = "/var/cache/conary/proxy";

/// TTL for cached index responses (seconds)
const INDEX_CACHE_TTL_SECS: u64 = 60;

/// Configuration for the Remi Lite proxy
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Port to listen on (default 7891)
    pub port: u16,
    /// Explicit upstream Remi URL, or discovered via mDNS
    pub upstream_url: Option<String>,
    /// Local cache directory (default /var/cache/conary/proxy)
    pub cache_dir: PathBuf,
    /// Enable mDNS auto-discovery (default true)
    pub mdns_enabled: bool,
    /// mDNS scan duration in seconds (default 3)
    pub mdns_scan_secs: u64,
    /// Serve only from cache, no upstream (default false)
    pub offline: bool,
    /// Announce this proxy via mDNS (default true)
    pub advertise: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            upstream_url: None,
            cache_dir: PathBuf::from(DEFAULT_CACHE_DIR),
            mdns_enabled: true,
            mdns_scan_secs: 3,
            offline: false,
            advertise: true,
        }
    }
}

/// Shared state for the Remi Lite proxy
pub struct ProxyState {
    /// Proxy configuration
    pub config: ProxyConfig,
    /// Resolved upstream URL (from config or mDNS discovery)
    pub upstream_url: Option<String>,
    /// Chunk fetcher chain (local cache + optional HTTP upstream)
    pub chunk_fetcher: Arc<dyn ChunkFetcher>,
    /// HTTP client for proxying index requests
    pub http_client: reqwest::Client,
    /// File-based index cache for proxied responses
    pub index_cache_dir: PathBuf,
}

/// A cached index response stored on disk
#[derive(Debug)]
struct CachedResponse {
    /// The response body
    data: Vec<u8>,
    /// When the cache entry was written
    timestamp: SystemTime,
}

/// Start the Remi Lite proxy
///
/// This is the main entry point. It:
/// 1. Discovers upstream via mDNS (if needed)
/// 2. Builds the chunk fetcher chain
/// 3. Optionally advertises via mDNS
/// 4. Starts the HTTP server
pub async fn run_proxy(config: ProxyConfig) -> Result<()> {
    info!(
        "[remi-lite] Starting Remi Lite proxy on port {}",
        config.port
    );
    info!("[remi-lite] Cache directory: {}", config.cache_dir.display());
    info!(
        "[remi-lite] Mode: {}",
        if config.offline { "offline" } else { "online" }
    );

    // Ensure cache directories exist
    let index_cache_dir = config.cache_dir.join("index");
    tokio::fs::create_dir_all(&index_cache_dir).await?;
    tokio::fs::create_dir_all(config.cache_dir.join("objects")).await?;

    // Step 1: Resolve upstream URL
    let upstream_url = resolve_upstream(&config).await?;

    if let Some(ref url) = upstream_url {
        info!("[remi-lite] Upstream: {}", url);
    } else if config.offline {
        info!("[remi-lite] Running in offline mode (cache only)");
    } else {
        warn!("[remi-lite] No upstream discovered or configured");
    }

    // Step 2: Build chunk fetcher
    let chunk_fetcher = build_chunk_fetcher(&config, upstream_url.as_deref())?;

    // Step 3: Build HTTP client for index proxying
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("conary-remi-lite/0.1")
        .build()?;

    // Step 4: Build state
    let state = Arc::new(RwLock::new(ProxyState {
        config: config.clone(),
        upstream_url: upstream_url.clone(),
        chunk_fetcher,
        http_client,
        index_cache_dir: index_cache_dir.clone(),
    }));

    // Step 5: Advertise via mDNS if enabled
    let mdns_handle = if config.advertise && config.mdns_enabled && !config.offline {
        match advertise_mdns(config.port) {
            Ok(mdns) => {
                info!("[remi-lite] Advertising via mDNS on port {}", config.port);
                Some(mdns)
            }
            Err(e) => {
                warn!("[remi-lite] Failed to advertise via mDNS: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Step 6: Create router and serve
    let app = create_proxy_router(state);

    let bind_addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!("[remi-lite] Listening on {}", bind_addr);
    info!("[remi-lite] Ready to serve");

    // Serve with graceful shutdown on ctrl-c
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Step 7: Cleanup mDNS on shutdown
    if let Some(mdns) = mdns_handle {
        info!("[remi-lite] Shutting down mDNS...");
        if let Err(e) = mdns.shutdown() {
            warn!("[remi-lite] Error during mDNS shutdown: {}", e);
        }
    }

    info!("[remi-lite] Stopped");
    Ok(())
}

/// Resolve the upstream URL from config or mDNS discovery
async fn resolve_upstream(config: &ProxyConfig) -> Result<Option<String>> {
    // Offline mode: no upstream
    if config.offline {
        return Ok(None);
    }

    // Explicit upstream from config
    if let Some(ref url) = config.upstream_url {
        return Ok(Some(url.trim_end_matches('/').to_string()));
    }

    // Try mDNS discovery
    if config.mdns_enabled {
        info!(
            "[remi-lite] Scanning for upstream Remi instances via mDNS ({} seconds)...",
            config.mdns_scan_secs
        );

        let scan_duration = Duration::from_secs(config.mdns_scan_secs);

        // mDNS uses std::thread internally, so run via spawn_blocking
        let peers = tokio::task::spawn_blocking(move || {
            let discovery = MdnsDiscovery::new()?;
            discovery.scan(scan_duration)
        })
        .await??;

        if peers.is_empty() {
            info!("[remi-lite] No upstream Remi instances found via mDNS");
            return Ok(None);
        }

        // Prefer hubs over leaves
        let best_peer = peers
            .iter()
            .find(|p| p.tier == PeerTier::RegionHub)
            .or_else(|| peers.iter().find(|p| p.tier == PeerTier::CellHub))
            .or_else(|| peers.first());

        if let Some(peer) = best_peer {
            let federated_peer = peer.to_peer()?;
            let url = federated_peer.endpoint.trim_end_matches('/').to_string();
            info!(
                "[remi-lite] Discovered upstream: {} (tier: {}, hostname: {})",
                url, peer.tier, peer.hostname
            );
            return Ok(Some(url));
        }
    }

    Ok(None)
}

/// Build the chunk fetcher chain based on config
fn build_chunk_fetcher(
    config: &ProxyConfig,
    upstream_url: Option<&str>,
) -> Result<Arc<dyn ChunkFetcher>> {
    let cache = LocalCacheFetcher::new(&config.cache_dir);

    if config.offline || upstream_url.is_none() {
        // Offline: only local cache
        info!("[remi-lite] Chunk source: local cache only");
        return Ok(Arc::new(cache));
    }

    // Online: local cache + HTTP upstream
    let upstream = upstream_url.expect("upstream_url checked above");
    let http_fetcher = HttpChunkFetcher::new(upstream)?;

    info!(
        "[remi-lite] Chunk source: local cache -> HTTP ({})",
        upstream
    );

    let composite =
        CompositeChunkFetcher::with_cache(vec![Arc::new(cache), Arc::new(http_fetcher)], &config.cache_dir);

    Ok(Arc::new(composite))
}

/// Register this proxy via mDNS for LAN discovery
fn advertise_mdns(port: u16) -> Result<MdnsDiscovery> {
    let mut mdns = MdnsDiscovery::new()?;

    let hostname = gethostname_safe();
    let instance_name = format!("remi-lite-{}", &hostname);
    let node_id = crate::hash::sha256(instance_name.as_bytes());

    mdns.register(&instance_name, &node_id, port, PeerTier::Leaf, Some(&hostname))?;

    Ok(mdns)
}

/// Get hostname safely, falling back to "unknown"
fn gethostname_safe() -> String {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        let mut buf = [0u8; 256];
        unsafe {
            if libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) == 0
                && let Ok(cstr) =
                    CStr::from_ptr(buf.as_ptr() as *const libc::c_char).to_str()
            {
                return cstr.split('.').next().unwrap_or(cstr).to_string();
            }
        }
    }
    "unknown".to_string()
}

/// Create the axum router for the proxy
fn create_proxy_router(state: Arc<RwLock<ProxyState>>) -> Router {
    Router::new()
        // Health check
        .route("/health", get(health))
        // Chunk serving (pull-through cache)
        .route("/v1/chunks/:hash", get(proxy_chunk))
        // Sparse index proxy
        .route("/v1/index/:distro/:name", get(proxy_index_entry))
        .route("/v1/index/:distro", get(proxy_index_list))
        // Package list proxy
        .route("/v1/:distro/packages/", get(proxy_package_list))
        .route("/v1/:distro/packages/:name", get(proxy_package_detail))
        .with_state(state)
}

// =============================================================================
// Handler: Health Check
// =============================================================================

/// GET /health
///
/// Simple liveness check for the proxy.
async fn health(State(state): State<Arc<RwLock<ProxyState>>>) -> Response {
    let state_guard = state.read().await;
    let mode = if state_guard.config.offline {
        "offline"
    } else if state_guard.upstream_url.is_some() {
        "online"
    } else {
        "degraded"
    };

    let body = format!(
        "OK\nmode: {}\nupstream: {}\ncache_dir: {}",
        mode,
        state_guard
            .upstream_url
            .as_deref()
            .unwrap_or("none"),
        state_guard.config.cache_dir.display()
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// =============================================================================
// Handler: Chunk Proxy (Pull-Through Cache)
// =============================================================================

/// Validate chunk hash format (64 hex chars for SHA-256)
fn is_valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
}

/// GET /v1/chunks/:hash
///
/// Fetch a chunk via the pull-through cache. Tries local cache first,
/// then upstream, caching on hit.
async fn proxy_chunk(
    State(state): State<Arc<RwLock<ProxyState>>>,
    Path(hash): Path<String>,
) -> Response {
    if !is_valid_hash(&hash) {
        return (StatusCode::BAD_REQUEST, "Invalid chunk hash format").into_response();
    }

    let state_guard = state.read().await;
    let fetcher = Arc::clone(&state_guard.chunk_fetcher);
    drop(state_guard);

    match fetcher.fetch(&hash).await {
        Ok(data) => {
            debug!("[remi-lite] Serving chunk: {} ({} bytes)", hash, data.len());
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::CONTENT_LENGTH, data.len())
                .header(
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable",
                )
                .header(header::ETAG, format!("\"{}\"", hash))
                .body(Body::from(data))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => {
            debug!("[remi-lite] Chunk not found: {} ({})", hash, e);
            (StatusCode::NOT_FOUND, "Chunk not found").into_response()
        }
    }
}

// =============================================================================
// Handler: Index Proxy (Pass-Through with File Cache)
// =============================================================================

/// GET /v1/index/:distro/:name
///
/// Proxy sparse index entry to upstream with file-based caching.
async fn proxy_index_entry(
    State(state): State<Arc<RwLock<ProxyState>>>,
    Path((distro, name)): Path<(String, String)>,
) -> Response {
    let path = format!("v1/index/{}/{}", distro, name);
    proxy_pass_through(state, &path).await
}

/// GET /v1/index/:distro
///
/// Proxy index package list to upstream with file-based caching.
async fn proxy_index_list(
    State(state): State<Arc<RwLock<ProxyState>>>,
    Path(distro): Path<String>,
) -> Response {
    let path = format!("v1/index/{}", distro);
    proxy_pass_through(state, &path).await
}

/// GET /v1/:distro/packages/
///
/// Proxy package list to upstream with file-based caching.
async fn proxy_package_list(
    State(state): State<Arc<RwLock<ProxyState>>>,
    Path(distro): Path<String>,
) -> Response {
    let path = format!("v1/{}/packages/", distro);
    proxy_pass_through(state, &path).await
}

/// GET /v1/:distro/packages/:name
///
/// Proxy package detail to upstream with file-based caching.
async fn proxy_package_detail(
    State(state): State<Arc<RwLock<ProxyState>>>,
    Path((distro, name)): Path<(String, String)>,
) -> Response {
    let path = format!("v1/{}/packages/{}", distro, name);
    proxy_pass_through(state, &path).await
}

fn json_response(data: Vec<u8>, cache_control: &str, x_cache: &str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, cache_control)
        .header("X-Cache", x_cache)
        .body(Body::from(data))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn serve_stale_cache(cache_file: &std::path::Path, path: &str) -> Option<Response> {
    let cached = read_cached_response(cache_file).await?;
    debug!("[remi-lite] Serving stale cache: {}", path);
    Some(json_response(cached.data, "public, max-age=0", "STALE"))
}

/// Generic pass-through proxy with file-based caching
///
/// Checks local file cache first (with TTL), then proxies to upstream.
/// Caches the upstream response for subsequent requests.
async fn proxy_pass_through(state: Arc<RwLock<ProxyState>>, path: &str) -> Response {
    let state_guard = state.read().await;
    let upstream_url = state_guard.upstream_url.clone();
    let http_client = state_guard.http_client.clone();
    let cache_dir = state_guard.index_cache_dir.clone();
    let offline = state_guard.config.offline;
    drop(state_guard);

    let cache_key = crate::hash::sha256(path.as_bytes());
    let cache_file = cache_dir.join(&cache_key);

    // Check file cache (with TTL)
    if let Some(cached) = read_cached_response(&cache_file).await {
        let age = cached
            .timestamp
            .elapsed()
            .unwrap_or(Duration::from_secs(u64::MAX));
        if age < Duration::from_secs(INDEX_CACHE_TTL_SECS) {
            debug!(
                "[remi-lite] Index cache hit: {} (age: {}s)",
                path,
                age.as_secs()
            );
            let max_age = format!("public, max-age={INDEX_CACHE_TTL_SECS}");
            return json_response(cached.data, &max_age, "HIT");
        }
    }

    // Offline mode: cannot proxy
    if offline || upstream_url.is_none() {
        if let Some(response) = serve_stale_cache(&cache_file, path).await {
            return response;
        }
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "No upstream configured (offline mode)",
        )
            .into_response();
    }

    // Proxy to upstream
    let upstream = upstream_url.expect("checked above");
    let fetch_url = format!("{}/{}", upstream, path);

    debug!("[remi-lite] Proxying: {} -> {}", path, fetch_url);

    let response = match http_client.get(&fetch_url).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!("[remi-lite] Upstream fetch failed: {} ({})", fetch_url, e);
            if let Some(response) = serve_stale_cache(&cache_file, path).await {
                return response;
            }
            return (StatusCode::BAD_GATEWAY, "Upstream fetch failed").into_response();
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        debug!(
            "[remi-lite] Upstream returned {} for {}",
            status, fetch_url
        );
        return (
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            format!("Upstream returned {}", status),
        )
            .into_response();
    }

    let data = match response.bytes().await {
        Ok(d) => d.to_vec(),
        Err(e) => {
            warn!(
                "[remi-lite] Failed to read upstream response: {} ({})",
                fetch_url, e
            );
            return (StatusCode::BAD_GATEWAY, "Failed to read upstream response").into_response();
        }
    };

    if let Err(e) = write_cached_response(&cache_file, &data).await {
        warn!("[remi-lite] Failed to cache index response: {}", e);
    }

    debug!(
        "[remi-lite] Proxied and cached: {} ({} bytes)",
        path,
        data.len()
    );

    let max_age = format!("public, max-age={INDEX_CACHE_TTL_SECS}");
    json_response(data, &max_age, "MISS")
}

// =============================================================================
// File-based Index Cache
// =============================================================================

/// Read a cached response from disk, returning None if not found
async fn read_cached_response(path: &std::path::Path) -> Option<CachedResponse> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    let timestamp = metadata.modified().ok()?;
    let data = tokio::fs::read(path).await.ok()?;

    Some(CachedResponse { data, timestamp })
}

/// Write a cached response to disk atomically
async fn write_cached_response(path: &std::path::Path, data: &[u8]) -> Result<()> {
    let temp_path = path.with_extension("tmp");
    tokio::fs::write(&temp_path, data).await?;
    tokio::fs::rename(&temp_path, path).await?;
    Ok(())
}

// =============================================================================
// Graceful Shutdown
// =============================================================================

/// Wait for ctrl-c signal for graceful shutdown
async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install ctrl-c handler");
    info!("[remi-lite] Shutdown signal received");
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_config_defaults() {
        let config = ProxyConfig::default();

        assert_eq!(config.port, 7891);
        assert_eq!(config.cache_dir, PathBuf::from("/var/cache/conary/proxy"));
        assert!(config.mdns_enabled);
        assert_eq!(config.mdns_scan_secs, 3);
        assert!(!config.offline);
        assert!(config.advertise);
        assert!(config.upstream_url.is_none());
    }

    #[test]
    fn test_proxy_config_custom() {
        let config = ProxyConfig {
            port: 9999,
            upstream_url: Some("https://remi.example.com".to_string()),
            cache_dir: PathBuf::from("/tmp/proxy-cache"),
            mdns_enabled: false,
            mdns_scan_secs: 5,
            offline: false,
            advertise: false,
        };

        assert_eq!(config.port, 9999);
        assert_eq!(
            config.upstream_url.as_deref(),
            Some("https://remi.example.com")
        );
        assert!(!config.mdns_enabled);
        assert!(!config.advertise);
    }

    #[test]
    fn test_proxy_config_offline() {
        let config = ProxyConfig {
            offline: true,
            ..Default::default()
        };

        assert!(config.offline);
        // In offline mode, upstream_url should typically be None
        assert!(config.upstream_url.is_none());
    }

    #[test]
    fn test_is_valid_hash() {
        // Valid 64-char hex hash
        assert!(is_valid_hash(
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
        ));
        // Uppercase hex should also be valid
        assert!(is_valid_hash(
            "ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890"
        ));

        // Wrong length
        assert!(!is_valid_hash("abcdef"));
        assert!(!is_valid_hash(""));
        assert!(!is_valid_hash(
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef12345678901"
        )); // 65 chars

        // Invalid characters
        assert!(!is_valid_hash(
            "zzzzzz1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
        ));
    }

    #[test]
    fn test_build_chunk_fetcher_offline() {
        let config = ProxyConfig {
            offline: true,
            cache_dir: PathBuf::from("/tmp/test-cache"),
            ..Default::default()
        };

        let fetcher = build_chunk_fetcher(&config, None).unwrap();
        // Offline mode should only use local cache
        assert_eq!(fetcher.name(), "local-cache");
    }

    #[test]
    fn test_build_chunk_fetcher_no_upstream() {
        let config = ProxyConfig {
            offline: false,
            cache_dir: PathBuf::from("/tmp/test-cache"),
            ..Default::default()
        };

        let fetcher = build_chunk_fetcher(&config, None).unwrap();
        // No upstream: falls back to local cache only
        assert_eq!(fetcher.name(), "local-cache");
    }

    #[test]
    fn test_build_chunk_fetcher_with_upstream() {
        let config = ProxyConfig {
            offline: false,
            cache_dir: PathBuf::from("/tmp/test-cache"),
            ..Default::default()
        };

        let fetcher = build_chunk_fetcher(&config, Some("http://localhost:8080")).unwrap();
        // With upstream: composite fetcher (local cache + http)
        assert_eq!(fetcher.name(), "composite");
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let config = ProxyConfig::default();
        let state = Arc::new(RwLock::new(ProxyState {
            config: config.clone(),
            upstream_url: Some("http://example.com".to_string()),
            chunk_fetcher: Arc::new(LocalCacheFetcher::new("/tmp/test")),
            http_client: reqwest::Client::new(),
            index_cache_dir: PathBuf::from("/tmp/test-index"),
        }));

        let response = health(State(state)).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_endpoint_offline() {
        let config = ProxyConfig {
            offline: true,
            ..Default::default()
        };
        let state = Arc::new(RwLock::new(ProxyState {
            config: config.clone(),
            upstream_url: None,
            chunk_fetcher: Arc::new(LocalCacheFetcher::new("/tmp/test")),
            http_client: reqwest::Client::new(),
            index_cache_dir: PathBuf::from("/tmp/test-index"),
        }));

        let response = health(State(state)).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_proxy_chunk_invalid_hash() {
        let config = ProxyConfig::default();
        let state = Arc::new(RwLock::new(ProxyState {
            config,
            upstream_url: None,
            chunk_fetcher: Arc::new(LocalCacheFetcher::new("/tmp/nonexistent")),
            http_client: reqwest::Client::new(),
            index_cache_dir: PathBuf::from("/tmp/test-index"),
        }));

        let response = proxy_chunk(State(state), Path("invalid".to_string())).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_proxy_chunk_not_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = ProxyConfig {
            cache_dir: temp_dir.path().to_path_buf(),
            offline: true,
            ..Default::default()
        };
        let state = Arc::new(RwLock::new(ProxyState {
            config,
            upstream_url: None,
            chunk_fetcher: Arc::new(LocalCacheFetcher::new(temp_dir.path())),
            http_client: reqwest::Client::new(),
            index_cache_dir: temp_dir.path().join("index"),
        }));

        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let response = proxy_chunk(State(state), Path(hash.to_string())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_proxy_chunk_cache_hit() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = LocalCacheFetcher::new(temp_dir.path());

        // Store a chunk in the cache
        let data = b"test chunk data for proxy";
        let hash = crate::hash::sha256(data);
        cache.store(&hash, data).await.unwrap();

        let config = ProxyConfig {
            cache_dir: temp_dir.path().to_path_buf(),
            offline: true,
            ..Default::default()
        };
        let state = Arc::new(RwLock::new(ProxyState {
            config,
            upstream_url: None,
            chunk_fetcher: Arc::new(cache),
            http_client: reqwest::Client::new(),
            index_cache_dir: temp_dir.path().join("index"),
        }));

        let response = proxy_chunk(State(state), Path(hash.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_proxy_pass_through_offline_no_cache() {
        let temp_dir = tempfile::tempdir().unwrap();
        let index_dir = temp_dir.path().join("index");
        tokio::fs::create_dir_all(&index_dir).await.unwrap();

        let config = ProxyConfig {
            cache_dir: temp_dir.path().to_path_buf(),
            offline: true,
            ..Default::default()
        };
        let state = Arc::new(RwLock::new(ProxyState {
            config,
            upstream_url: None,
            chunk_fetcher: Arc::new(LocalCacheFetcher::new(temp_dir.path())),
            http_client: reqwest::Client::new(),
            index_cache_dir: index_dir,
        }));

        let response = proxy_pass_through(state, "v1/index/fedora/nginx").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_file_cache_read_write() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_file = temp_dir.path().join("test_cache_entry");
        let data = b"cached response body";

        // Write
        write_cached_response(&cache_file, data).await.unwrap();

        // Read
        let cached = read_cached_response(&cache_file).await.unwrap();
        assert_eq!(cached.data, data);
        assert!(cached.timestamp.elapsed().unwrap() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn test_file_cache_read_nonexistent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_file = temp_dir.path().join("nonexistent");

        let cached = read_cached_response(&cache_file).await;
        assert!(cached.is_none());
    }

    #[test]
    fn test_create_proxy_router() {
        let config = ProxyConfig::default();
        let state = Arc::new(RwLock::new(ProxyState {
            config,
            upstream_url: None,
            chunk_fetcher: Arc::new(LocalCacheFetcher::new("/tmp/test")),
            http_client: reqwest::Client::new(),
            index_cache_dir: PathBuf::from("/tmp/test-index"),
        }));

        // Verify router construction does not panic
        let _router = create_proxy_router(state);
    }

    #[test]
    fn test_gethostname_safe() {
        let hostname = gethostname_safe();
        // Should always return something, never empty
        assert!(!hostname.is_empty());
        // Should not contain dots (stripped to short hostname)
        assert!(!hostname.contains('.'));
    }
}
