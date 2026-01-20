// src/provenance/mod.rs

//! Package DNA / Full Provenance tracking
//!
//! This module provides complete lineage tracking for packages:
//! - Source Layer: upstream origin, patches, fetch timestamps
//! - Build Layer: recipe, dependencies, build environment attestation
//! - Signature Layer: builder signatures, reviewer signatures, transparency logs
//! - Content Layer: merkle roots, component hashes, chunk manifests
//!
//! Every package can answer: "Show me everything that went into this binary"

mod source;
mod build;
mod signature;
mod content;
mod dna;
mod slsa;

pub use source::{SourceProvenance, PatchInfo};
pub use build::{BuildProvenance, BuildDependency, HostAttestation, ReproducibilityInfo};
pub use signature::{SignatureProvenance, Signature, SignatureScope, TransparencyLog};
pub use content::{ContentProvenance, ComponentHash};
pub use dna::{PackageDna, DnaHash};
pub use slsa::{build_slsa_statement, SlsaContext, SlsaError};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

/// Complete provenance record for a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    /// Source layer - where the code came from
    pub source: SourceProvenance,

    /// Build layer - how it was built
    pub build: BuildProvenance,

    /// Signature layer - who vouches for it
    pub signatures: SignatureProvenance,

    /// Content layer - what's in the package
    pub content: ContentProvenance,

    /// When this provenance record was created
    pub created_at: DateTime<Utc>,
}

impl Provenance {
    /// Create a new provenance record
    pub fn new(
        source: SourceProvenance,
        build: BuildProvenance,
        signatures: SignatureProvenance,
        content: ContentProvenance,
    ) -> Self {
        Self {
            source,
            build,
            signatures,
            content,
            created_at: Utc::now(),
        }
    }

    /// Compute the DNA hash - a unique identifier for this provenance
    pub fn dna_hash(&self) -> DnaHash {
        let mut hasher = Sha256::new();

        // Hash all layers
        hasher.update(self.source.canonical_bytes());
        hasher.update(self.build.canonical_bytes());
        hasher.update(self.signatures.canonical_bytes());
        hasher.update(self.content.canonical_bytes());

        DnaHash::from_bytes(&hasher.finalize())
    }

    /// Serialize to JSON for storage
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserialize from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

impl Default for Provenance {
    fn default() -> Self {
        Self {
            source: SourceProvenance::default(),
            build: BuildProvenance::default(),
            signatures: SignatureProvenance::default(),
            content: ContentProvenance::default(),
            created_at: Utc::now(),
        }
    }
}

/// Trait for types that can provide canonical bytes for hashing
pub trait CanonicalBytes {
    fn canonical_bytes(&self) -> Vec<u8>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provenance_creation() {
        let prov = Provenance::default();
        assert!(prov.created_at <= Utc::now());
    }

    #[test]
    fn test_dna_hash_deterministic() {
        let prov1 = Provenance::default();
        let prov2 = Provenance::default();

        // Same content should produce same hash
        // (timestamps differ but aren't included in hash)
        let hash1 = prov1.dna_hash();
        let hash2 = prov2.dna_hash();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_json_roundtrip() {
        let prov = Provenance::default();
        let json = prov.to_json().unwrap();
        let restored = Provenance::from_json(&json).unwrap();

        assert_eq!(prov.source.upstream_url, restored.source.upstream_url);
    }
}
