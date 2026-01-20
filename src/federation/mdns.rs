// src/federation/mdns.rs
//! mDNS-based service discovery for cell-local peer discovery
//!
//! Uses mDNS (Multicast DNS) to automatically discover Conary CAS peers
//! on the local network without manual configuration.
//!
//! # Service Type
//!
//! Conary announces itself with the service type `_conary-cas._tcp.local.`
//! The following TXT properties are included:
//! - `tier`: The peer's tier (leaf, cell_hub, region_hub)
//! - `node_id`: The peer's unique identifier
//! - `version`: Conary protocol version
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │                  Local Network                       │
//! │                                                      │
//! │  ┌──────────┐    mDNS     ┌──────────┐              │
//! │  │ Conary A │◄──────────►│ Conary B │              │
//! │  │ (leaf)   │             │ (cell_hub)│              │
//! │  └──────────┘             └──────────┘              │
//! │       │                         │                    │
//! │       └─────────┬───────────────┘                    │
//! │                 ▼                                    │
//! │           ┌──────────┐                              │
//! │           │ Conary C │                              │
//! │           │ (leaf)   │                              │
//! │           └──────────┘                              │
//! └─────────────────────────────────────────────────────┘
//! ```

use super::config::PeerTier;
use super::peer::{Peer, PeerId};
use crate::error::{Error, Result};
use mdns_sd::{Receiver, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Service type for Conary CAS federation
const SERVICE_TYPE: &str = "_conary-cas._tcp.local.";

/// Protocol version for compatibility checking
const PROTOCOL_VERSION: &str = "1";

/// Discovery callback for handling found/lost peers
pub type DiscoveryCallback = Box<dyn Fn(MdnsEvent) + Send + Sync>;

/// Events from mDNS discovery
#[derive(Debug, Clone)]
pub enum MdnsEvent {
    /// A new peer was discovered
    PeerFound(DiscoveredPeer),
    /// A peer was lost (stopped advertising)
    PeerLost(PeerId),
    /// Search started for a service type
    SearchStarted(String),
    /// Search stopped
    SearchStopped(String),
}

/// Information about a discovered peer
#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    /// Unique peer ID (from TXT record or generated)
    pub id: PeerId,
    /// Service instance name
    pub instance_name: String,
    /// Full hostname
    pub hostname: String,
    /// IP addresses (may have multiple)
    pub addresses: Vec<IpAddr>,
    /// Port number
    pub port: u16,
    /// Peer tier
    pub tier: PeerTier,
    /// Protocol version
    pub version: String,
    /// Additional properties
    pub properties: HashMap<String, String>,
}

impl DiscoveredPeer {
    /// Convert to a federation Peer
    pub fn to_peer(&self) -> Result<Peer> {
        // Prefer IPv4 addresses for simplicity, fall back to IPv6
        let addr = self
            .addresses
            .iter()
            .find(|a| a.is_ipv4())
            .or_else(|| self.addresses.first())
            .ok_or_else(|| Error::NotFound("No IP address for peer".into()))?;

        let endpoint = format!("http://{}:{}", addr, self.port);
        let mut peer = Peer::from_endpoint(&endpoint, self.tier)?;
        peer.name = Some(self.instance_name.clone());

        Ok(peer)
    }
}

/// mDNS discovery manager
///
/// Handles both service registration (announcing this node) and
/// service discovery (finding other nodes).
pub struct MdnsDiscovery {
    /// The mDNS daemon
    daemon: ServiceDaemon,
    /// Our registered service info (if any)
    registered_service: Option<String>,
    /// Browse receiver (if browsing)
    browse_receiver: Option<Receiver<ServiceEvent>>,
    /// Whether discovery is running
    running: Arc<AtomicBool>,
    /// Discovery thread handle
    discovery_thread: Option<thread::JoinHandle<()>>,
}

impl MdnsDiscovery {
    /// Create a new mDNS discovery manager
    pub fn new() -> Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| Error::InitError(format!("Failed to create mDNS daemon: {e}")))?;

        Ok(Self {
            daemon,
            registered_service: None,
            browse_receiver: None,
            running: Arc::new(AtomicBool::new(false)),
            discovery_thread: None,
        })
    }

    /// Register this node as a mDNS service
    ///
    /// Should only be called if this node is a hub (cell_hub or region_hub)
    /// and wants to be discoverable by other nodes.
    pub fn register(
        &mut self,
        instance_name: &str,
        node_id: &str,
        port: u16,
        tier: PeerTier,
        hostname: Option<&str>,
    ) -> Result<()> {
        // Build TXT properties
        let properties: Vec<(&str, &str)> = vec![
            ("tier", tier.to_string().leak()),
            ("node_id", node_id),
            ("version", PROTOCOL_VERSION),
        ];

        // Get hostname or use a default
        let host = hostname
            .map(String::from)
            .unwrap_or_else(|| get_local_hostname().unwrap_or_else(|| "conary".to_string()));
        let host_with_local = format!("{}.local.", host);

        // Get local IP addresses
        let addrs = get_local_addresses();
        if addrs.is_empty() {
            return Err(Error::InitError(
                "No local IP addresses found for mDNS registration".into(),
            ));
        }

        // Create service info
        let service = ServiceInfo::new(
            SERVICE_TYPE,
            instance_name,
            &host_with_local,
            &addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>()[..],
            port,
            &properties[..],
        )
        .map_err(|e| Error::InitError(format!("Failed to create service info: {e}")))?;

        let fullname = service.get_fullname().to_string();

        // Register with the daemon
        self.daemon
            .register(service)
            .map_err(|e| Error::InitError(format!("Failed to register mDNS service: {e}")))?;

        self.registered_service = Some(fullname.clone());

        info!(
            "[mdns] Registered service: {} ({}:{}) tier={}",
            instance_name, host, port, tier
        );

        Ok(())
    }

    /// Unregister the service
    pub fn unregister(&mut self) -> Result<()> {
        if let Some(fullname) = self.registered_service.take() {
            self.daemon
                .unregister(&fullname)
                .map_err(|e| Error::Federation(format!("Failed to unregister mDNS service: {e}")))?;
            info!("[mdns] Unregistered service: {}", fullname);
        }
        Ok(())
    }

    /// Start browsing for Conary CAS services
    ///
    /// Returns a receiver that will yield discovery events.
    pub fn browse(&mut self) -> Result<Receiver<ServiceEvent>> {
        if self.browse_receiver.is_some() {
            return Err(Error::Federation("Already browsing".into()));
        }

        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| Error::InitError(format!("Failed to start mDNS browse: {e}")))?;

        info!("[mdns] Started browsing for {}", SERVICE_TYPE);
        Ok(receiver)
    }

    /// Start continuous discovery with a callback
    ///
    /// Spawns a background thread that handles discovery events
    /// and calls the provided callback for each event.
    pub fn start_discovery<F>(&mut self, callback: F) -> Result<()>
    where
        F: Fn(MdnsEvent) + Send + Sync + 'static,
    {
        if self.running.load(Ordering::SeqCst) {
            return Err(Error::Federation("Discovery already running".into()));
        }

        let receiver = self.browse()?;
        self.browse_receiver = Some(receiver.clone());

        let running = Arc::clone(&self.running);
        running.store(true, Ordering::SeqCst);

        let callback = Arc::new(callback);

        let handle = thread::spawn(move || {
            info!("[mdns] Discovery thread started");

            while running.load(Ordering::SeqCst) {
                // Use recv_timeout to allow checking the running flag
                match receiver.recv_timeout(Duration::from_millis(500)) {
                    Ok(event) => {
                        if let Some(mdns_event) = process_service_event(event) {
                            callback(mdns_event);
                        }
                    }
                    Err(flume::RecvTimeoutError::Timeout) => {
                        // Continue checking running flag
                    }
                    Err(flume::RecvTimeoutError::Disconnected) => {
                        warn!("[mdns] Browse receiver disconnected");
                        break;
                    }
                }
            }

            info!("[mdns] Discovery thread stopped");
        });

        self.discovery_thread = Some(handle);
        Ok(())
    }

    /// Stop continuous discovery
    pub fn stop_discovery(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        // Stop browsing
        if let Err(e) = self.daemon.stop_browse(SERVICE_TYPE) {
            debug!("[mdns] Error stopping browse: {}", e);
        }
        self.browse_receiver = None;

        // Wait for thread to finish
        if let Some(handle) = self.discovery_thread.take() {
            if let Err(e) = handle.join() {
                error!("[mdns] Discovery thread panicked: {:?}", e);
            }
        }

        info!("[mdns] Discovery stopped");
    }

    /// Check if discovery is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Perform a one-shot discovery scan
    ///
    /// Browses for services for the specified duration and returns
    /// all discovered peers.
    pub fn scan(&self, duration: Duration) -> Result<Vec<DiscoveredPeer>> {
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| Error::InitError(format!("Failed to start mDNS browse: {e}")))?;

        let mut peers = Vec::new();
        let start = std::time::Instant::now();

        while start.elapsed() < duration {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    if let Some(MdnsEvent::PeerFound(peer)) = process_service_event(event) {
                        // Avoid duplicates
                        if !peers.iter().any(|p: &DiscoveredPeer| p.id == peer.id) {
                            peers.push(peer);
                        }
                    }
                }
                Err(flume::RecvTimeoutError::Timeout) => continue,
                Err(flume::RecvTimeoutError::Disconnected) => break,
            }
        }

        // Stop browsing
        let _ = self.daemon.stop_browse(SERVICE_TYPE);

        info!("[mdns] Scan complete: found {} peers", peers.len());
        Ok(peers)
    }

    /// Shutdown the mDNS daemon
    pub fn shutdown(mut self) -> Result<()> {
        self.stop_discovery();
        self.unregister()?;

        if let Err(e) = self.daemon.shutdown() {
            warn!("[mdns] Error during shutdown: {}", e);
        }

        Ok(())
    }
}

impl Drop for MdnsDiscovery {
    fn drop(&mut self) {
        self.stop_discovery();
        let _ = self.unregister();
        // Note: daemon.shutdown() consumes self, so we can't call it here
    }
}

/// Process a ServiceEvent into an MdnsEvent
fn process_service_event(event: ServiceEvent) -> Option<MdnsEvent> {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            debug!("[mdns] Service resolved: {}", info.get_fullname());

            // Extract properties
            let properties: HashMap<String, String> = info
                .get_properties()
                .iter()
                .map(|p| (p.key().to_string(), p.val_str().to_string()))
                .collect();

            // Get tier from properties
            let tier = properties
                .get("tier")
                .and_then(|t| match t.as_str() {
                    "region_hub" => Some(PeerTier::RegionHub),
                    "cell_hub" => Some(PeerTier::CellHub),
                    "leaf" => Some(PeerTier::Leaf),
                    _ => None,
                })
                .unwrap_or(PeerTier::Leaf);

            // Get node_id from properties or generate from fullname
            let node_id = properties
                .get("node_id")
                .cloned()
                .unwrap_or_else(|| crate::hash::sha256(info.get_fullname().as_bytes()));

            let version = properties
                .get("version")
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());

            let peer = DiscoveredPeer {
                id: node_id,
                instance_name: info
                    .get_fullname()
                    .split('.')
                    .next()
                    .unwrap_or("unknown")
                    .to_string(),
                hostname: info.get_hostname().to_string(),
                addresses: info.get_addresses().iter().copied().collect(),
                port: info.get_port(),
                tier,
                version,
                properties,
            };

            Some(MdnsEvent::PeerFound(peer))
        }
        ServiceEvent::ServiceRemoved(_, fullname) => {
            debug!("[mdns] Service removed: {}", fullname);
            // Generate peer ID from fullname (same as we would for unknown peers)
            let peer_id = crate::hash::sha256(fullname.as_bytes());
            Some(MdnsEvent::PeerLost(peer_id))
        }
        ServiceEvent::SearchStarted(service_type) => {
            debug!("[mdns] Search started: {}", service_type);
            Some(MdnsEvent::SearchStarted(service_type))
        }
        ServiceEvent::SearchStopped(service_type) => {
            debug!("[mdns] Search stopped: {}", service_type);
            Some(MdnsEvent::SearchStopped(service_type))
        }
        _ => None,
    }
}

/// Get the local hostname
fn get_local_hostname() -> Option<String> {
    // Try gethostname
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        let mut buf = [0u8; 256];
        unsafe {
            if libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) == 0 {
                if let Ok(cstr) = CStr::from_ptr(buf.as_ptr() as *const libc::c_char).to_str() {
                    // Remove .local or domain suffix if present
                    return Some(cstr.split('.').next().unwrap_or(cstr).to_string());
                }
            }
        }
    }
    None
}

/// Get local IP addresses suitable for mDNS
fn get_local_addresses() -> Vec<IpAddr> {
    let mut addrs = Vec::new();

    // Use a simple approach: try to bind to 0.0.0.0 and get the local address
    // This is a fallback; in practice, mdns-sd handles this internally
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        // Connect to a public address to determine our local interface
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(local_addr) = socket.local_addr() {
                addrs.push(local_addr.ip());
            }
        }
    }

    // Also try IPv6
    if let Ok(socket) = std::net::UdpSocket::bind("[::]:0") {
        if socket.connect("[2001:4860:4860::8888]:80").is_ok() {
            if let Ok(local_addr) = socket.local_addr() {
                let ip = local_addr.ip();
                // Only add if it's not a link-local address
                if !ip.is_loopback() {
                    addrs.push(ip);
                }
            }
        }
    }

    addrs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovered_peer_to_peer() {
        let discovered = DiscoveredPeer {
            id: "test-id".to_string(),
            instance_name: "test-instance".to_string(),
            hostname: "test-host.local.".to_string(),
            addresses: vec!["192.168.1.100".parse().unwrap()],
            port: 7891,
            tier: PeerTier::CellHub,
            version: "1".to_string(),
            properties: HashMap::new(),
        };

        let peer = discovered.to_peer().unwrap();
        assert_eq!(peer.endpoint, "http://192.168.1.100:7891");
        assert_eq!(peer.tier, PeerTier::CellHub);
        assert_eq!(peer.name, Some("test-instance".to_string()));
    }

    #[test]
    fn test_discovered_peer_prefers_ipv4() {
        let discovered = DiscoveredPeer {
            id: "test-id".to_string(),
            instance_name: "test-instance".to_string(),
            hostname: "test-host.local.".to_string(),
            addresses: vec![
                "fe80::1".parse().unwrap(),
                "192.168.1.100".parse().unwrap(),
            ],
            port: 7891,
            tier: PeerTier::Leaf,
            version: "1".to_string(),
            properties: HashMap::new(),
        };

        let peer = discovered.to_peer().unwrap();
        assert_eq!(peer.endpoint, "http://192.168.1.100:7891");
    }

    #[test]
    fn test_peer_tier_from_string() {
        let props: HashMap<String, String> =
            [("tier".to_string(), "cell_hub".to_string())].into();

        let tier = props
            .get("tier")
            .and_then(|t| match t.as_str() {
                "region_hub" => Some(PeerTier::RegionHub),
                "cell_hub" => Some(PeerTier::CellHub),
                "leaf" => Some(PeerTier::Leaf),
                _ => None,
            })
            .unwrap_or(PeerTier::Leaf);

        assert_eq!(tier, PeerTier::CellHub);
    }

    #[test]
    fn test_service_type_format() {
        // Verify service type follows DNS-SD conventions
        assert!(SERVICE_TYPE.starts_with('_'));
        assert!(SERVICE_TYPE.ends_with(".local."));
        assert!(SERVICE_TYPE.contains("._tcp."));
    }
}
