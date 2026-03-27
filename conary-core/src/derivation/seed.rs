// conary-core/src/derivation/seed.rs

//! Layer 0 seed model for loading and verifying bootstrap seeds.
//!
//! A **seed** is a pre-built EROFS image that serves as the initial build
//! environment (Layer 0) for the CAS-layered bootstrap. It contains the
//! minimal toolchain (compiler, libc, coreutils) needed to build everything
//! else from source.
//!
//! Seeds are stored as a directory containing `seed.erofs` (the image) and
//! `seed.toml` (metadata). The `seed_id` in the metadata must match the
//! SHA-256 hash of the EROFS image file, providing content-addressable
//! integrity verification.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::derivation::compose::erofs_image_hash;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur when loading or verifying a seed.
#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    /// The EROFS image file (`seed.erofs`) is missing from the seed directory.
    #[error("missing seed image: {0}")]
    MissingImage(String),

    /// The metadata file (`seed.toml`) is missing from the seed directory.
    #[error("missing seed metadata: {0}")]
    MissingMetadata(String),

    /// An I/O operation failed.
    #[error("I/O error: {0}")]
    Io(String),

    /// The TOML metadata could not be parsed.
    #[error("metadata parse error: {0}")]
    Parse(String),

    /// The `seed_id` in metadata does not match the actual image hash.
    #[error("seed hash mismatch: expected {expected}, actual {actual}")]
    HashMismatch {
        /// The `seed_id` declared in `seed.toml`.
        expected: String,
        /// The SHA-256 hash computed from the EROFS image file.
        actual: String,
    },
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// How the seed was produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeedSource {
    /// Built by the community and distributed as a trusted artifact.
    Community,
    /// Imported from an external source (e.g., a distro minimal root).
    Imported,
    /// Built locally by the user's own bootstrap pipeline.
    SelfBuilt,
    /// Adopted from an existing distro installation (e.g., a live Arch system).
    Adopted,
}

impl fmt::Display for SeedSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Community => write!(f, "community"),
            Self::Imported => write!(f, "imported"),
            Self::SelfBuilt => write!(f, "selfbuilt"),
            Self::Adopted => write!(f, "adopted"),
        }
    }
}

/// Metadata describing a bootstrap seed, serialized as `seed.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedMetadata {
    /// SHA-256 hash of the EROFS image — serves as the content address.
    pub seed_id: String,
    /// How this seed was produced.
    pub source: SeedSource,
    /// URL where this seed was originally obtained, if applicable.
    pub origin_url: Option<String>,
    /// Identifier for the builder that produced this seed.
    pub builder: Option<String>,
    /// List of packages included in the seed image.
    pub packages: Vec<String>,
    /// Target triple (e.g., `x86_64-unknown-linux-gnu`).
    pub target_triple: String,
    /// Identifiers of entities that verified this seed's integrity.
    pub verified_by: Vec<String>,
    /// Distro name for adopted seeds (e.g., `"archlinux"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_distro: Option<String>,
    /// Distro version for adopted seeds (e.g., `"2026.03.01"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_version: Option<String>,
}

/// A loaded and verified bootstrap seed.
///
/// Combines parsed metadata with the on-disk paths needed to mount and use
/// the seed as a build environment.
#[derive(Debug)]
pub struct Seed {
    /// Parsed metadata from `seed.toml`.
    pub metadata: SeedMetadata,
    /// Path to the `seed.erofs` image file.
    pub image_path: PathBuf,
    /// CAS object directory associated with this seed.
    pub cas_dir: PathBuf,
}

impl Seed {
    /// Load a seed from a directory containing `seed.erofs` and `seed.toml`.
    ///
    /// Reads and parses the metadata, then verifies that `seed_id` matches
    /// the SHA-256 hash of the EROFS image file.
    ///
    /// The `cas_dir` is set to `seed_dir/cas` by convention — callers can
    /// override it on the returned struct if needed.
    ///
    /// # Errors
    ///
    /// - [`SeedError::MissingImage`] if `seed.erofs` does not exist.
    /// - [`SeedError::MissingMetadata`] if `seed.toml` does not exist.
    /// - [`SeedError::Io`] if files cannot be read.
    /// - [`SeedError::Parse`] if `seed.toml` is not valid TOML.
    /// - [`SeedError::HashMismatch`] if the declared `seed_id` does not match
    ///   the actual image hash.
    pub fn load_local(seed_dir: &Path) -> Result<Self, SeedError> {
        let image_path = seed_dir.join("seed.erofs");
        let metadata_path = seed_dir.join("seed.toml");

        // Check for a missing image explicitly before attempting to hash it.
        // This avoids relying on error message string matching for NotFound detection.
        if !image_path.exists() {
            return Err(SeedError::MissingImage(format!(
                "{}",
                image_path.display()
            )));
        }

        let actual_hash = erofs_image_hash(&image_path)
            .map_err(|e| SeedError::Io(format!("hashing {}: {e}", image_path.display())))?;

        // Read and parse metadata directly; map NotFound to MissingMetadata.
        let toml_content = std::fs::read_to_string(&metadata_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SeedError::MissingMetadata(format!("{}", metadata_path.display()))
            } else {
                SeedError::Io(format!("reading {}: {e}", metadata_path.display()))
            }
        })?;
        let metadata: SeedMetadata =
            toml::from_str(&toml_content).map_err(|e| SeedError::Parse(e.to_string()))?;

        if metadata.seed_id != actual_hash {
            return Err(SeedError::HashMismatch {
                expected: metadata.seed_id,
                actual: actual_hash,
            });
        }

        let cas_dir = seed_dir.join("cas");

        Ok(Self {
            metadata,
            image_path,
            cas_dir,
        })
    }

    /// Returns the content-addressed hash identifying this seed's build
    /// environment.
    ///
    /// This is the SHA-256 of the EROFS image, which can be used as the
    /// `build_env_hash` when constructing derivation inputs on top of this
    /// seed.
    #[must_use]
    pub fn build_env_hash(&self) -> &str {
        &self.metadata.seed_id
    }
}

// ---------------------------------------------------------------------------
// Probe-based validation
// ---------------------------------------------------------------------------

/// Result of probing a seed's build environment capabilities.
#[derive(Debug)]
pub struct SeedValidation {
    /// True if `gcc --version` succeeds inside the sysroot.
    pub has_c_compiler: bool,
    /// True if `usr/include/stdio.h` exists inside the sysroot.
    pub has_libc_headers: bool,
    /// True if `make --version` succeeds inside the sysroot.
    pub has_make: bool,
    /// True if `/bin/sh -c echo ok` succeeds inside the sysroot.
    pub has_shell: bool,
    /// True if `ls --version` succeeds inside the sysroot.
    pub has_coreutils: bool,
    /// True if `ld --version` succeeds inside the sysroot.
    pub has_binutils: bool,
}

impl SeedValidation {
    /// Probe a mounted sysroot for required build tools.
    pub fn probe(sysroot: &Path) -> Self {
        Self {
            has_c_compiler: probe_cmd(sysroot, &["gcc", "--version"]),
            has_libc_headers: sysroot.join("usr/include/stdio.h").exists(),
            has_make: probe_cmd(sysroot, &["make", "--version"]),
            has_shell: probe_cmd(sysroot, &["/bin/sh", "-c", "echo ok"]),
            has_coreutils: probe_cmd(sysroot, &["ls", "--version"]),
            has_binutils: probe_cmd(sysroot, &["ld", "--version"]),
        }
    }

    /// Returns `true` if all required build tools are present.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.has_c_compiler
            && self.has_libc_headers
            && self.has_make
            && self.has_shell
            && self.has_coreutils
            && self.has_binutils
    }

    /// Returns the names of any missing tools.
    #[must_use]
    pub fn missing_tools(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self.has_c_compiler {
            missing.push("gcc");
        }
        if !self.has_libc_headers {
            missing.push("libc headers");
        }
        if !self.has_make {
            missing.push("make");
        }
        if !self.has_shell {
            missing.push("/bin/sh");
        }
        if !self.has_coreutils {
            missing.push("coreutils");
        }
        if !self.has_binutils {
            missing.push("binutils (ld)");
        }
        missing
    }
}

fn probe_cmd(sysroot: &Path, args: &[&str]) -> bool {
    std::process::Command::new("chroot")
        .arg(sysroot)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_metadata_serializes_to_toml() {
        let meta = SeedMetadata {
            seed_id: "a".repeat(64),
            source: SeedSource::Community,
            origin_url: Some("https://seeds.conary.io/v1/x86_64".to_owned()),
            builder: Some("conary-bootstrap-0.6.0".to_owned()),
            packages: vec!["gcc".to_owned(), "glibc".to_owned(), "coreutils".to_owned()],
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            verified_by: vec!["sig:abc123".to_owned()],
            origin_distro: None,
            origin_version: None,
        };

        let toml_str = toml::to_string_pretty(&meta).expect("serialize to TOML");

        assert!(
            toml_str.contains("source = \"community\""),
            "source should serialize as lowercase: {toml_str}"
        );
        assert!(
            toml_str.contains(&"a".repeat(64)),
            "seed_id should be present"
        );
        assert!(
            toml_str.contains("x86_64-unknown-linux-gnu"),
            "target_triple should be present"
        );
    }

    #[test]
    fn seed_source_roundtrips_through_serde() {
        // TOML can't serialize a bare enum; test via JSON which can.
        for source in [
            SeedSource::Community,
            SeedSource::Imported,
            SeedSource::SelfBuilt,
        ] {
            let serialized = serde_json::to_string(&source).expect("serialize SeedSource");
            let deserialized: SeedSource =
                serde_json::from_str(&serialized).expect("deserialize SeedSource");
            assert_eq!(source, deserialized);
        }
    }

    #[test]
    fn seed_source_serde_values() {
        // Verify rename_all = "lowercase" produces correct strings.
        assert_eq!(
            serde_json::to_string(&SeedSource::Community).unwrap(),
            "\"community\""
        );
        assert_eq!(
            serde_json::to_string(&SeedSource::Imported).unwrap(),
            "\"imported\""
        );
        assert_eq!(
            serde_json::to_string(&SeedSource::SelfBuilt).unwrap(),
            "\"selfbuilt\""
        );
    }

    #[test]
    fn load_local_fails_on_missing_directory() {
        let result = Seed::load_local(Path::new("/nonexistent/seed/dir"));

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, SeedError::MissingImage(_)),
            "expected MissingImage, got: {err}"
        );
    }

    #[test]
    fn load_local_fails_on_missing_metadata() {
        let dir = tempfile::tempdir().unwrap();
        // Create image but no metadata.
        std::fs::write(dir.path().join("seed.erofs"), b"fake image").unwrap();

        let result = Seed::load_local(dir.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, SeedError::MissingMetadata(_)),
            "expected MissingMetadata, got: {err}"
        );
    }

    #[test]
    fn load_local_fails_on_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let image_content = b"fake erofs image content";
        std::fs::write(dir.path().join("seed.erofs"), image_content).unwrap();

        let meta = SeedMetadata {
            seed_id: "0".repeat(64), // Wrong hash on purpose.
            source: SeedSource::Community,
            origin_url: None,
            builder: None,
            packages: vec![],
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            verified_by: vec![],
            origin_distro: None,
            origin_version: None,
        };
        let toml_str = toml::to_string_pretty(&meta).unwrap();
        std::fs::write(dir.path().join("seed.toml"), toml_str).unwrap();

        let result = Seed::load_local(dir.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, SeedError::HashMismatch { .. }),
            "expected HashMismatch, got: {err}"
        );
    }

    #[test]
    fn load_local_succeeds_with_correct_hash() {
        let dir = tempfile::tempdir().unwrap();
        let image_content = b"deterministic seed image bytes";
        let image_path = dir.path().join("seed.erofs");
        std::fs::write(&image_path, image_content).unwrap();

        // Compute the real hash so metadata matches.
        let actual_hash = erofs_image_hash(&image_path).unwrap();

        let meta = SeedMetadata {
            seed_id: actual_hash.clone(),
            source: SeedSource::SelfBuilt,
            origin_url: None,
            builder: Some("test".to_owned()),
            packages: vec!["gcc".to_owned()],
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            verified_by: vec![],
            origin_distro: None,
            origin_version: None,
        };
        let toml_str = toml::to_string_pretty(&meta).unwrap();
        std::fs::write(dir.path().join("seed.toml"), toml_str).unwrap();

        let seed = Seed::load_local(dir.path()).expect("load should succeed");

        assert_eq!(seed.build_env_hash(), actual_hash);
        assert_eq!(seed.metadata.source, SeedSource::SelfBuilt);
        assert_eq!(seed.metadata.packages, vec!["gcc"]);
        assert_eq!(seed.image_path, image_path);
        assert_eq!(seed.cas_dir, dir.path().join("cas"));
    }

    #[test]
    fn build_env_hash_returns_seed_id() {
        let dir = tempfile::tempdir().unwrap();
        let image_content = b"test image for hash check";
        let image_path = dir.path().join("seed.erofs");
        std::fs::write(&image_path, image_content).unwrap();

        let actual_hash = erofs_image_hash(&image_path).unwrap();

        let meta = SeedMetadata {
            seed_id: actual_hash.clone(),
            source: SeedSource::Imported,
            origin_url: None,
            builder: None,
            packages: vec![],
            target_triple: "aarch64-unknown-linux-gnu".to_owned(),
            verified_by: vec![],
            origin_distro: None,
            origin_version: None,
        };
        let toml_str = toml::to_string_pretty(&meta).unwrap();
        std::fs::write(dir.path().join("seed.toml"), toml_str).unwrap();

        let seed = Seed::load_local(dir.path()).unwrap();
        assert_eq!(seed.build_env_hash(), &actual_hash);
    }

    #[test]
    fn adopted_source_serde_roundtrip() {
        let meta = SeedMetadata {
            seed_id: "abc".into(),
            source: SeedSource::Adopted,
            origin_url: None,
            builder: None,
            packages: vec!["gcc".into()],
            target_triple: "x86_64-unknown-linux-gnu".into(),
            verified_by: vec![],
            origin_distro: Some("archlinux".into()),
            origin_version: Some("2026.03.01".into()),
        };
        let toml_str = toml::to_string(&meta).unwrap();
        assert!(toml_str.contains(r#"source = "adopted""#));
        assert!(toml_str.contains(r#"origin_distro = "archlinux""#));

        let parsed: SeedMetadata = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.source, SeedSource::Adopted);
        assert_eq!(parsed.origin_distro.as_deref(), Some("archlinux"));
    }

    #[test]
    fn seed_validation_missing_tools() {
        let v = SeedValidation {
            has_c_compiler: true,
            has_libc_headers: false,
            has_make: true,
            has_shell: true,
            has_coreutils: false,
            has_binutils: true,
        };
        assert!(!v.is_valid());
        let missing = v.missing_tools();
        assert_eq!(missing, vec!["libc headers", "coreutils"]);
    }

    #[test]
    fn seed_validation_all_present() {
        let v = SeedValidation {
            has_c_compiler: true,
            has_libc_headers: true,
            has_make: true,
            has_shell: true,
            has_coreutils: true,
            has_binutils: true,
        };
        assert!(v.is_valid());
        assert!(v.missing_tools().is_empty());
    }

    #[test]
    fn backward_compat_no_origin_fields() {
        // Old seed.toml without origin_distro/origin_version should still parse.
        let toml_str = r#"
seed_id = "abc"
source = "community"
packages = ["gcc"]
target_triple = "x86_64-unknown-linux-gnu"
verified_by = []
"#;
        let parsed: SeedMetadata = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.source, SeedSource::Community);
        assert!(parsed.origin_distro.is_none());
        assert!(parsed.origin_version.is_none());
    }
}
