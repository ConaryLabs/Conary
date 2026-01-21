// src/capability/inference/mod.rs
//! Capability inference for converted packages
//!
//! This module provides automatic inference of security capabilities for packages
//! that don't have explicit declarations. It uses multiple strategies:
//!
//! 1. **Well-known profiles**: Pre-defined capabilities for common packages (nginx, postgresql, etc.)
//! 2. **Heuristics**: Rule-based inference from file paths, dependencies, and names
//! 3. **Config scanning**: Parse config files for network ports, filesystem paths
//! 4. **Binary analysis**: ELF inspection using goblin crate (optional, slower)
//!
//! # Example
//!
//! ```ignore
//! use conary::capability::inference::{infer_capabilities, InferenceOptions};
//!
//! let options = InferenceOptions::default();
//! let result = infer_capabilities(&package_files, &metadata, &options)?;
//!
//! println!("Inferred capabilities with {:?} confidence", result.confidence);
//! let declaration = result.to_declaration();
//! ```

mod binary;
mod cache;
mod confidence;
mod error;
mod heuristics;
mod wellknown;

pub use binary::BinaryAnalyzer;
pub use cache::{global_cache, CacheStats, InferenceCache};
pub use confidence::{Confidence, ConfidenceScore};
pub use error::InferenceError;
pub use heuristics::HeuristicInferrer;
pub use wellknown::WellKnownProfiles;

use crate::capability::{
    CapabilityDeclaration, FilesystemCapabilities, NetworkCapabilities, SyscallCapabilities,
};

/// Result type for inference operations
pub type InferenceResult<T> = Result<T, InferenceError>;

/// Options controlling the inference process
#[derive(Debug, Clone)]
pub struct InferenceOptions {
    /// Maximum inference tier to use (1-4)
    /// 1: Well-known profiles only
    /// 2: Heuristics (file paths, deps)
    /// 3: Config file scanning
    /// 4: Binary analysis
    pub max_tier: u8,

    /// Enable binary analysis (tier 4) - slower but more accurate
    pub enable_binary_analysis: bool,

    /// Minimum confidence to include in results
    pub min_confidence: Confidence,

    /// Maximum number of binaries to analyze (for performance)
    pub max_binaries_to_analyze: usize,

    /// Timeout for binary analysis in milliseconds
    pub binary_analysis_timeout_ms: u64,

    /// Use global cache for inference results
    pub use_cache: bool,
}

impl Default for InferenceOptions {
    fn default() -> Self {
        Self {
            max_tier: 2, // Heuristics by default
            enable_binary_analysis: false,
            min_confidence: Confidence::Low,
            max_binaries_to_analyze: 50,
            binary_analysis_timeout_ms: 5000,
            use_cache: true, // Caching enabled by default
        }
    }
}

impl InferenceOptions {
    /// Create options for full analysis (all tiers including binary)
    pub fn full_analysis() -> Self {
        Self {
            max_tier: 4,
            enable_binary_analysis: true,
            min_confidence: Confidence::Low,
            max_binaries_to_analyze: 100,
            binary_analysis_timeout_ms: 10000,
            use_cache: true,
        }
    }

    /// Create options for fast analysis (well-known + heuristics only)
    pub fn fast() -> Self {
        Self {
            max_tier: 2,
            enable_binary_analysis: false,
            min_confidence: Confidence::Medium,
            max_binaries_to_analyze: 0,
            binary_analysis_timeout_ms: 0,
            use_cache: true,
        }
    }

    /// Disable caching for this inference run
    pub fn without_cache(mut self) -> Self {
        self.use_cache = false;
        self
    }
}

/// Represents a file in a package for inference purposes
#[derive(Debug, Clone)]
pub struct PackageFile {
    /// Path where the file will be installed (e.g., "/usr/bin/nginx")
    pub path: String,

    /// File size in bytes
    pub size: u64,

    /// File mode/permissions
    pub mode: u32,

    /// Whether the file is executable
    pub is_executable: bool,

    /// Content hash for deduplication
    pub content_hash: Option<String>,

    /// Actual file content (for analysis) - may be None if not loaded
    pub content: Option<Vec<u8>>,
}

impl PackageFile {
    /// Create a new package file entry
    pub fn new(path: impl Into<String>) -> Self {
        let path = path.into();
        let is_executable = path.starts_with("/usr/bin")
            || path.starts_with("/usr/sbin")
            || path.starts_with("/bin")
            || path.starts_with("/sbin");

        Self {
            path,
            size: 0,
            mode: 0o644,
            is_executable,
            content_hash: None,
            content: None,
        }
    }

    /// Create with content for analysis
    pub fn with_content(path: impl Into<String>, content: Vec<u8>) -> Self {
        let path = path.into();
        let size = content.len() as u64;
        let is_executable = path.starts_with("/usr/bin")
            || path.starts_with("/usr/sbin")
            || path.starts_with("/bin")
            || path.starts_with("/sbin");

        Self {
            path,
            size,
            mode: 0o755,
            is_executable,
            content_hash: None,
            content: Some(content),
        }
    }

    /// Check if this is a config file
    pub fn is_config(&self) -> bool {
        self.path.starts_with("/etc/") || self.path.ends_with(".conf") || self.path.ends_with(".cfg")
    }

    /// Check if this is a systemd service file
    pub fn is_systemd_service(&self) -> bool {
        self.path.contains("/systemd/") && self.path.ends_with(".service")
    }

    /// Check if this is a library
    pub fn is_library(&self) -> bool {
        self.path.contains("/lib/") && (self.path.ends_with(".so") || self.path.contains(".so."))
    }
}

/// Metadata about a package for inference
#[derive(Debug, Clone, Default)]
pub struct PackageMetadataRef {
    /// Package name
    pub name: String,

    /// Package version
    pub version: String,

    /// Package description
    pub description: Option<String>,

    /// Dependencies
    pub dependencies: Vec<String>,

    /// Provides
    pub provides: Vec<String>,
}

/// Result of capability inference
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InferredCapabilities {
    /// Network capabilities
    pub network: InferredNetwork,

    /// Filesystem capabilities
    pub filesystem: InferredFilesystem,

    /// Syscall profile suggestion
    pub syscall_profile: Option<String>,

    /// Overall confidence in the inference
    pub confidence: ConfidenceScore,

    /// Which tier produced the result
    pub tier_used: u8,

    /// Human-readable rationale
    pub rationale: String,

    /// Source of inference (wellknown, heuristic, binary, etc.)
    pub source: InferenceSource,
}

impl Default for InferredCapabilities {
    fn default() -> Self {
        Self {
            network: InferredNetwork::default(),
            filesystem: InferredFilesystem::default(),
            syscall_profile: None,
            confidence: ConfidenceScore::new(Confidence::Low),
            tier_used: 0,
            rationale: String::new(),
            source: InferenceSource::None,
        }
    }
}

impl InferredCapabilities {
    /// Convert to a CapabilityDeclaration
    pub fn to_declaration(&self) -> CapabilityDeclaration {
        CapabilityDeclaration {
            version: 1,
            rationale: Some(format!(
                "Inferred via {} (confidence: {}). {}",
                self.source, self.confidence.primary, self.rationale
            )),
            network: NetworkCapabilities {
                outbound: self.network.outbound_ports.clone(),
                listen: self.network.listen_ports.clone(),
                none: self.network.no_network,
            },
            filesystem: FilesystemCapabilities {
                read: self.filesystem.read_paths.clone(),
                write: self.filesystem.write_paths.clone(),
                execute: self.filesystem.execute_paths.clone(),
                deny: Vec::new(),
            },
            syscalls: SyscallCapabilities {
                allow: Vec::new(),
                deny: Vec::new(),
                profile: self.syscall_profile.clone(),
            },
        }
    }

    /// Merge another inference result, preferring higher confidence
    pub fn merge(&mut self, other: &InferredCapabilities) {
        // Merge network
        if other.network.confidence > self.network.confidence {
            self.network = other.network.clone();
        } else {
            // Add any new ports
            for port in &other.network.listen_ports {
                if !self.network.listen_ports.contains(port) {
                    self.network.listen_ports.push(port.clone());
                }
            }
            for port in &other.network.outbound_ports {
                if !self.network.outbound_ports.contains(port) {
                    self.network.outbound_ports.push(port.clone());
                }
            }
        }

        // Merge filesystem
        for path in &other.filesystem.read_paths {
            if !self.filesystem.read_paths.contains(path) {
                self.filesystem.read_paths.push(path.clone());
            }
        }
        for path in &other.filesystem.write_paths {
            if !self.filesystem.write_paths.contains(path) {
                self.filesystem.write_paths.push(path.clone());
            }
        }

        // Use higher confidence syscall profile
        if other.syscall_profile.is_some()
            && (self.syscall_profile.is_none() || other.confidence.primary > self.confidence.primary)
        {
            self.syscall_profile = other.syscall_profile.clone();
        }

        // Update overall confidence (take the higher one for the dominant source)
        if other.confidence.primary > self.confidence.primary {
            self.confidence = other.confidence.clone();
            self.source = other.source;
            self.tier_used = other.tier_used;
        }
    }
}

/// Inferred network capabilities
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct InferredNetwork {
    /// Ports the package listens on
    pub listen_ports: Vec<String>,

    /// Outbound ports/destinations
    pub outbound_ports: Vec<String>,

    /// If true, package likely doesn't need network
    pub no_network: bool,

    /// Confidence in network inference
    pub confidence: Confidence,
}

/// Inferred filesystem capabilities
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct InferredFilesystem {
    /// Paths likely read by the package
    pub read_paths: Vec<String>,

    /// Paths likely written by the package
    pub write_paths: Vec<String>,

    /// Paths likely executed
    pub execute_paths: Vec<String>,

    /// Confidence in filesystem inference
    pub confidence: Confidence,
}

/// Source of the inference
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InferenceSource {
    /// No inference performed
    None,
    /// From well-known package profiles
    WellKnown,
    /// From heuristic rules
    Heuristic,
    /// From config file scanning
    ConfigScan,
    /// From binary analysis
    BinaryAnalysis,
    /// Combined from multiple sources
    Combined,
}

impl std::fmt::Display for InferenceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::WellKnown => write!(f, "well-known profile"),
            Self::Heuristic => write!(f, "heuristic analysis"),
            Self::ConfigScan => write!(f, "config file scan"),
            Self::BinaryAnalysis => write!(f, "binary analysis"),
            Self::Combined => write!(f, "combined analysis"),
        }
    }
}

/// Main entry point: infer capabilities for a package
pub fn infer_capabilities(
    files: &[PackageFile],
    metadata: &PackageMetadataRef,
    options: &InferenceOptions,
) -> InferenceResult<InferredCapabilities> {
    // Check cache first if enabled
    let cache_key = if options.use_cache {
        let file_hashes: Vec<&str> = files
            .iter()
            .filter_map(|f| f.content_hash.as_deref())
            .collect();
        let key = InferenceCache::compute_key(&metadata.name, &metadata.version, &file_hashes);

        if let Some(cached) = global_cache().get(&key) {
            tracing::debug!("Cache hit for inference of {}", metadata.name);
            return Ok(cached);
        }

        Some(key)
    } else {
        None
    };

    // Perform actual inference
    let result = infer_capabilities_uncached(files, metadata, options)?;

    // Store in cache if enabled
    if let Some(key) = cache_key {
        global_cache().put(key, result.clone());
    }

    Ok(result)
}

/// Internal inference implementation (no caching)
fn infer_capabilities_uncached(
    files: &[PackageFile],
    metadata: &PackageMetadataRef,
    options: &InferenceOptions,
) -> InferenceResult<InferredCapabilities> {
    let mut result = InferredCapabilities::default();

    // Tier 1: Check well-known profiles first
    if options.max_tier >= 1
        && let Some(wellknown) = WellKnownProfiles::lookup(&metadata.name)
    {
        result = wellknown;
        result.tier_used = 1;
        result.source = InferenceSource::WellKnown;

        // If well-known has high confidence, we can return early
        if result.confidence.primary >= Confidence::High {
            return Ok(result);
        }
    }

    // Tier 2: Apply heuristics
    if options.max_tier >= 2 {
        let heuristic_result = HeuristicInferrer::infer(files, metadata)?;
        result.merge(&heuristic_result);
        if result.tier_used == 0 {
            result.tier_used = 2;
        }
    }

    // Tier 3: Config file scanning (if files have content)
    if options.max_tier >= 3 {
        let config_files: Vec<_> = files.iter().filter(|f| f.is_config() && f.content.is_some()).collect();
        if !config_files.is_empty() {
            let config_result = scan_config_files(&config_files)?;
            result.merge(&config_result);
        }
    }

    // Tier 4: Binary analysis
    if options.max_tier >= 4 && options.enable_binary_analysis {
        let executables: Vec<_> = files
            .iter()
            .filter(|f| f.is_executable && f.content.is_some())
            .take(options.max_binaries_to_analyze)
            .collect();

        if !executables.is_empty() {
            let binary_result = BinaryAnalyzer::analyze_all(&executables)?;
            result.merge(&binary_result);
        }
    }

    // Set source to combined if we used multiple tiers
    if result.tier_used > 1 && result.source != InferenceSource::WellKnown {
        result.source = InferenceSource::Combined;
    }

    // Generate rationale
    if result.rationale.is_empty() {
        result.rationale = generate_rationale(&result, metadata);
    }

    Ok(result)
}

/// Scan config files for capability hints
fn scan_config_files(files: &[&PackageFile]) -> InferenceResult<InferredCapabilities> {
    use std::sync::LazyLock;

    static PORT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)(?:listen|port|bind)[^\d]*(\d{1,5})").unwrap()
    });

    static PATH_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?:file|path|dir|log)[^\s"']*[\s=:]+["']?(/[a-zA-Z0-9/_.-]+)"#).unwrap()
    });

    let mut result = InferredCapabilities {
        source: InferenceSource::ConfigScan,
        tier_used: 3,
        ..Default::default()
    };

    for file in files {
        if let Some(ref content) = file.content
            && let Ok(text) = std::str::from_utf8(content)
        {
            // Look for port patterns
            for cap in PORT_RE.captures_iter(text) {
                if let Some(port) = cap.get(1) {
                    let port_str = port.as_str().to_string();
                    if !result.network.listen_ports.contains(&port_str) {
                        result.network.listen_ports.push(port_str);
                    }
                }
            }

            // Look for path patterns
            for cap in PATH_RE.captures_iter(text) {
                if let Some(path) = cap.get(1) {
                    let path_str = path.as_str().to_string();
                    if path_str.contains("log")
                        || path_str.contains("cache")
                        || path_str.contains("tmp")
                    {
                        if !result.filesystem.write_paths.contains(&path_str) {
                            result.filesystem.write_paths.push(path_str);
                        }
                    } else if !result.filesystem.read_paths.contains(&path_str) {
                        result.filesystem.read_paths.push(path_str);
                    }
                }
            }
        }
    }

    result.network.confidence = if result.network.listen_ports.is_empty() {
        Confidence::Low
    } else {
        Confidence::Medium
    };

    result.confidence = ConfidenceScore::new(Confidence::Medium);

    Ok(result)
}

/// Generate a human-readable rationale for the inference
fn generate_rationale(result: &InferredCapabilities, metadata: &PackageMetadataRef) -> String {
    let mut parts = Vec::new();

    if !result.network.listen_ports.is_empty() {
        parts.push(format!(
            "Listens on port(s): {}",
            result.network.listen_ports.join(", ")
        ));
    }

    if !result.network.outbound_ports.is_empty() {
        parts.push(format!(
            "Makes outbound connections to: {}",
            result.network.outbound_ports.join(", ")
        ));
    }

    if !result.filesystem.write_paths.is_empty() {
        parts.push(format!(
            "Writes to: {}",
            result.filesystem.write_paths.join(", ")
        ));
    }

    if let Some(ref profile) = result.syscall_profile {
        parts.push(format!("Syscall profile: {}", profile));
    }

    if parts.is_empty() {
        format!(
            "Package '{}' analyzed with {} confidence",
            metadata.name, result.confidence.primary
        )
    } else {
        parts.join(". ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_file_detection() {
        let bin = PackageFile::new("/usr/bin/nginx");
        assert!(bin.is_executable);

        let conf = PackageFile::new("/etc/nginx/nginx.conf");
        assert!(conf.is_config());
        assert!(!conf.is_executable);

        let service = PackageFile::new("/usr/lib/systemd/system/nginx.service");
        assert!(service.is_systemd_service());

        let lib = PackageFile::new("/usr/lib/libnginx.so.1");
        assert!(lib.is_library());
    }

    #[test]
    fn test_inference_options_default() {
        let opts = InferenceOptions::default();
        assert_eq!(opts.max_tier, 2);
        assert!(!opts.enable_binary_analysis);
    }

    #[test]
    fn test_inference_options_full() {
        let opts = InferenceOptions::full_analysis();
        assert_eq!(opts.max_tier, 4);
        assert!(opts.enable_binary_analysis);
    }

    #[test]
    fn test_inferred_to_declaration() {
        let mut inferred = InferredCapabilities::default();
        inferred.network.listen_ports.push("80".to_string());
        inferred.network.listen_ports.push("443".to_string());
        inferred.filesystem.write_paths.push("/var/log/nginx".to_string());
        inferred.syscall_profile = Some("network-server".to_string());

        let decl = inferred.to_declaration();
        assert_eq!(decl.network.listen, vec!["80", "443"]);
        assert_eq!(decl.filesystem.write, vec!["/var/log/nginx"]);
        assert_eq!(decl.syscalls.profile, Some("network-server".to_string()));
    }

    // =========================================================================
    // Inference Merging Tests (Task 537)
    // =========================================================================

    #[test]
    fn test_merge_prefers_higher_confidence_network() {
        let mut base = InferredCapabilities {
            network: InferredNetwork {
                listen_ports: vec!["80".to_string()],
                confidence: Confidence::Low,
                ..Default::default()
            },
            confidence: ConfidenceScore::new(Confidence::Low),
            ..Default::default()
        };

        let other = InferredCapabilities {
            network: InferredNetwork {
                listen_ports: vec!["443".to_string()],
                confidence: Confidence::High,
                ..Default::default()
            },
            confidence: ConfidenceScore::new(Confidence::High),
            ..Default::default()
        };

        base.merge(&other);

        // Higher confidence network replaces lower
        assert_eq!(base.network.listen_ports, vec!["443"]);
        assert_eq!(base.network.confidence, Confidence::High);
    }

    #[test]
    fn test_merge_combines_ports_when_equal_confidence() {
        let mut base = InferredCapabilities {
            network: InferredNetwork {
                listen_ports: vec!["80".to_string()],
                outbound_ports: vec!["443".to_string()],
                confidence: Confidence::Medium,
                ..Default::default()
            },
            confidence: ConfidenceScore::new(Confidence::Medium),
            ..Default::default()
        };

        let other = InferredCapabilities {
            network: InferredNetwork {
                listen_ports: vec!["8080".to_string()],
                outbound_ports: vec!["5432".to_string()],
                confidence: Confidence::Low, // Lower, so ports are added
                ..Default::default()
            },
            confidence: ConfidenceScore::new(Confidence::Low),
            ..Default::default()
        };

        base.merge(&other);

        // Ports are combined since base confidence is higher
        assert!(base.network.listen_ports.contains(&"80".to_string()));
        assert!(base.network.listen_ports.contains(&"8080".to_string()));
        assert!(base.network.outbound_ports.contains(&"443".to_string()));
        assert!(base.network.outbound_ports.contains(&"5432".to_string()));
    }

    #[test]
    fn test_merge_combines_filesystem_paths() {
        let mut base = InferredCapabilities {
            filesystem: InferredFilesystem {
                read_paths: vec!["/etc/nginx".to_string()],
                write_paths: vec!["/var/log/nginx".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        let other = InferredCapabilities {
            filesystem: InferredFilesystem {
                read_paths: vec!["/etc/ssl".to_string()],
                write_paths: vec!["/var/cache/nginx".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        base.merge(&other);

        assert_eq!(base.filesystem.read_paths.len(), 2);
        assert!(base.filesystem.read_paths.contains(&"/etc/nginx".to_string()));
        assert!(base.filesystem.read_paths.contains(&"/etc/ssl".to_string()));
        assert_eq!(base.filesystem.write_paths.len(), 2);
    }

    #[test]
    fn test_merge_no_duplicate_paths() {
        let mut base = InferredCapabilities {
            filesystem: InferredFilesystem {
                read_paths: vec!["/etc/nginx".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        let other = InferredCapabilities {
            filesystem: InferredFilesystem {
                read_paths: vec!["/etc/nginx".to_string()], // Same path
                ..Default::default()
            },
            ..Default::default()
        };

        base.merge(&other);

        // Should not duplicate
        assert_eq!(base.filesystem.read_paths.len(), 1);
    }

    #[test]
    fn test_merge_syscall_profile_prefers_higher_confidence() {
        let mut base = InferredCapabilities {
            syscall_profile: Some("basic".to_string()),
            confidence: ConfidenceScore::new(Confidence::Low),
            ..Default::default()
        };

        let other = InferredCapabilities {
            syscall_profile: Some("network-server".to_string()),
            confidence: ConfidenceScore::new(Confidence::High),
            ..Default::default()
        };

        base.merge(&other);

        assert_eq!(base.syscall_profile, Some("network-server".to_string()));
    }

    #[test]
    fn test_merge_keeps_syscall_profile_if_other_is_none() {
        let mut base = InferredCapabilities {
            syscall_profile: Some("system-daemon".to_string()),
            confidence: ConfidenceScore::new(Confidence::Medium),
            ..Default::default()
        };

        let other = InferredCapabilities {
            syscall_profile: None,
            confidence: ConfidenceScore::new(Confidence::High),
            ..Default::default()
        };

        base.merge(&other);

        // Keep existing since other has None
        assert_eq!(base.syscall_profile, Some("system-daemon".to_string()));
    }

    // =========================================================================
    // Multi-tier Inference Tests
    // =========================================================================

    #[test]
    fn test_multi_tier_inference_wellknown_plus_heuristic() {
        // Simulate nginx inference which would hit wellknown first
        let files = vec![
            PackageFile::new("/usr/sbin/nginx"),
            PackageFile::new("/etc/nginx/nginx.conf"),
            PackageFile::new("/var/log/nginx/access.log"),
        ];

        let metadata = PackageMetadataRef {
            name: "nginx".to_string(),
            version: "1.24.0".to_string(),
            ..Default::default()
        };

        let options = InferenceOptions {
            max_tier: 2,
            use_cache: false,
            ..Default::default()
        };

        let result = infer_capabilities(&files, &metadata, &options).unwrap();

        // Should use wellknown (tier 1) since nginx is a known package
        assert_eq!(result.tier_used, 1);
        assert_eq!(result.source, InferenceSource::WellKnown);
        // nginx profile should have network capabilities
        assert!(!result.network.no_network);
    }

    #[test]
    fn test_multi_tier_inference_heuristic_only() {
        // Unknown package, should fall through to heuristics
        let files = vec![
            PackageFile::new("/usr/sbin/myunknownservice"),
            PackageFile::new("/etc/myunknownservice/config.conf"),
        ];

        let metadata = PackageMetadataRef {
            name: "myunknownservice".to_string(),
            version: "1.0.0".to_string(),
            dependencies: vec!["libssl3".to_string()],
            ..Default::default()
        };

        let options = InferenceOptions {
            max_tier: 2,
            use_cache: false,
            ..Default::default()
        };

        let result = infer_capabilities(&files, &metadata, &options).unwrap();

        // Should use heuristics since package is unknown
        assert_eq!(result.tier_used, 2);
        // Has sbin executable and ssl dependency
        assert!(result.syscall_profile.is_some());
    }

    #[test]
    fn test_inference_with_config_scanning() {
        let config_content = b"listen 8080\nlog_path /var/log/myapp/app.log";
        let files = vec![
            PackageFile::new("/usr/bin/myapp"),
            PackageFile::with_content("/etc/myapp/config.conf", config_content.to_vec()),
        ];

        let metadata = PackageMetadataRef {
            name: "myapp".to_string(),
            version: "1.0.0".to_string(),
            ..Default::default()
        };

        let options = InferenceOptions {
            max_tier: 3, // Enable config scanning
            use_cache: false,
            ..Default::default()
        };

        let result = infer_capabilities(&files, &metadata, &options).unwrap();

        // Should extract port from config
        assert!(result.network.listen_ports.contains(&"8080".to_string()));
    }

    #[test]
    fn test_inference_tier_limit() {
        let files = vec![PackageFile::new("/usr/bin/test")];

        let metadata = PackageMetadataRef {
            name: "testpkg".to_string(),
            version: "1.0.0".to_string(),
            ..Default::default()
        };

        // Limit to tier 1 only (wellknown)
        let options = InferenceOptions {
            max_tier: 1,
            use_cache: false,
            ..Default::default()
        };

        let result = infer_capabilities(&files, &metadata, &options).unwrap();

        // Unknown package with tier 1 only = no inference
        assert_eq!(result.tier_used, 0);
    }

    // =========================================================================
    // Edge Case Tests
    // =========================================================================

    #[test]
    fn test_empty_files_inference() {
        let files: Vec<PackageFile> = vec![];

        let metadata = PackageMetadataRef {
            name: "empty-pkg".to_string(),
            version: "1.0.0".to_string(),
            ..Default::default()
        };

        let options = InferenceOptions {
            use_cache: false,
            ..Default::default()
        };

        let result = infer_capabilities(&files, &metadata, &options).unwrap();

        // Should succeed with default/empty capabilities
        assert!(result.network.listen_ports.is_empty());
        assert!(result.filesystem.read_paths.is_empty());
    }

    #[test]
    fn test_inference_source_display() {
        assert_eq!(format!("{}", InferenceSource::WellKnown), "well-known profile");
        assert_eq!(format!("{}", InferenceSource::Heuristic), "heuristic analysis");
        assert_eq!(format!("{}", InferenceSource::BinaryAnalysis), "binary analysis");
        assert_eq!(format!("{}", InferenceSource::Combined), "combined analysis");
    }

    #[test]
    fn test_inferred_network_default() {
        let network = InferredNetwork::default();
        assert!(network.listen_ports.is_empty());
        assert!(network.outbound_ports.is_empty());
        assert!(!network.no_network); // Default is false (might need network)
    }

    #[test]
    fn test_generate_rationale_with_ports() {
        let result = InferredCapabilities {
            network: InferredNetwork {
                listen_ports: vec!["80".to_string(), "443".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        let metadata = PackageMetadataRef {
            name: "webserver".to_string(),
            ..Default::default()
        };

        let rationale = generate_rationale(&result, &metadata);
        assert!(rationale.contains("80"));
        assert!(rationale.contains("443"));
    }

    #[test]
    fn test_generate_rationale_empty() {
        let result = InferredCapabilities::default();
        let metadata = PackageMetadataRef {
            name: "minimal".to_string(),
            ..Default::default()
        };

        let rationale = generate_rationale(&result, &metadata);
        assert!(rationale.contains("minimal"));
        assert!(rationale.contains("confidence"));
    }
}
