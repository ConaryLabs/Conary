// src/provenance/signature.rs

//! Signature layer provenance - who vouches for this package

use super::CanonicalBytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Signature layer provenance information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SignatureProvenance {
    /// Builder's signature
    #[serde(default)]
    pub builder_sig: Option<Signature>,

    /// Reviewer signatures (security audits, quality reviews, etc.)
    #[serde(default)]
    pub reviewer_sigs: Vec<Signature>,

    /// Transparency log entry (Rekor, etc.)
    #[serde(default)]
    pub transparency_log: Option<TransparencyLog>,

    /// SBOM (Software Bill of Materials) references
    #[serde(default)]
    pub sbom: Option<SbomRef>,
}

impl SignatureProvenance {
    /// Create new signature provenance with builder signature
    pub fn with_builder(sig: Signature) -> Self {
        Self {
            builder_sig: Some(sig),
            ..Default::default()
        }
    }

    /// Add a reviewer signature
    pub fn add_reviewer(&mut self, sig: Signature) {
        self.reviewer_sigs.push(sig);
    }

    /// Set transparency log entry
    pub fn set_transparency_log(&mut self, log: TransparencyLog) {
        self.transparency_log = Some(log);
    }

    /// Check if this package has been signed
    pub fn is_signed(&self) -> bool {
        self.builder_sig.is_some()
    }

    /// Check if this package has security review
    pub fn has_security_review(&self) -> bool {
        self.reviewer_sigs.iter().any(|s| s.scope == SignatureScope::Security)
    }

    /// Get all signers (builder + reviewers)
    pub fn all_signers(&self) -> Vec<&str> {
        let mut signers = Vec::new();
        if let Some(ref builder) = self.builder_sig {
            signers.push(builder.key_id.as_str());
        }
        for reviewer in &self.reviewer_sigs {
            signers.push(reviewer.key_id.as_str());
        }
        signers
    }
}

impl CanonicalBytes for SignatureProvenance {
    fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        if let Some(ref sig) = self.builder_sig {
            bytes.extend_from_slice(b"builder:");
            bytes.extend_from_slice(sig.key_id.as_bytes());
            bytes.push(b':');
            bytes.extend_from_slice(sig.signature.as_bytes());
            bytes.push(0);
        }

        // Sort reviewer sigs by key_id for determinism
        let mut reviewers: Vec<_> = self.reviewer_sigs.iter().collect();
        reviewers.sort_by(|a, b| a.key_id.cmp(&b.key_id));

        for sig in reviewers {
            bytes.extend_from_slice(b"reviewer:");
            bytes.extend_from_slice(sig.key_id.as_bytes());
            bytes.push(b':');
            bytes.extend_from_slice(sig.signature.as_bytes());
            bytes.push(0);
        }

        if let Some(ref log) = self.transparency_log {
            bytes.extend_from_slice(b"rekor:");
            bytes.extend_from_slice(log.log_index.to_string().as_bytes());
            bytes.push(0);
        }

        bytes
    }
}

/// A cryptographic signature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    /// Key identifier (email, fingerprint, etc.)
    pub key_id: String,

    /// The signature data (base64 encoded)
    pub signature: String,

    /// What this signature covers
    pub scope: SignatureScope,

    /// When the signature was made
    pub timestamp: DateTime<Utc>,

    /// Signature algorithm used
    #[serde(default)]
    pub algorithm: Option<String>,

    /// Additional metadata
    #[serde(default)]
    pub metadata: Option<String>,
}

impl Signature {
    /// Create a new signature
    pub fn new(key_id: &str, signature: &str, scope: SignatureScope) -> Self {
        Self {
            key_id: key_id.to_string(),
            signature: signature.to_string(),
            scope,
            timestamp: Utc::now(),
            algorithm: None,
            metadata: None,
        }
    }

    /// Set the algorithm
    pub fn with_algorithm(mut self, algorithm: &str) -> Self {
        self.algorithm = Some(algorithm.to_string());
        self
    }

    /// Create a builder signature
    pub fn builder(key_id: &str, signature: &str) -> Self {
        Self::new(key_id, signature, SignatureScope::Build)
    }

    /// Create a security review signature
    pub fn security_review(key_id: &str, signature: &str) -> Self {
        Self::new(key_id, signature, SignatureScope::Security)
    }

    /// Create an audit signature
    pub fn audit(key_id: &str, signature: &str) -> Self {
        Self::new(key_id, signature, SignatureScope::Audit)
    }
}

/// What aspect of the package the signature covers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SignatureScope {
    /// Signature from the builder (built this package)
    #[default]
    Build,

    /// Security review signature
    Security,

    /// General code review
    Review,

    /// Full security audit
    Audit,

    /// Performance review/benchmarks
    Performance,

    /// License compliance review
    Compliance,
}

impl std::fmt::Display for SignatureScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Build => write!(f, "build"),
            Self::Security => write!(f, "security"),
            Self::Review => write!(f, "review"),
            Self::Audit => write!(f, "audit"),
            Self::Performance => write!(f, "performance"),
            Self::Compliance => write!(f, "compliance"),
        }
    }
}

/// Transparency log entry (Sigstore Rekor, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransparencyLog {
    /// Log provider (e.g., "rekor.sigstore.dev")
    pub provider: String,

    /// Entry index in the log
    pub log_index: u64,

    /// Full URL to the log entry
    #[serde(default)]
    pub entry_url: Option<String>,

    /// Inclusion proof (base64 encoded)
    #[serde(default)]
    pub inclusion_proof: Option<String>,

    /// When the entry was recorded
    pub integrated_time: DateTime<Utc>,
}

impl TransparencyLog {
    /// Create a Rekor log entry
    pub fn rekor(log_index: u64) -> Self {
        Self {
            provider: "rekor.sigstore.dev".to_string(),
            log_index,
            entry_url: Some(format!(
                "https://rekor.sigstore.dev/api/v1/log/entries?logIndex={}",
                log_index
            )),
            inclusion_proof: None,
            integrated_time: Utc::now(),
        }
    }

    /// Set inclusion proof
    pub fn with_proof(mut self, proof: &str) -> Self {
        self.inclusion_proof = Some(proof.to_string());
        self
    }
}

/// SBOM (Software Bill of Materials) reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SbomRef {
    /// SPDX SBOM hash
    #[serde(default)]
    pub spdx_hash: Option<String>,

    /// CycloneDX SBOM hash
    #[serde(default)]
    pub cyclonedx_hash: Option<String>,

    /// URL where SBOM is published
    #[serde(default)]
    pub url: Option<String>,
}

impl SbomRef {
    /// Create SBOM reference with SPDX hash
    pub fn spdx(hash: &str) -> Self {
        Self {
            spdx_hash: Some(hash.to_string()),
            cyclonedx_hash: None,
            url: None,
        }
    }

    /// Create SBOM reference with CycloneDX hash
    pub fn cyclonedx(hash: &str) -> Self {
        Self {
            spdx_hash: None,
            cyclonedx_hash: Some(hash.to_string()),
            url: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_creation() {
        let sig = Signature::builder("builder@example.com", "base64sig==")
            .with_algorithm("ed25519");

        assert_eq!(sig.scope, SignatureScope::Build);
        assert_eq!(sig.algorithm.as_deref(), Some("ed25519"));
    }

    #[test]
    fn test_signature_provenance() {
        let mut prov = SignatureProvenance::with_builder(
            Signature::builder("builder@example.com", "sig1==")
        );

        assert!(prov.is_signed());
        assert!(!prov.has_security_review());

        prov.add_reviewer(Signature::security_review("security@example.com", "sig2=="));
        assert!(prov.has_security_review());

        assert_eq!(prov.all_signers().len(), 2);
    }

    #[test]
    fn test_rekor_log() {
        let log = TransparencyLog::rekor(12345678)
            .with_proof("inclusion_proof_base64");

        assert!(log.entry_url.as_ref().unwrap().contains("12345678"));
        assert!(log.inclusion_proof.is_some());
    }

    #[test]
    fn test_canonical_bytes() {
        let prov1 = SignatureProvenance::with_builder(
            Signature::builder("builder@example.com", "sig==")
        );
        let prov2 = SignatureProvenance::with_builder(
            Signature::builder("builder@example.com", "sig==")
        );

        assert_eq!(prov1.canonical_bytes(), prov2.canonical_bytes());
    }
}
