// src/provenance/build.rs

//! Build layer provenance - how the package was built

use super::CanonicalBytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Build layer provenance information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuildProvenance {
    /// Hash of the recipe file used to build
    #[serde(default)]
    pub recipe_hash: Option<String>,

    /// Build dependencies with their DNA hashes (recursive provenance)
    #[serde(default)]
    pub build_deps: Vec<BuildDependency>,

    /// Host attestation (build machine info)
    #[serde(default)]
    pub host_attestation: Option<HostAttestation>,

    /// When the build started
    #[serde(default)]
    pub build_start: Option<DateTime<Utc>>,

    /// When the build completed
    #[serde(default)]
    pub build_end: Option<DateTime<Utc>>,

    /// Hash of the build log
    #[serde(default)]
    pub build_log_hash: Option<String>,

    /// Reproducibility verification info
    #[serde(default)]
    pub reproducibility: Option<ReproducibilityInfo>,

    /// Build isolation level used
    #[serde(default)]
    pub isolation_level: IsolationLevel,

    /// Environment variables that affected the build
    #[serde(default)]
    pub build_env: Vec<(String, String)>,
}

impl BuildProvenance {
    /// Create new build provenance with recipe hash
    pub fn new(recipe_hash: &str) -> Self {
        Self {
            recipe_hash: Some(recipe_hash.to_string()),
            build_start: Some(Utc::now()),
            ..Default::default()
        }
    }

    /// Mark build as complete
    pub fn complete(&mut self) {
        self.build_end = Some(Utc::now());
    }

    /// Add a build dependency with its DNA hash
    pub fn add_dependency(&mut self, dep: BuildDependency) {
        self.build_deps.push(dep);
    }

    /// Set host attestation
    pub fn set_host_attestation(&mut self, attestation: HostAttestation) {
        self.host_attestation = Some(attestation);
    }

    /// Calculate build duration in seconds
    pub fn duration_secs(&self) -> Option<i64> {
        match (&self.build_start, &self.build_end) {
            (Some(start), Some(end)) => Some((*end - *start).num_seconds()),
            _ => None,
        }
    }
}

impl CanonicalBytes for BuildProvenance {
    fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        if let Some(ref hash) = self.recipe_hash {
            bytes.extend_from_slice(b"recipe:");
            bytes.extend_from_slice(hash.as_bytes());
            bytes.push(0);
        }

        // Sort deps by name for determinism
        let mut deps: Vec<_> = self.build_deps.iter().collect();
        deps.sort_by(|a, b| a.name.cmp(&b.name));

        for dep in deps {
            bytes.extend_from_slice(b"dep:");
            bytes.extend_from_slice(dep.name.as_bytes());
            bytes.push(b':');
            bytes.extend_from_slice(dep.version.as_bytes());
            if let Some(ref dna) = dep.dna_hash {
                bytes.push(b':');
                bytes.extend_from_slice(dna.as_bytes());
            }
            bytes.push(0);
        }

        if let Some(ref attestation) = self.host_attestation {
            bytes.extend_from_slice(b"host:");
            bytes.extend_from_slice(attestation.arch.as_bytes());
            bytes.push(b':');
            bytes.extend_from_slice(attestation.kernel.as_bytes());
            bytes.push(0);
        }

        bytes
    }
}

/// A build dependency with full provenance tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildDependency {
    /// Package name
    pub name: String,

    /// Package version
    pub version: String,

    /// DNA hash of the dependency (recursive provenance)
    #[serde(default)]
    pub dna_hash: Option<String>,

    /// Content hash of installed files
    #[serde(default)]
    pub content_hash: Option<String>,
}

impl BuildDependency {
    /// Create a new build dependency
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            dna_hash: None,
            content_hash: None,
        }
    }

    /// Add DNA hash for full provenance chain
    pub fn with_dna(mut self, dna_hash: &str) -> Self {
        self.dna_hash = Some(dna_hash.to_string());
        self
    }

    /// Add content hash
    pub fn with_content_hash(mut self, content_hash: &str) -> Self {
        self.content_hash = Some(content_hash.to_string());
        self
    }
}

/// Host attestation - information about the build machine
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostAttestation {
    /// CPU architecture
    pub arch: String,

    /// Kernel version
    pub kernel: String,

    /// OS distribution
    #[serde(default)]
    pub distro: Option<String>,

    /// TPM quote for hardware attestation (if available)
    #[serde(default)]
    pub tpm_quote: Option<String>,

    /// Secure boot status
    #[serde(default)]
    pub secure_boot: Option<bool>,

    /// Hostname (for audit, not used in hash)
    #[serde(default)]
    pub hostname: Option<String>,
}

impl HostAttestation {
    /// Create attestation from current system
    pub fn from_current_system() -> Self {
        Self {
            arch: std::env::consts::ARCH.to_string(),
            kernel: get_kernel_version(),
            distro: get_distro_info(),
            tpm_quote: None,  // Would require TPM integration
            secure_boot: check_secure_boot(),
            hostname: get_hostname(),
        }
    }
}

/// Get hostname from /etc/hostname or via gethostname syscall
fn get_hostname() -> Option<String> {
    // Try /etc/hostname first
    if let Ok(hostname) = std::fs::read_to_string("/etc/hostname") {
        let hostname = hostname.trim();
        if !hostname.is_empty() {
            return Some(hostname.to_string());
        }
    }

    // Fall back to HOSTNAME env var
    std::env::var("HOSTNAME").ok()
}

/// Get kernel version from uname
fn get_kernel_version() -> String {
    // Try to read from /proc/version
    std::fs::read_to_string("/proc/version")
        .ok()
        .and_then(|v| v.split_whitespace().nth(2).map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Get distro information
fn get_distro_info() -> Option<String> {
    // Try /etc/os-release
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|content| {
            for line in content.lines() {
                if line.starts_with("PRETTY_NAME=") {
                    return Some(line.trim_start_matches("PRETTY_NAME=")
                        .trim_matches('"')
                        .to_string());
                }
            }
            None
        })
}

/// Check if secure boot is enabled
fn check_secure_boot() -> Option<bool> {
    // Check EFI variable
    std::fs::read("/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c")
        .ok()
        .map(|data| data.last().map(|&b| b == 1).unwrap_or(false))
}

/// Reproducibility verification information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReproducibilityInfo {
    /// List of builders who verified reproducibility
    pub verified_by: Vec<String>,

    /// Whether consensus was reached
    pub consensus: bool,

    /// Hash of the content (should match across builders)
    #[serde(default)]
    pub content_hash: Option<String>,

    /// Any noted differences between builds
    #[serde(default)]
    pub differences: Vec<String>,
}

impl ReproducibilityInfo {
    /// Create new reproducibility info
    pub fn new(content_hash: &str) -> Self {
        Self {
            verified_by: Vec::new(),
            consensus: false,
            content_hash: Some(content_hash.to_string()),
            differences: Vec::new(),
        }
    }

    /// Add a verifier
    pub fn add_verifier(&mut self, builder_id: &str, matches: bool) {
        self.verified_by.push(builder_id.to_string());
        if !matches {
            self.consensus = false;
        } else if self.verified_by.len() >= 2 {
            // Consensus requires at least 2 matching builders
            self.consensus = true;
        }
    }
}

/// Build isolation level
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IsolationLevel {
    /// No isolation (unsafe)
    None,

    /// Container isolation with network blocked during build
    #[default]
    Container,

    /// Hermetic isolation (no host mounts, maximum reproducibility)
    Hermetic,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_provenance() {
        let mut build = BuildProvenance::new("sha256:recipe123");
        build.add_dependency(BuildDependency::new("gcc", "14.2.0").with_dna("sha256:gcc_dna"));
        build.complete();

        assert!(build.duration_secs().is_some());
        assert_eq!(build.build_deps.len(), 1);
    }

    #[test]
    fn test_host_attestation() {
        let attestation = HostAttestation::from_current_system();
        assert!(!attestation.arch.is_empty());
        assert!(!attestation.kernel.is_empty());
    }

    #[test]
    fn test_reproducibility_consensus() {
        let mut repro = ReproducibilityInfo::new("sha256:content");

        repro.add_verifier("builder1", true);
        assert!(!repro.consensus);  // Need at least 2

        repro.add_verifier("builder2", true);
        assert!(repro.consensus);
    }

    #[test]
    fn test_canonical_bytes() {
        let build1 = BuildProvenance::new("sha256:abc");
        let build2 = BuildProvenance::new("sha256:abc");

        assert_eq!(build1.canonical_bytes(), build2.canonical_bytes());
    }
}
