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

/// Per-tier allowlists for restricting which peers can participate
///
/// When an allowlist is set for a tier, only peers whose endpoints match
/// the allowlist patterns will be used. Patterns support:
/// - Exact match: `"https://remi.conary.io:7891"`
/// - Prefix match: `"https://remi.conary.io:*"` (any port)
/// - Wildcard subdomain: `"https://*.conary.io:7891"`
///
/// If an allowlist is `None`, all peers of that tier are allowed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TierAllowlists {
    /// Allowed cell hub endpoints (LAN peers)
    #[serde(default)]
    pub cell_hubs: Option<Vec<String>>,

    /// Allowed region hub endpoints (WAN peers)
    #[serde(default)]
    pub region_hubs: Option<Vec<String>>,

    /// Allowed leaf endpoints (other nodes)
    #[serde(default)]
    pub leaves: Option<Vec<String>>,
}

impl TierAllowlists {
    /// Check if an endpoint is allowed for a given tier
    pub fn is_allowed(&self, endpoint: &str, tier: PeerTier) -> bool {
        let allowlist = match tier {
            PeerTier::CellHub => &self.cell_hubs,
            PeerTier::RegionHub => &self.region_hubs,
            PeerTier::Leaf => &self.leaves,
        };

        match allowlist {
            None => true, // No allowlist = allow all
            Some(patterns) => patterns.iter().any(|pattern| endpoint_matches(endpoint, pattern)),
        }
    }

    /// Check if any tier has an allowlist configured
    pub fn has_any(&self) -> bool {
        self.cell_hubs.is_some() || self.region_hubs.is_some() || self.leaves.is_some()
    }
}

/// Check if an endpoint matches a pattern
///
/// Supports:
/// - Exact match
/// - Port wildcard: `https://host:*` matches any port
/// - Subdomain wildcard: `https://*.domain.com:port`
fn endpoint_matches(endpoint: &str, pattern: &str) -> bool {
    // Exact match
    if endpoint == pattern {
        return true;
    }

    // Port wildcard: pattern ends with ":*"
    if let Some(prefix) = pattern.strip_suffix(":*") {
        // Check if endpoint starts with the prefix and has a port
        if let Some(rest) = endpoint.strip_prefix(prefix) {
            // rest should be ":" followed by digits
            return rest.starts_with(':') && rest[1..].chars().all(|c| c.is_ascii_digit());
        }
    }

    // Subdomain wildcard: pattern contains "*."
    if pattern.contains("*.") {
        // Check for combined subdomain + port wildcard: "https://*.domain:*"
        let has_port_wildcard = pattern.ends_with(":*");
        let pattern_for_parsing = if has_port_wildcard {
            pattern.strip_suffix(":*").unwrap_or(pattern)
        } else {
            pattern
        };

        // Parse pattern into scheme://[*.]host:port format
        let pattern_without_wildcard = pattern_for_parsing.replace("*.", "");

        // Extract the domain part from both
        if let (Some(endpoint_domain), Some(pattern_domain)) =
            (extract_domain(endpoint), extract_domain(&pattern_without_wildcard))
        {
            // Check if endpoint's domain ends with pattern's domain
            if endpoint_domain == pattern_domain || endpoint_domain.ends_with(&format!(".{}", pattern_domain)) {
                // Also verify scheme matches
                let endpoint_scheme = endpoint.split("://").next().unwrap_or("");
                let pattern_scheme = pattern.split("://").next().unwrap_or("");
                if endpoint_scheme == pattern_scheme {
                    // Port wildcard means any port is OK
                    if has_port_wildcard {
                        return true;
                    }
                    // Check port (using defaults for implicit ports)
                    let endpoint_port = extract_port_with_default(endpoint);
                    let pattern_port = extract_port_with_default(&pattern_without_wildcard);
                    return endpoint_port == pattern_port;
                }
            }
        }
    }

    false
}

/// Extract domain from URL (without port)
fn extract_domain(url: &str) -> Option<String> {
    let without_scheme = url.split("://").nth(1)?;
    let host_port = without_scheme.split('/').next()?;
    let host = host_port.split(':').next()?;
    Some(host.to_string())
}

/// Extract port from URL, returning default port if not specified
fn extract_port_with_default(url: &str) -> Option<String> {
    let scheme = url.split("://").next()?;
    let without_scheme = url.split("://").nth(1)?;
    let host_port = without_scheme.split('/').next()?;

    // Check for explicit port
    if let Some(port) = host_port.split(':').nth(1) {
        return Some(port.to_string());
    }

    // Return default port based on scheme
    match scheme {
        "https" => Some("443".to_string()),
        "http" => Some("80".to_string()),
        _ => None,
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
/// region_hubs = ["https://remi.conary.io"]  # Behind Cloudflare, port 443
/// cell_hubs = ["http://rack-cache.local:7891"]
/// prefer_cell = true
/// rendezvous_k = 3
/// circuit_threshold = 5
/// circuit_cooldown_secs = 30
/// request_timeout_ms = 5000
/// max_chunk_size = 524288
///
/// # mTLS for WAN region hubs (required in production)
/// require_mtls_wan = true
/// mtls_cert_path = "/etc/conary/federation/client.crt"
/// mtls_key_path = "/etc/conary/federation/client.key"
/// mtls_ca_path = "/etc/conary/federation/ca.crt"  # Optional, uses system CA if not set
///
/// # Per-tier allowlists (optional)
/// [federation.tier_allowlists]
/// cell_hubs = ["http://192.168.1.*:7891", "http://rack-*:7891"]
/// region_hubs = ["https://*.conary.io:*"]  # Allow any port for conary.io subdomains
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

    /// Path to client certificate for mTLS (PEM format)
    #[serde(default)]
    pub mtls_cert_path: Option<String>,

    /// Path to client private key for mTLS (PEM format)
    #[serde(default)]
    pub mtls_key_path: Option<String>,

    /// Path to CA certificate for verifying region hub servers (PEM format)
    /// If not set, uses system root certificates
    #[serde(default)]
    pub mtls_ca_path: Option<String>,

    /// Global allowlist of peer endpoints (if set, only these peers are allowed)
    /// Consider using `tier_allowlists` for more granular control.
    #[serde(default)]
    pub allowed_peers: Option<Vec<String>>,

    /// Per-tier allowlists for fine-grained control over which peers are allowed
    #[serde(default)]
    pub tier_allowlists: TierAllowlists,

    /// Listen port for this node (if acting as hub)
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,

    /// Maximum number of nodes per cell for rendezvous scoping
    #[serde(default = "default_max_cell_size")]
    pub max_cell_size: usize,

    /// Upstream URL for pull-through caching (cell hubs only)
    #[serde(default)]
    pub upstream: Option<String>,

    /// Trusted public keys for manifest verification (base64-encoded Ed25519)
    #[serde(default)]
    pub manifest_trusted_keys: Vec<String>,

    /// Allow fetching resources with unsigned manifests (default: true)
    /// Set to false in production to require signed manifests
    #[serde(default = "default_manifest_allow_unsigned")]
    pub manifest_allow_unsigned: bool,
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

fn default_manifest_allow_unsigned() -> bool {
    true // Permissive by default, set false in production
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
            mtls_cert_path: None,
            mtls_key_path: None,
            mtls_ca_path: None,
            allowed_peers: None,
            tier_allowlists: TierAllowlists::default(),
            listen_port: default_listen_port(),
            max_cell_size: default_max_cell_size(),
            upstream: None,
            manifest_trusted_keys: Vec::new(),
            manifest_allow_unsigned: default_manifest_allow_unsigned(),
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

    // =========================================================================
    // Tier Allowlist Tests
    // =========================================================================

    #[test]
    fn test_endpoint_matches_exact() {
        assert!(endpoint_matches("https://remi.conary.io:7891", "https://remi.conary.io:7891"));
        assert!(!endpoint_matches("https://remi.conary.io:7891", "https://other.conary.io:7891"));
    }

    #[test]
    fn test_endpoint_matches_port_wildcard() {
        assert!(endpoint_matches("https://remi.conary.io:7891", "https://remi.conary.io:*"));
        assert!(endpoint_matches("https://remi.conary.io:8080", "https://remi.conary.io:*"));
        assert!(endpoint_matches("https://remi.conary.io:443", "https://remi.conary.io:*"));
        assert!(!endpoint_matches("https://other.conary.io:7891", "https://remi.conary.io:*"));
    }

    #[test]
    fn test_endpoint_matches_subdomain_wildcard() {
        assert!(endpoint_matches("https://remi.conary.io:7891", "https://*.conary.io:7891"));
        assert!(endpoint_matches("https://cell1.conary.io:7891", "https://*.conary.io:7891"));
        assert!(endpoint_matches("https://region.west.conary.io:7891", "https://*.conary.io:7891"));
        assert!(endpoint_matches("https://conary.io:7891", "https://*.conary.io:7891"));
        assert!(!endpoint_matches("https://remi.conary.io:8080", "https://*.conary.io:7891"));
        assert!(!endpoint_matches("http://remi.conary.io:7891", "https://*.conary.io:7891"));
    }

    #[test]
    fn test_endpoint_matches_default_ports() {
        // HTTPS defaults to 443
        assert!(endpoint_matches("https://remi.conary.io", "https://*.conary.io:443"));
        assert!(endpoint_matches("https://remi.conary.io:443", "https://*.conary.io"));
        assert!(endpoint_matches("https://remi.conary.io", "https://*.conary.io"));

        // HTTP defaults to 80
        assert!(endpoint_matches("http://cell.local", "http://*.local:80"));
        assert!(endpoint_matches("http://cell.local:80", "http://*.local"));

        // Mismatched ports should fail
        assert!(!endpoint_matches("https://remi.conary.io:8080", "https://*.conary.io"));
        assert!(!endpoint_matches("https://remi.conary.io", "https://*.conary.io:8080"));
    }

    #[test]
    fn test_endpoint_matches_combined_wildcards() {
        // Subdomain + port wildcard: https://*.conary.io:*
        assert!(endpoint_matches("https://remi.conary.io", "https://*.conary.io:*"));
        assert!(endpoint_matches("https://remi.conary.io:443", "https://*.conary.io:*"));
        assert!(endpoint_matches("https://remi.conary.io:7891", "https://*.conary.io:*"));
        assert!(endpoint_matches("https://cell.conary.io:8080", "https://*.conary.io:*"));

        // Scheme must still match
        assert!(!endpoint_matches("http://remi.conary.io:7891", "https://*.conary.io:*"));
    }

    #[test]
    fn test_tier_allowlists_no_allowlist() {
        let allowlists = TierAllowlists::default();

        // No allowlist = allow all
        assert!(allowlists.is_allowed("http://any.host:7891", PeerTier::CellHub));
        assert!(allowlists.is_allowed("https://any.host:7891", PeerTier::RegionHub));
        assert!(allowlists.is_allowed("http://any.host:7891", PeerTier::Leaf));
    }

    #[test]
    fn test_tier_allowlists_cell_only() {
        let allowlists = TierAllowlists {
            cell_hubs: Some(vec!["http://192.168.1.*:7891".to_string()]),
            region_hubs: None,
            leaves: None,
        };

        // Cell hubs restricted to 192.168.1.* pattern
        assert!(!allowlists.is_allowed("http://10.0.0.1:7891", PeerTier::CellHub));
        // Note: our wildcard matching uses subdomain style, not IP range
        // For IP ranges, exact patterns would be needed

        // Other tiers not restricted
        assert!(allowlists.is_allowed("https://any.host:7891", PeerTier::RegionHub));
        assert!(allowlists.is_allowed("http://any.host:7891", PeerTier::Leaf));
    }

    #[test]
    fn test_tier_allowlists_region_restricted() {
        let allowlists = TierAllowlists {
            cell_hubs: None,
            region_hubs: Some(vec![
                "https://remi.conary.io:7891".to_string(),
                "https://*.trusted.net:7891".to_string(),
            ]),
            leaves: None,
        };

        // Region hubs restricted
        assert!(allowlists.is_allowed("https://remi.conary.io:7891", PeerTier::RegionHub));
        assert!(allowlists.is_allowed("https://west.trusted.net:7891", PeerTier::RegionHub));
        assert!(!allowlists.is_allowed("https://untrusted.net:7891", PeerTier::RegionHub));

        // Other tiers not restricted
        assert!(allowlists.is_allowed("http://any.host:7891", PeerTier::CellHub));
        assert!(allowlists.is_allowed("http://any.host:7891", PeerTier::Leaf));
    }

    #[test]
    fn test_tier_allowlists_has_any() {
        let empty = TierAllowlists::default();
        assert!(!empty.has_any());

        let with_cell = TierAllowlists {
            cell_hubs: Some(vec!["http://host:7891".to_string()]),
            ..Default::default()
        };
        assert!(with_cell.has_any());

        let with_region = TierAllowlists {
            region_hubs: Some(vec!["https://host:7891".to_string()]),
            ..Default::default()
        };
        assert!(with_region.has_any());
    }

    #[test]
    fn test_tier_allowlists_serde() {
        let toml = r#"
            enabled = true
            tier = "leaf"

            [tier_allowlists]
            cell_hubs = ["http://192.168.1.*:7891"]
            region_hubs = ["https://*.conary.io:7891"]
        "#;

        let config: FederationConfig = toml::from_str(toml).unwrap();
        assert!(config.tier_allowlists.has_any());
        assert!(config.tier_allowlists.cell_hubs.is_some());
        assert!(config.tier_allowlists.region_hubs.is_some());
        assert!(config.tier_allowlists.leaves.is_none());
    }
}
