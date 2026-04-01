// conary-core/src/provenance/build.rs

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
                bytes.extend_from_slice(b":dna:");
                bytes.extend_from_slice(dna.as_bytes());
            }
            if let Some(ref ch) = dep.content_hash {
                bytes.extend_from_slice(b":ch:");
                bytes.extend_from_slice(ch.as_bytes());
            }
            bytes.push(0);
        }

        if let Some(ref attestation) = self.host_attestation {
            bytes.extend_from_slice(b"host:");
            bytes.extend_from_slice(attestation.arch.as_bytes());
            bytes.push(b':');
            bytes.extend_from_slice(attestation.kernel.as_bytes());
            if let Some(ref distro) = attestation.distro {
                bytes.extend_from_slice(b":distro:");
                bytes.extend_from_slice(distro.as_bytes());
            }
            if let Some(sb) = attestation.secure_boot {
                bytes.extend_from_slice(if sb { b":sb:true" } else { b":sb:false" });
            }
            if let Some(ref tpm) = attestation.tpm_quote {
                bytes.extend_from_slice(b":tpm:");
                bytes.extend_from_slice(tpm.as_bytes());
            }
            bytes.push(0);
        }

        if let Some(ref ts) = self.build_start {
            bytes.extend_from_slice(b"start:");
            bytes.extend_from_slice(ts.to_rfc3339().as_bytes());
            bytes.push(0);
        }
        if let Some(ref ts) = self.build_end {
            bytes.extend_from_slice(b"end:");
            bytes.extend_from_slice(ts.to_rfc3339().as_bytes());
            bytes.push(0);
        }
        if let Some(ref h) = self.build_log_hash {
            bytes.extend_from_slice(b"log:");
            bytes.extend_from_slice(h.as_bytes());
            bytes.push(0);
        }

        // Environment variables (sorted for determinism)
        let mut env: Vec<_> = self.build_env.iter().collect();
        env.sort();
        for (key, value) in env {
            bytes.extend_from_slice(b"env:");
            bytes.extend_from_slice(key.as_bytes());
            bytes.push(b'=');
            bytes.extend_from_slice(value.as_bytes());
            bytes.push(0);
        }

        // Reproducibility info
        if let Some(ref repro) = self.reproducibility {
            bytes.extend_from_slice(b"repro-consensus:");
            bytes.extend_from_slice(if repro.consensus { b"true" } else { b"false" });
            bytes.push(0);
            if let Some(ref hash) = repro.content_hash {
                bytes.extend_from_slice(b"repro-hash:");
                bytes.extend_from_slice(hash.as_bytes());
                bytes.push(0);
            }
            let mut verifiers: Vec<_> = repro.verified_by.iter().collect();
            verifiers.sort();
            for v in verifiers {
                bytes.extend_from_slice(b"repro-verifier:");
                bytes.extend_from_slice(v.as_bytes());
                bytes.push(0);
            }
            if !repro.differences.is_empty() {
                let mut diffs = repro.differences.clone();
                diffs.sort();
                for d in &diffs {
                    bytes.extend_from_slice(b"repro-diff:");
                    bytes.extend_from_slice(d.as_bytes());
                    bytes.push(0);
                }
            }
        }

        // Isolation level
        let isolation_str = match self.isolation_level {
            IsolationLevel::None => "none",
            IsolationLevel::Container => "container",
            IsolationLevel::Hermetic => "hermetic",
        };
        bytes.extend_from_slice(b"isolation:");
        bytes.extend_from_slice(isolation_str.as_bytes());
        bytes.push(0);

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
            tpm_quote: None, // Would require TPM integration
            secure_boot: check_secure_boot(),
            hostname: get_hostname(),
        }
    }
}

/// Get hostname from /etc/hostname (Linux) or the HOSTNAME environment variable
fn get_hostname() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        // Try /etc/hostname first (Linux convention)
        if let Ok(hostname) = std::fs::read_to_string("/etc/hostname") {
            let hostname = hostname.trim();
            if !hostname.is_empty() {
                return Some(hostname.to_string());
            }
        }
    }

    // Fall back to HOSTNAME env var
    std::env::var("HOSTNAME").ok()
}

/// Get kernel version string
///
/// On Linux this is read from `/proc/version`. On other platforms returns "unknown".
fn get_kernel_version() -> String {
    #[cfg(target_os = "linux")]
    {
        // /proc/version is a Linux-only virtual file
        if let Some(ver) = std::fs::read_to_string("/proc/version")
            .ok()
            .and_then(|v| v.split_whitespace().nth(2).map(|s| s.to_string()))
        {
            return ver;
        }
    }
    "unknown".to_string()
}

/// Get distro information from `/etc/os-release`
///
/// This file is a Linux standard (freedesktop.org). Returns `None` on other platforms.
fn get_distro_info() -> Option<String> {
    #[cfg(target_os = "linux")]
    return std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|content| {
            for line in content.lines() {
                if line.starts_with("PRETTY_NAME=") {
                    return Some(
                        line.trim_start_matches("PRETTY_NAME=")
                            .trim_matches('"')
                            .to_string(),
                    );
                }
            }
            None
        });
    #[cfg(not(target_os = "linux"))]
    None
}

/// Check if UEFI Secure Boot is enabled via the Linux EFI variable sysfs interface
///
/// Only meaningful on Linux; returns `None` on other platforms.
fn check_secure_boot() -> Option<bool> {
    #[cfg(target_os = "linux")]
    // The SecureBoot EFI variable is exposed under /sys/firmware/efi/efivars on Linux
    return std::fs::read(
        "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c",
    )
    .ok()
    .map(|data| data.last().map(|&b| b == 1).unwrap_or(false));
    #[cfg(not(target_os = "linux"))]
    None
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
            self.differences.push(builder_id.to_string());
        }
        let match_count = self.verified_by.len() - self.differences.len();
        self.consensus = match_count >= 2 && self.differences.is_empty();
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
        assert!(!repro.consensus); // Need at least 2

        repro.add_verifier("builder2", true);
        assert!(repro.consensus);
    }

    #[test]
    fn test_consensus_not_restored_after_failure() {
        let mut info = ReproducibilityInfo::new("abc123");
        info.add_verifier("builder1", true);
        assert!(!info.consensus);
        info.add_verifier("builder2", false);
        assert!(!info.consensus);
        info.add_verifier("builder3", true);
        assert!(!info.consensus); // must NOT be re-enabled after a mismatch
    }

    #[test]
    fn test_canonical_bytes() {
        use chrono::TimeZone;
        let fixed_ts = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

        let mut build1 = BuildProvenance::new("sha256:abc");
        build1.build_start = Some(fixed_ts);
        let mut build2 = BuildProvenance::new("sha256:abc");
        build2.build_start = Some(fixed_ts);

        assert_eq!(build1.canonical_bytes(), build2.canonical_bytes());
    }
}
