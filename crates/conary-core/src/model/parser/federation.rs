// conary-core/src/model/parser/federation.rs

//! Federation configuration types owned by the system-model parser.

use serde::{Deserialize, Serialize};

/// Peer tier in the federation hierarchy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FederationTier {
    /// WAN hub, requires mTLS
    RegionHub,
    /// Site-local cache (rack-level)
    CellHub,
    /// Individual node (default)
    #[default]
    Leaf,
}

/// Federation configuration for CAS sharing across machines
///
/// Enables multiple machines to share content-addressable storage chunks
/// over a network, reducing bandwidth and storage by deduplicating content.
///
/// # Example (TOML)
///
/// ```toml
/// [federation]
/// enabled = true
/// tier = "leaf"
/// region_hubs = ["https://remi.conary.io:7891"]
/// cell_hubs = ["http://rack-cache.local:7891"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationConfig {
    /// Enable federation (default: false)
    #[serde(default)]
    pub enabled: bool,

    /// Optional node identifier (auto-generated if not set)
    #[serde(default)]
    pub node_id: Option<String>,

    /// What role is this node? (default: leaf)
    #[serde(default)]
    pub tier: FederationTier,

    /// Cell-local hubs (fast path, LAN)
    #[serde(default)]
    pub cell_hubs: Vec<String>,

    /// WAN hubs (mTLS required in production)
    #[serde(default)]
    pub region_hubs: Vec<String>,

    /// Enable mDNS for LAN peer discovery (default: false)
    #[serde(default)]
    pub enable_mdns: bool,

    /// Number of candidate peers per chunk (default: 3)
    #[serde(default = "default_rendezvous_k")]
    pub rendezvous_k: usize,

    /// Try cell peers before region peers (default: true)
    #[serde(default = "default_prefer_cell")]
    pub prefer_cell: bool,

    /// Failures before opening circuit breaker (default: 5)
    #[serde(default = "default_circuit_threshold")]
    pub circuit_threshold: u32,

    /// Cooldown before retrying open circuit (default: 30)
    #[serde(default = "default_circuit_cooldown")]
    pub circuit_cooldown_secs: u64,

    /// Random jitter factor for cooldowns (default: 0.5 = 50%)
    #[serde(default = "default_jitter_factor")]
    pub jitter_factor: f32,

    /// Per-request timeout in milliseconds (default: 5000)
    #[serde(default = "default_request_timeout")]
    pub request_timeout_ms: u64,

    /// Maximum chunk size to accept (default: 512KB)
    #[serde(default = "default_max_chunk_size")]
    pub max_chunk_size: usize,

    /// Listen port for this node (if acting as hub)
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,

    /// Upstream URL for pull-through caching (cell hubs only)
    #[serde(default)]
    pub upstream: Option<String>,
}

fn default_rendezvous_k() -> usize {
    3
}

fn default_prefer_cell() -> bool {
    true
}

fn default_circuit_threshold() -> u32 {
    5
}

fn default_circuit_cooldown() -> u64 {
    30
}

fn default_jitter_factor() -> f32 {
    0.5
}

fn default_request_timeout() -> u64 {
    5000
}

fn default_max_chunk_size() -> usize {
    512 * 1024 // 512KB
}

fn default_listen_port() -> u16 {
    7891
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            node_id: None,
            tier: FederationTier::Leaf,
            cell_hubs: Vec::new(),
            region_hubs: Vec::new(),
            enable_mdns: false,
            rendezvous_k: default_rendezvous_k(),
            prefer_cell: default_prefer_cell(),
            circuit_threshold: default_circuit_threshold(),
            circuit_cooldown_secs: default_circuit_cooldown(),
            jitter_factor: default_jitter_factor(),
            request_timeout_ms: default_request_timeout(),
            max_chunk_size: default_max_chunk_size(),
            listen_port: default_listen_port(),
            upstream: None,
        }
    }
}
