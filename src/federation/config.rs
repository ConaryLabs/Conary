// src/federation/config.rs
//! Federation configuration types

use serde::{Deserialize, Serialize};

/// Peer tier in the federation hierarchy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PeerTier {
    /// WAN hub, requires mTLS
    RegionHub,
    /// Site-local cache (rack-level)
    CellHub,
    /// Individual node (default)
    #[default]
    Leaf,
}

impl std::fmt::Display for PeerTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerTier::RegionHub => write!(f, "region_hub"),
            PeerTier::CellHub => write!(f, "cell_hub"),
            PeerTier::Leaf => write!(f, "leaf"),
        }
    }
}

/// Federation configuration
///
/// Configures how this node participates in the CAS federation network.
///
/// # Example (TOML)
///
/// ```toml
/// [federation]
/// enabled = true
/// tier = "leaf"
/// region_hubs = ["https://remi.conary.io:7891"]
/// cell_hubs = ["http://rack-cache.local:7891"]
/// prefer_cell = true
/// rendezvous_k = 3
/// circuit_threshold = 5
/// circuit_cooldown_secs = 30
/// request_timeout_ms = 5000
/// max_chunk_size = 524288
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
    pub tier: PeerTier,

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

    /// Require mTLS for WAN peers (default: true in production)
    #[serde(default)]
    pub require_mtls_wan: bool,

    /// Allowlist of peer endpoints (if set, only these peers are allowed)
    #[serde(default)]
    pub allowed_peers: Option<Vec<String>>,

    /// Listen port for this node (if acting as hub)
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,

    /// Maximum number of nodes per cell for rendezvous scoping
    #[serde(default = "default_max_cell_size")]
    pub max_cell_size: usize,

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

fn default_max_cell_size() -> usize {
    50
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            node_id: None,
            tier: PeerTier::Leaf,
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
            require_mtls_wan: false,
            allowed_peers: None,
            listen_port: default_listen_port(),
            max_cell_size: default_max_cell_size(),
            upstream: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = FederationConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.tier, PeerTier::Leaf);
        assert_eq!(config.rendezvous_k, 3);
        assert_eq!(config.circuit_threshold, 5);
        assert_eq!(config.max_chunk_size, 512 * 1024);
    }

    #[test]
    fn test_peer_tier_display() {
        assert_eq!(format!("{}", PeerTier::Leaf), "leaf");
        assert_eq!(format!("{}", PeerTier::CellHub), "cell_hub");
        assert_eq!(format!("{}", PeerTier::RegionHub), "region_hub");
    }

    #[test]
    fn test_config_serde() {
        let toml = r#"
            enabled = true
            tier = "cell_hub"
            cell_hubs = ["http://local:7891"]
            region_hubs = ["https://remi.conary.io:7891"]
            rendezvous_k = 5
        "#;

        let config: FederationConfig = toml::from_str(toml).unwrap();
        assert!(config.enabled);
        assert_eq!(config.tier, PeerTier::CellHub);
        assert_eq!(config.cell_hubs.len(), 1);
        assert_eq!(config.region_hubs.len(), 1);
        assert_eq!(config.rendezvous_k, 5);
    }
}
