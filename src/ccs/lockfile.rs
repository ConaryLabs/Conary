// src/ccs/lockfile.rs

//! CCS Lockfile for reproducible builds
//!
//! The lockfile (`ccs.lock`) captures the exact resolved state of all dependencies
//! at a point in time, enabling reproducible builds across different machines and times.
//!
//! # Format
//!
//! The lockfile is TOML-based and includes:
//! - Lockfile metadata (version, timestamp, generator)
//! - Resolved dependencies with exact versions and content hashes
//! - Build dependencies (host tools)
//! - Runtime dependencies (target packages)
//!
//! # Example
//!
//! ```toml
//! [metadata]
//! version = 1
//! generated = "2024-01-15T10:30:00Z"
//! generator = "conary 0.2.0"
//! package = "myapp"
//! package_version = "1.0.0"
//!
//! [[dependencies]]
//! name = "openssl"
//! version = "3.1.4"
//! content_hash = "sha256:abc123..."
//! source = "https://repo.example.com"
//! kind = "runtime"
//!
//! [[dependencies]]
//! name = "gcc"
//! version = "13.2.0"
//! content_hash = "sha256:def456..."
//! kind = "build"
//! ```

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use thiserror::Error;

/// Current lockfile format version
pub const LOCKFILE_VERSION: u32 = 1;

/// Default lockfile name
pub const LOCKFILE_NAME: &str = "ccs.lock";

#[derive(Error, Debug)]
pub enum LockfileError {
    #[error("Failed to read lockfile: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("Failed to parse lockfile: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("Failed to serialize lockfile: {0}")]
    SerializeError(#[from] toml::ser::Error),

    #[error("Lockfile version mismatch: expected {expected}, found {found}")]
    VersionMismatch { expected: u32, found: u32 },

    #[error("Lockfile validation failed: {0}")]
    ValidationError(String),

    #[error("Dependency mismatch: {name} expected {expected}, found {found}")]
    DependencyMismatch {
        name: String,
        expected: String,
        found: String,
    },

    #[error("Missing dependency in lockfile: {0}")]
    MissingDependency(String),
}

/// Lockfile root structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    /// Lockfile metadata
    pub metadata: LockfileMetadata,

    /// All resolved dependencies
    #[serde(default)]
    pub dependencies: Vec<LockedDependency>,

    /// Platform-specific dependency overrides
    #[serde(default)]
    pub platform_deps: HashMap<String, Vec<LockedDependency>>,

    /// Hash of the ccs.toml that generated this lockfile
    #[serde(default)]
    pub manifest_hash: Option<String>,
}

/// Lockfile metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileMetadata {
    /// Lockfile format version
    pub version: u32,

    /// When the lockfile was generated (ISO 8601)
    pub generated: String,

    /// Tool that generated the lockfile
    pub generator: String,

    /// Package name this lockfile is for
    pub package: String,

    /// Package version
    pub package_version: String,

    /// Optional comment/description
    #[serde(default)]
    pub comment: Option<String>,
}

/// A locked (resolved) dependency
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedDependency {
    /// Dependency name
    pub name: String,

    /// Exact resolved version
    pub version: String,

    /// Content hash of the package (sha256:...)
    pub content_hash: String,

    /// Where the dependency was resolved from
    #[serde(default)]
    pub source: Option<String>,

    /// Repository/label this came from
    #[serde(default)]
    pub label: Option<String>,

    /// Kind of dependency: runtime, build, optional
    #[serde(default = "default_dep_kind")]
    pub kind: DependencyKind,

    /// Sub-dependencies (for tree representation)
    #[serde(default)]
    pub requires: Vec<String>,

    /// Package DNA hash for provenance tracking
    #[serde(default)]
    pub dna_hash: Option<String>,
}

fn default_dep_kind() -> DependencyKind {
    DependencyKind::Runtime
}

/// Kind of locked dependency
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DependencyKind {
    /// Runtime dependency (installed with package)
    #[default]
    Runtime,
    /// Build-time dependency (needed to compile)
    Build,
    /// Optional dependency (suggested)
    Optional,
    /// Development dependency (tests, linting)
    Dev,
}

impl std::fmt::Display for DependencyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime => write!(f, "runtime"),
            Self::Build => write!(f, "build"),
            Self::Optional => write!(f, "optional"),
            Self::Dev => write!(f, "dev"),
        }
    }
}

impl Lockfile {
    /// Create a new empty lockfile for a package
    pub fn new(package_name: &str, package_version: &str) -> Self {
        Self {
            metadata: LockfileMetadata {
                version: LOCKFILE_VERSION,
                generated: Utc::now().to_rfc3339(),
                generator: format!("conary {}", env!("CARGO_PKG_VERSION")),
                package: package_name.to_string(),
                package_version: package_version.to_string(),
                comment: None,
            },
            dependencies: Vec::new(),
            platform_deps: HashMap::new(),
            manifest_hash: None,
        }
    }

    /// Load lockfile from a path
    pub fn from_file(path: &Path) -> Result<Self, LockfileError> {
        let content = fs::read_to_string(path)?;
        Self::parse(&content)
    }

    /// Parse lockfile from TOML string
    pub fn parse(content: &str) -> Result<Self, LockfileError> {
        let lockfile: Lockfile = toml::from_str(content)?;

        // Validate version
        if lockfile.metadata.version > LOCKFILE_VERSION {
            return Err(LockfileError::VersionMismatch {
                expected: LOCKFILE_VERSION,
                found: lockfile.metadata.version,
            });
        }

        Ok(lockfile)
    }

    /// Write lockfile to a path
    pub fn write_to_file(&self, path: &Path) -> Result<(), LockfileError> {
        let content = self.to_toml()?;
        let mut file = fs::File::create(path)?;
        file.write_all(content.as_bytes())?;
        Ok(())
    }

    /// Serialize to TOML string
    pub fn to_toml(&self) -> Result<String, LockfileError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Add a dependency to the lockfile
    pub fn add_dependency(&mut self, dep: LockedDependency) {
        // Check if already exists, update if so
        if let Some(existing) = self
            .dependencies
            .iter_mut()
            .find(|d| d.name == dep.name && d.kind == dep.kind)
        {
            *existing = dep;
        } else {
            self.dependencies.push(dep);
        }
    }

    /// Add a platform-specific dependency
    pub fn add_platform_dependency(&mut self, platform: &str, dep: LockedDependency) {
        self.platform_deps
            .entry(platform.to_string())
            .or_default()
            .push(dep);
    }

    /// Get a dependency by name
    pub fn get_dependency(&self, name: &str) -> Option<&LockedDependency> {
        self.dependencies.iter().find(|d| d.name == name)
    }

    /// Get dependencies by kind
    pub fn dependencies_by_kind(&self, kind: DependencyKind) -> impl Iterator<Item = &LockedDependency> {
        self.dependencies.iter().filter(move |d| d.kind == kind)
    }

    /// Get runtime dependencies
    pub fn runtime_deps(&self) -> impl Iterator<Item = &LockedDependency> {
        self.dependencies_by_kind(DependencyKind::Runtime)
    }

    /// Get build dependencies
    pub fn build_deps(&self) -> impl Iterator<Item = &LockedDependency> {
        self.dependencies_by_kind(DependencyKind::Build)
    }

    /// Validate that current resolved dependencies match the lockfile
    ///
    /// Returns a list of mismatches if validation fails.
    pub fn validate_against(
        &self,
        resolved: &[LockedDependency],
    ) -> Result<(), Vec<LockfileError>> {
        let mut errors = Vec::new();

        for locked in &self.dependencies {
            if let Some(current) = resolved.iter().find(|r| r.name == locked.name) {
                // Check version match
                if current.version != locked.version {
                    errors.push(LockfileError::DependencyMismatch {
                        name: locked.name.clone(),
                        expected: locked.version.clone(),
                        found: current.version.clone(),
                    });
                }
                // Check content hash match (most important for reproducibility)
                if current.content_hash != locked.content_hash {
                    errors.push(LockfileError::DependencyMismatch {
                        name: format!("{} (content)", locked.name),
                        expected: locked.content_hash.clone(),
                        found: current.content_hash.clone(),
                    });
                }
            } else {
                errors.push(LockfileError::MissingDependency(locked.name.clone()));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Check if the lockfile needs updating based on manifest changes
    pub fn needs_update(&self, manifest_hash: &str) -> bool {
        self.manifest_hash.as_ref() != Some(&manifest_hash.to_string())
    }

    /// Set the manifest hash
    pub fn set_manifest_hash(&mut self, hash: &str) {
        self.manifest_hash = Some(hash.to_string());
    }

    /// Get total dependency count
    pub fn total_deps(&self) -> usize {
        self.dependencies.len()
            + self
                .platform_deps
                .values()
                .map(|v| v.len())
                .sum::<usize>()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.dependencies.is_empty() && self.platform_deps.is_empty()
    }
}

impl LockedDependency {
    /// Create a new locked dependency
    pub fn new(name: &str, version: &str, content_hash: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            content_hash: content_hash.to_string(),
            source: None,
            label: None,
            kind: DependencyKind::Runtime,
            requires: Vec::new(),
            dna_hash: None,
        }
    }

    /// Builder: set source
    pub fn with_source(mut self, source: &str) -> Self {
        self.source = Some(source.to_string());
        self
    }

    /// Builder: set label
    pub fn with_label(mut self, label: &str) -> Self {
        self.label = Some(label.to_string());
        self
    }

    /// Builder: set kind
    pub fn with_kind(mut self, kind: DependencyKind) -> Self {
        self.kind = kind;
        self
    }

    /// Builder: set DNA hash
    pub fn with_dna_hash(mut self, hash: &str) -> Self {
        self.dna_hash = Some(hash.to_string());
        self
    }

    /// Builder: add required dependency
    pub fn with_requires(mut self, deps: Vec<String>) -> Self {
        self.requires = deps;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lockfile_creation() {
        let lockfile = Lockfile::new("myapp", "1.0.0");

        assert_eq!(lockfile.metadata.package, "myapp");
        assert_eq!(lockfile.metadata.package_version, "1.0.0");
        assert_eq!(lockfile.metadata.version, LOCKFILE_VERSION);
        assert!(lockfile.dependencies.is_empty());
    }

    #[test]
    fn test_add_dependency() {
        let mut lockfile = Lockfile::new("myapp", "1.0.0");

        lockfile.add_dependency(LockedDependency::new(
            "openssl",
            "3.1.4",
            "sha256:abc123def456",
        ));

        assert_eq!(lockfile.dependencies.len(), 1);
        assert_eq!(lockfile.get_dependency("openssl").unwrap().version, "3.1.4");
    }

    #[test]
    fn test_dependency_kinds() {
        let mut lockfile = Lockfile::new("myapp", "1.0.0");

        lockfile.add_dependency(
            LockedDependency::new("libc", "2.38", "sha256:aaa")
                .with_kind(DependencyKind::Runtime),
        );
        lockfile.add_dependency(
            LockedDependency::new("gcc", "13.2", "sha256:bbb")
                .with_kind(DependencyKind::Build),
        );
        lockfile.add_dependency(
            LockedDependency::new("gdb", "14.1", "sha256:ccc")
                .with_kind(DependencyKind::Dev),
        );

        assert_eq!(lockfile.runtime_deps().count(), 1);
        assert_eq!(lockfile.build_deps().count(), 1);
        assert_eq!(lockfile.dependencies_by_kind(DependencyKind::Dev).count(), 1);
    }

    #[test]
    fn test_lockfile_serialization() {
        let mut lockfile = Lockfile::new("myapp", "1.0.0");
        lockfile.add_dependency(LockedDependency::new(
            "openssl",
            "3.1.4",
            "sha256:abc123",
        ));

        let toml = lockfile.to_toml().unwrap();
        assert!(toml.contains("openssl"));
        assert!(toml.contains("3.1.4"));
        assert!(toml.contains("sha256:abc123"));

        // Round-trip
        let parsed = Lockfile::parse(&toml).unwrap();
        assert_eq!(parsed.dependencies.len(), 1);
        assert_eq!(parsed.get_dependency("openssl").unwrap().version, "3.1.4");
    }

    #[test]
    fn test_lockfile_validation() {
        let mut lockfile = Lockfile::new("myapp", "1.0.0");
        lockfile.add_dependency(LockedDependency::new(
            "openssl",
            "3.1.4",
            "sha256:abc123",
        ));

        // Matching resolved deps
        let resolved = vec![LockedDependency::new("openssl", "3.1.4", "sha256:abc123")];
        assert!(lockfile.validate_against(&resolved).is_ok());

        // Version mismatch
        let resolved_wrong_version =
            vec![LockedDependency::new("openssl", "3.2.0", "sha256:abc123")];
        let result = lockfile.validate_against(&resolved_wrong_version);
        assert!(result.is_err());

        // Content hash mismatch
        let resolved_wrong_hash =
            vec![LockedDependency::new("openssl", "3.1.4", "sha256:different")];
        let result = lockfile.validate_against(&resolved_wrong_hash);
        assert!(result.is_err());
    }

    #[test]
    fn test_lockfile_parse_full() {
        let toml = r#"
[metadata]
version = 1
generated = "2024-01-15T10:30:00Z"
generator = "conary 0.2.0"
package = "myapp"
package_version = "1.0.0"

[[dependencies]]
name = "openssl"
version = "3.1.4"
content_hash = "sha256:abc123def456"
source = "https://repo.example.com"
kind = "runtime"

[[dependencies]]
name = "gcc"
version = "13.2.0"
content_hash = "sha256:def789"
kind = "build"
"#;

        let lockfile = Lockfile::parse(toml).unwrap();
        assert_eq!(lockfile.metadata.package, "myapp");
        assert_eq!(lockfile.dependencies.len(), 2);

        let openssl = lockfile.get_dependency("openssl").unwrap();
        assert_eq!(openssl.kind, DependencyKind::Runtime);

        let gcc = lockfile.get_dependency("gcc").unwrap();
        assert_eq!(gcc.kind, DependencyKind::Build);
    }

    #[test]
    fn test_platform_specific_deps() {
        let mut lockfile = Lockfile::new("myapp", "1.0.0");

        lockfile.add_platform_dependency(
            "x86_64-linux",
            LockedDependency::new("glibc", "2.38", "sha256:linux-amd64"),
        );
        lockfile.add_platform_dependency(
            "aarch64-linux",
            LockedDependency::new("glibc", "2.38", "sha256:linux-arm64"),
        );

        assert_eq!(lockfile.platform_deps.len(), 2);
        assert_eq!(lockfile.platform_deps["x86_64-linux"].len(), 1);
    }

    #[test]
    fn test_needs_update() {
        let mut lockfile = Lockfile::new("myapp", "1.0.0");
        lockfile.set_manifest_hash("sha256:original");

        assert!(!lockfile.needs_update("sha256:original"));
        assert!(lockfile.needs_update("sha256:changed"));
    }

    #[test]
    fn test_dependency_kind_display() {
        assert_eq!(format!("{}", DependencyKind::Runtime), "runtime");
        assert_eq!(format!("{}", DependencyKind::Build), "build");
        assert_eq!(format!("{}", DependencyKind::Optional), "optional");
        assert_eq!(format!("{}", DependencyKind::Dev), "dev");
    }
}
