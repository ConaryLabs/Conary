// conary-core/src/derivation/manifest.rs

//! User-facing system manifest TOML parser for the CAS-layered bootstrap.
//!
//! A `SystemManifest` declares the desired system state: target architecture,
//! seed image, package selection, kernel configuration, and optional
//! customization layers. It is the primary input to the bootstrap pipeline,
//! replacing ad-hoc configuration with a single declarative TOML file.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Errors that can occur when loading or parsing a system manifest.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// I/O error reading the manifest file.
    #[error("failed to read manifest: {0}")]
    Io(String),
    /// TOML parse or schema error.
    #[error("failed to parse manifest: {0}")]
    Parse(String),
}

/// Top-level system manifest describing the desired bootstrap output.
///
/// A manifest is the single source of truth for a bootstrap build: it names the
/// system, selects a seed, lists packages, and optionally configures the kernel,
/// substituters, integrity settings, and customization layers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemManifest {
    /// Core system identity (name, target triple).
    pub system: SystemSection,
    /// Seed image reference used as the build environment baseline.
    pub seed: SeedReference,
    /// Package include/exclude lists.
    pub packages: PackageSelection,
    /// Optional kernel configuration.
    pub kernel: Option<KernelSection>,
    /// Optional customization layers applied after package installation.
    pub customization: Option<CustomizationSection>,
    /// Optional binary cache / substituter configuration.
    pub substituters: Option<SubstituterSection>,
    /// Optional integrity verification settings.
    pub integrity: Option<IntegritySection>,
}

/// Core system identity fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemSection {
    /// Human-readable system name (e.g. "conaryos-workstation").
    pub name: String,
    /// Target triple (e.g. "x86_64-conary-linux-gnu").
    pub target: String,
}

/// Reference to the seed image that bootstraps the build environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedReference {
    /// Seed source identifier (path, URL, or CAS hash).
    pub source: String,
}

/// Package selection for the system image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSelection {
    /// Packages to include in the system image.
    pub include: Vec<String>,
    /// Packages to explicitly exclude (overrides transitive pulls).
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Kernel build configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelSection {
    /// Path or identifier for the kernel config (e.g. "defconfig", path to `.config`).
    pub config: String,
}

/// Post-install customization layers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomizationSection {
    /// Ordered list of overlay layer paths or identifiers.
    #[serde(default)]
    pub layers: Vec<String>,
}

/// Binary cache / substituter configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubstituterSection {
    /// Substituter endpoint URLs, tried in order.
    pub sources: Vec<String>,
    /// Trust policy for substituted artifacts (default: "check").
    #[serde(default = "default_trust")]
    pub trust: String,
}

/// Integrity verification settings for the final image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegritySection {
    /// Enable fs-verity on CAS objects.
    #[serde(default)]
    pub fsverity: bool,
    /// Enable EROFS digest verification.
    #[serde(default)]
    pub erofs_digest: bool,
}

/// Default trust policy for substituters.
fn default_trust() -> String {
    "check".to_owned()
}

impl SystemManifest {
    /// Load a system manifest from a TOML file on disk.
    ///
    /// # Errors
    ///
    /// Returns `ManifestError::Io` if the file cannot be read, or
    /// `ManifestError::Parse` if the contents are not valid TOML or do not
    /// match the expected schema.
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let content =
            std::fs::read_to_string(path).map_err(|e| ManifestError::Io(e.to_string()))?;
        Self::parse(&content)
    }

    /// Parse a system manifest from a TOML string.
    ///
    /// # Errors
    ///
    /// Returns `ManifestError::Parse` if the string is not valid TOML or does
    /// not match the expected schema.
    pub fn parse(content: &str) -> Result<Self, ManifestError> {
        toml::from_str(content).map_err(|e| ManifestError::Parse(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_MANIFEST: &str = r#"
[system]
name = "conaryos-minimal"
target = "x86_64-conary-linux-gnu"

[seed]
source = "https://packages.conary.io/seeds/stage0-x86_64.erofs"

[packages]
include = ["glibc", "coreutils", "bash"]
"#;

    const FULL_MANIFEST: &str = r#"
[system]
name = "conaryos-workstation"
target = "x86_64-conary-linux-gnu"

[seed]
source = "cas:sha256:abc123def456"

[packages]
include = ["glibc", "coreutils", "bash", "systemd", "linux"]
exclude = ["telnet", "ftp"]

[kernel]
config = "defconfig"

[customization]
layers = ["/etc/conary/overlays/branding", "/etc/conary/overlays/network"]

[substituters]
sources = ["https://cache.conary.io", "https://cache-eu.conary.io"]
trust = "verify"

[integrity]
fsverity = true
erofs_digest = true
"#;

    #[test]
    fn parse_minimal_manifest() {
        let manifest = SystemManifest::parse(MINIMAL_MANIFEST).expect("should parse");

        assert_eq!(manifest.system.name, "conaryos-minimal");
        assert_eq!(manifest.system.target, "x86_64-conary-linux-gnu");
        assert_eq!(
            manifest.seed.source,
            "https://packages.conary.io/seeds/stage0-x86_64.erofs"
        );
        assert_eq!(
            manifest.packages.include,
            vec!["glibc", "coreutils", "bash"]
        );
        assert!(manifest.packages.exclude.is_empty());
        assert!(manifest.kernel.is_none());
        assert!(manifest.customization.is_none());
        assert!(manifest.substituters.is_none());
        assert!(manifest.integrity.is_none());
    }

    #[test]
    fn parse_full_manifest() {
        let manifest = SystemManifest::parse(FULL_MANIFEST).expect("should parse");

        assert_eq!(manifest.system.name, "conaryos-workstation");
        assert_eq!(manifest.packages.include.len(), 5);
        assert_eq!(manifest.packages.exclude, vec!["telnet", "ftp"]);

        let kernel = manifest.kernel.expect("kernel section present");
        assert_eq!(kernel.config, "defconfig");

        let customization = manifest.customization.expect("customization present");
        assert_eq!(customization.layers.len(), 2);

        let substituters = manifest.substituters.expect("substituters present");
        assert_eq!(substituters.sources.len(), 2);
        assert_eq!(substituters.trust, "verify");

        let integrity = manifest.integrity.expect("integrity present");
        assert!(integrity.fsverity);
        assert!(integrity.erofs_digest);
    }

    #[test]
    fn roundtrip_serialization() {
        let manifest = SystemManifest::parse(FULL_MANIFEST).expect("should parse");
        let serialized = toml::to_string(&manifest).expect("should serialize");
        let roundtripped = SystemManifest::parse(&serialized).expect("should re-parse");
        assert_eq!(manifest, roundtripped);
    }

    #[test]
    fn invalid_toml_returns_parse_error() {
        let result = SystemManifest::parse("this is not valid toml {{{}}}");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ManifestError::Parse(_)),
            "expected Parse error, got: {err}"
        );
    }

    #[test]
    fn missing_required_section_returns_parse_error() {
        // Missing [seed] and [packages] sections.
        let incomplete = r#"
[system]
name = "test"
target = "x86_64"
"#;
        let result = SystemManifest::parse(incomplete);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ManifestError::Parse(_)));
    }

    #[test]
    fn substituter_trust_defaults_to_check() {
        let toml_str = r#"
[system]
name = "test"
target = "x86_64"

[seed]
source = "local:seed.erofs"

[packages]
include = ["glibc"]

[substituters]
sources = ["https://cache.conary.io"]
"#;
        let manifest = SystemManifest::parse(toml_str).expect("should parse");
        let substituters = manifest.substituters.expect("substituters present");
        assert_eq!(substituters.trust, "check");
    }

    #[test]
    fn integrity_defaults_to_false() {
        let toml_str = r#"
[system]
name = "test"
target = "x86_64"

[seed]
source = "local:seed.erofs"

[packages]
include = ["glibc"]

[integrity]
"#;
        let manifest = SystemManifest::parse(toml_str).expect("should parse");
        let integrity = manifest.integrity.expect("integrity present");
        assert!(!integrity.fsverity);
        assert!(!integrity.erofs_digest);
    }

    #[test]
    fn parse_conaryos_manifest() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = manifest_dir.parent().unwrap().join("conaryos.toml");
        let content =
            std::fs::read_to_string(&path).expect("conaryos.toml not found at workspace root");
        let manifest = SystemManifest::parse(&content).expect("conaryos.toml should parse");
        assert_eq!(manifest.system.name, "conaryos-base");
        assert_eq!(manifest.system.target, "x86_64-conary-linux-gnu");
        assert!(
            manifest.packages.include.len() >= 85,
            "expected 85+ packages"
        );
        assert!(manifest.packages.include.contains(&"glibc".to_string()));
        assert!(manifest.packages.include.contains(&"linux-pam".to_string()));
        assert!(manifest.kernel.is_some());
    }
}
