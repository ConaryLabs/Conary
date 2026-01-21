// src/server/config.rs
//! Configuration file parsing for the Remi server
//!
//! Supports TOML configuration files with the following sections:
//! - [server] - Bind address, workers, admin settings
//! - [storage] - Root directory, eviction thresholds
//! - [upstream.*] - Upstream repository configuration
//! - [conversion] - CCS conversion settings
//! - [federation] - Federation peer settings
//! - [security] - Rate limiting, banning, CORS

use crate::server::ServerConfig;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// TOML configuration file structure
#[derive(Debug, Deserialize)]
pub struct RemiConfig {
    /// Server settings
    #[serde(default)]
    pub server: ServerSection,

    /// Storage settings
    #[serde(default)]
    pub storage: StorageSection,

    /// Upstream repositories
    #[serde(default)]
    pub upstream: HashMap<String, UpstreamSection>,

    /// Conversion settings
    #[serde(default)]
    pub conversion: ConversionSection,

    /// Federation settings
    #[serde(default)]
    pub federation: FederationSection,

    /// Security settings
    #[serde(default)]
    pub security: SecuritySection,

    /// Builder settings
    #[serde(default)]
    pub builder: BuilderSection,
}

impl Default for RemiConfig {
    fn default() -> Self {
        Self {
            server: ServerSection::default(),
            storage: StorageSection::default(),
            upstream: HashMap::new(),
            conversion: ConversionSection::default(),
            federation: FederationSection::default(),
            security: SecuritySection::default(),
            builder: BuilderSection::default(),
        }
    }
}

/// Server configuration section
#[derive(Debug, Deserialize)]
pub struct ServerSection {
    /// Public API bind address
    #[serde(default = "default_bind")]
    pub bind: String,

    /// Admin API bind address (localhost only for security)
    #[serde(default = "default_admin_bind")]
    pub admin_bind: String,

    /// Number of worker threads (0 = auto)
    #[serde(default)]
    pub workers: usize,

    /// Enable Prometheus metrics
    #[serde(default = "default_true")]
    pub metrics: bool,

    /// Enable audit logging
    #[serde(default = "default_true")]
    pub audit_log: bool,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            admin_bind: default_admin_bind(),
            workers: 0,
            metrics: true,
            audit_log: true,
        }
    }
}

fn default_bind() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_admin_bind() -> String {
    "127.0.0.1:8081".to_string()
}

fn default_true() -> bool {
    true
}

/// Storage configuration section
#[derive(Debug, Deserialize)]
pub struct StorageSection {
    /// Root directory for all storage
    #[serde(default = "default_root")]
    pub root: PathBuf,

    /// Eviction threshold (0.0-1.0, e.g., 0.90 = 90%)
    #[serde(default = "default_eviction_threshold")]
    pub eviction_threshold: f64,

    /// Minimum age for eviction (e.g., "1h", "30m")
    #[serde(default = "default_eviction_min_age")]
    pub eviction_min_age: String,

    /// Negative cache TTL (e.g., "15m", "1h")
    #[serde(default = "default_negative_cache_ttl")]
    pub negative_cache_ttl: String,

    /// Maximum cache size in bytes (e.g., "700GB", "1TB")
    #[serde(default)]
    pub max_cache_size: Option<String>,
}

impl Default for StorageSection {
    fn default() -> Self {
        Self {
            root: default_root(),
            eviction_threshold: 0.90,
            eviction_min_age: default_eviction_min_age(),
            negative_cache_ttl: default_negative_cache_ttl(),
            max_cache_size: None,
        }
    }
}

fn default_root() -> PathBuf {
    PathBuf::from("/conary")
}

fn default_eviction_threshold() -> f64 {
    0.90
}

fn default_eviction_min_age() -> String {
    "1h".to_string()
}

fn default_negative_cache_ttl() -> String {
    "15m".to_string()
}

/// Upstream repository configuration
#[derive(Debug, Deserialize)]
pub struct UpstreamSection {
    /// Metalink URL for mirror discovery
    pub metalink: Option<String>,

    /// Direct base URL
    pub base_url: Option<String>,

    /// Supported releases
    #[serde(default)]
    pub releases: Vec<String>,

    /// Supported architectures
    #[serde(default)]
    pub arches: Vec<String>,

    /// Metadata refresh interval (e.g., "6h", "24h")
    #[serde(default = "default_metadata_refresh")]
    pub metadata_refresh: String,

    /// Priority (lower = preferred)
    #[serde(default = "default_priority")]
    pub priority: u32,
}

fn default_metadata_refresh() -> String {
    "6h".to_string()
}

fn default_priority() -> u32 {
    100
}

/// CCS conversion configuration
#[derive(Debug, Deserialize)]
pub struct ConversionSection {
    /// Enable content-defined chunking
    #[serde(default = "default_true")]
    pub chunking: bool,

    /// Minimum chunk size
    #[serde(default = "default_chunk_min")]
    pub chunk_min: usize,

    /// Average chunk size
    #[serde(default = "default_chunk_avg")]
    pub chunk_avg: usize,

    /// Maximum chunk size
    #[serde(default = "default_chunk_max")]
    pub chunk_max: usize,

    /// Strip debug symbols from binaries
    #[serde(default)]
    pub strip_debug: bool,

    /// Maximum concurrent conversions
    #[serde(default = "default_max_conversions")]
    pub max_concurrent: usize,
}

impl Default for ConversionSection {
    fn default() -> Self {
        Self {
            chunking: true,
            chunk_min: 16384,
            chunk_avg: 65536,
            chunk_max: 262144,
            strip_debug: false,
            max_concurrent: 4,
        }
    }
}

fn default_chunk_min() -> usize {
    16384
}

fn default_chunk_avg() -> usize {
    65536
}

fn default_chunk_max() -> usize {
    262144
}

fn default_max_conversions() -> usize {
    4
}

/// Federation configuration
#[derive(Debug, Deserialize)]
pub struct FederationSection {
    /// Enable federation
    #[serde(default)]
    pub enabled: bool,

    /// Federation tier (region_hub, cell_hub, leaf)
    #[serde(default = "default_tier")]
    pub tier: String,

    /// mTLS certificate path
    pub cert_path: Option<PathBuf>,

    /// mTLS key path
    pub key_path: Option<PathBuf>,

    /// mTLS CA certificate path for peer verification
    pub ca_path: Option<PathBuf>,

    /// Ed25519 signing key path
    pub signing_key: Option<PathBuf>,

    /// Peer URLs
    #[serde(default)]
    pub peers: Vec<String>,
}

impl Default for FederationSection {
    fn default() -> Self {
        Self {
            enabled: false,
            tier: default_tier(),
            cert_path: None,
            key_path: None,
            ca_path: None,
            signing_key: None,
            peers: Vec::new(),
        }
    }
}

fn default_tier() -> String {
    "leaf".to_string()
}

/// Security configuration
#[derive(Debug, Deserialize)]
pub struct SecuritySection {
    /// Enable rate limiting
    #[serde(default = "default_true")]
    pub rate_limit: bool,

    /// Requests per second per IP
    #[serde(default = "default_rate_limit_rps")]
    pub rate_limit_rps: u32,

    /// Rate limit burst size
    #[serde(default = "default_rate_limit_burst")]
    pub rate_limit_burst: u32,

    /// CORS allowed origins (empty = same-origin only)
    #[serde(default)]
    pub cors_origins: Vec<String>,

    /// Ban threshold (failures before ban)
    #[serde(default = "default_ban_threshold")]
    pub ban_threshold: u32,

    /// Ban duration (e.g., "5m", "1h")
    #[serde(default = "default_ban_duration")]
    pub ban_duration: String,

    /// Trusted proxy header for real IP extraction (e.g., "CF-Connecting-IP")
    #[serde(default)]
    pub trusted_proxy_header: Option<String>,

    /// Cloudflare IP ranges file for validation
    #[serde(default)]
    pub cloudflare_ips_file: Option<PathBuf>,
}

impl Default for SecuritySection {
    fn default() -> Self {
        Self {
            rate_limit: true,
            rate_limit_rps: 100,
            rate_limit_burst: 200,
            cors_origins: Vec::new(),
            ban_threshold: 10,
            ban_duration: default_ban_duration(),
            trusted_proxy_header: None,
            cloudflare_ips_file: None,
        }
    }
}

fn default_rate_limit_rps() -> u32 {
    100
}

fn default_rate_limit_burst() -> u32 {
    200
}

fn default_ban_threshold() -> u32 {
    10
}

fn default_ban_duration() -> String {
    "5m".to_string()
}

/// Builder configuration section
#[derive(Debug, Deserialize)]
pub struct BuilderSection {
    /// Enable builder service
    #[serde(default)]
    pub enabled: bool,

    /// Build work directory
    #[serde(default = "default_build_work_dir")]
    pub work_dir: PathBuf,

    /// Maximum concurrent builds
    #[serde(default = "default_max_builds")]
    pub max_concurrent: usize,

    /// Enable container isolation for builds
    #[serde(default = "default_true")]
    pub isolation: bool,

    /// Block network during build phase
    #[serde(default = "default_true")]
    pub network_blocked: bool,

    /// Bootstrap image output directory
    #[serde(default = "default_bootstrap_dir")]
    pub bootstrap_dir: PathBuf,

    /// Auto-chunk bootstrap images into CAS
    #[serde(default = "default_true")]
    pub chunk_bootstrap: bool,
}

impl Default for BuilderSection {
    fn default() -> Self {
        Self {
            enabled: false,
            work_dir: default_build_work_dir(),
            max_concurrent: 2,
            isolation: true,
            network_blocked: true,
            bootstrap_dir: default_bootstrap_dir(),
            chunk_bootstrap: true,
        }
    }
}

fn default_build_work_dir() -> PathBuf {
    PathBuf::from("/conary/build")
}

fn default_max_builds() -> usize {
    2
}

fn default_bootstrap_dir() -> PathBuf {
    PathBuf::from("/conary/bootstrap")
}

impl RemiConfig {
    /// Load configuration from a TOML file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: RemiConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        config.validate()?;
        Ok(config)
    }

    /// Create a default configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        // Validate bind addresses
        self.server
            .bind
            .parse::<SocketAddr>()
            .with_context(|| format!("Invalid server.bind address: {}", self.server.bind))?;

        self.server.admin_bind.parse::<SocketAddr>().with_context(|| {
            format!("Invalid server.admin_bind address: {}", self.server.admin_bind)
        })?;

        // Validate eviction threshold
        if !(0.0..=1.0).contains(&self.storage.eviction_threshold) {
            anyhow::bail!(
                "storage.eviction_threshold must be between 0.0 and 1.0, got {}",
                self.storage.eviction_threshold
            );
        }

        // Validate chunk sizes
        if self.conversion.chunk_min > self.conversion.chunk_avg {
            anyhow::bail!("conversion.chunk_min must be <= conversion.chunk_avg");
        }
        if self.conversion.chunk_avg > self.conversion.chunk_max {
            anyhow::bail!("conversion.chunk_avg must be <= conversion.chunk_max");
        }

        // Validate federation tier
        let valid_tiers = ["region_hub", "cell_hub", "leaf"];
        if !valid_tiers.contains(&self.federation.tier.as_str()) {
            anyhow::bail!(
                "federation.tier must be one of {:?}, got '{}'",
                valid_tiers,
                self.federation.tier
            );
        }

        Ok(())
    }

    /// Convert to the internal ServerConfig structure
    pub fn to_server_config(&self) -> Result<ServerConfig> {
        let bind_addr = self.server.bind.parse()?;

        // Parse cache max size
        let cache_max_bytes = if let Some(ref size_str) = self.storage.max_cache_size {
            parse_size(size_str)?
        } else {
            // Default: 90% of storage threshold applied to 1TB
            700 * 1024 * 1024 * 1024 // 700GB
        };

        // Parse ban duration
        let ban_duration_secs = parse_duration(&self.security.ban_duration)?.as_secs();

        Ok(ServerConfig {
            bind_addr,
            db_path: self.storage.root.join("metadata/conary.db"),
            chunk_dir: self.storage.root.join("chunks"),
            cache_dir: self.storage.root.join("cache"),
            max_concurrent_conversions: self.conversion.max_concurrent,
            cache_max_bytes,
            chunk_ttl_days: 30, // Could make configurable
            enable_bloom_filter: true,
            bloom_expected_chunks: 1_000_000,
            upstream_url: self.get_primary_upstream_url(),
            upstream_timeout: Duration::from_secs(30),
            enable_rate_limit: self.security.rate_limit,
            rate_limit_rps: self.security.rate_limit_rps,
            rate_limit_burst: self.security.rate_limit_burst,
            cors_allowed_origins: self.security.cors_origins.clone(),
            enable_audit_log: self.server.audit_log,
            ban_threshold: self.security.ban_threshold,
            ban_duration_secs,
        })
    }

    /// Get admin bind address
    pub fn admin_bind_addr(&self) -> Result<SocketAddr> {
        self.server.admin_bind.parse().with_context(|| {
            format!("Invalid admin bind address: {}", self.server.admin_bind)
        })
    }

    /// Get primary upstream URL (first configured)
    fn get_primary_upstream_url(&self) -> Option<String> {
        self.upstream.values().find_map(|u| {
            u.base_url
                .clone()
                .or_else(|| u.metalink.clone())
        })
    }

    /// Get the storage root directory
    pub fn storage_root(&self) -> &Path {
        &self.storage.root
    }

    /// Get all storage subdirectories that should exist
    pub fn storage_dirs(&self) -> Vec<PathBuf> {
        vec![
            self.storage.root.join("chunks"),
            self.storage.root.join("converted"),
            self.storage.root.join("built"),
            self.storage.root.join("bootstrap"),
            self.storage.root.join("build"),
            self.storage.root.join("metadata"),
            self.storage.root.join("manifests"),
            self.storage.root.join("keys"),
            self.storage.root.join("cache"),
        ]
    }

    /// Parse negative cache TTL to Duration
    pub fn negative_cache_duration(&self) -> Result<Duration> {
        parse_duration(&self.storage.negative_cache_ttl)
    }

    /// Parse eviction min age to Duration
    pub fn eviction_min_age(&self) -> Result<Duration> {
        parse_duration(&self.storage.eviction_min_age)
    }

    /// Get trusted proxy header for IP extraction
    pub fn trusted_proxy_header(&self) -> Option<&str> {
        self.security.trusted_proxy_header.as_deref()
    }
}

/// Parse a human-readable size string (e.g., "700GB", "1TB", "512MB")
pub fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim().to_uppercase();

    let (num_str, multiplier) = if s.ends_with("TB") {
        (&s[..s.len() - 2], 1024u64 * 1024 * 1024 * 1024)
    } else if s.ends_with("GB") {
        (&s[..s.len() - 2], 1024u64 * 1024 * 1024)
    } else if s.ends_with("MB") {
        (&s[..s.len() - 2], 1024u64 * 1024)
    } else if s.ends_with("KB") {
        (&s[..s.len() - 2], 1024u64)
    } else if s.ends_with('B') {
        (&s[..s.len() - 1], 1u64)
    } else {
        // Assume bytes
        (s.as_str(), 1u64)
    };

    let num: f64 = num_str
        .trim()
        .parse()
        .with_context(|| format!("Invalid size number: {}", num_str))?;

    Ok((num * multiplier as f64) as u64)
}

/// Parse a human-readable duration string (e.g., "15m", "1h", "30s")
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim().to_lowercase();

    let (num_str, multiplier) = if s.ends_with('d') {
        (&s[..s.len() - 1], 24 * 60 * 60)
    } else if s.ends_with('h') {
        (&s[..s.len() - 1], 60 * 60)
    } else if s.ends_with('m') {
        (&s[..s.len() - 1], 60)
    } else if s.ends_with('s') {
        (&s[..s.len() - 1], 1)
    } else {
        // Assume seconds
        (s.as_str(), 1)
    };

    let num: u64 = num_str
        .trim()
        .parse()
        .with_context(|| format!("Invalid duration number: {}", num_str))?;

    Ok(Duration::from_secs(num * multiplier))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("1KB").unwrap(), 1024);
        assert_eq!(parse_size("1MB").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1GB").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size("1TB").unwrap(), 1024u64 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("700GB").unwrap(), 700 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1.5GB").unwrap(), (1.5 * 1024.0 * 1024.0 * 1024.0) as u64);
    }

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("15m").unwrap(), Duration::from_secs(15 * 60));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("2d").unwrap(), Duration::from_secs(2 * 24 * 3600));
    }

    #[test]
    fn test_default_config() {
        let config = RemiConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.server.bind, "0.0.0.0:8080");
        assert_eq!(config.server.admin_bind, "127.0.0.1:8081");
    }

    #[test]
    fn test_storage_dirs() {
        let config = RemiConfig::default();
        let dirs = config.storage_dirs();
        assert!(dirs.contains(&PathBuf::from("/conary/chunks")));
        assert!(dirs.contains(&PathBuf::from("/conary/metadata")));
        assert!(dirs.contains(&PathBuf::from("/conary/bootstrap")));
    }

    #[test]
    fn test_parse_toml() {
        let toml_str = r#"
[server]
bind = "0.0.0.0:8080"
admin_bind = "127.0.0.1:8081"
workers = 4

[storage]
root = "/conary"
eviction_threshold = 0.90
negative_cache_ttl = "15m"

[upstream.fedora]
metalink = "https://mirrors.fedoraproject.org/metalink"
releases = ["43"]
arches = ["x86_64"]

[conversion]
chunking = true
chunk_min = 16384
chunk_avg = 65536
chunk_max = 262144

[federation]
enabled = false

[security]
rate_limit = true
rate_limit_rps = 100
"#;
        let config: RemiConfig = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.server.workers, 4);
        assert!(config.upstream.contains_key("fedora"));
        assert!(!config.federation.enabled);
    }

    #[test]
    fn test_invalid_eviction_threshold() {
        let toml_str = r#"
[storage]
eviction_threshold = 1.5
"#;
        let config: RemiConfig = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_chunk_sizes() {
        let toml_str = r#"
[conversion]
chunk_min = 100000
chunk_avg = 50000
"#;
        let config: RemiConfig = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_err());
    }
}
