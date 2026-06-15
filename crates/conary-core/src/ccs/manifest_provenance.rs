// conary-core/src/ccs/manifest_provenance.rs
//! CCS manifest provenance data transfer objects.

use serde::{Deserialize, Serialize};

/// Package DNA / Full provenance information in manifest
///
/// This section tracks the complete lineage of a package:
/// - Where the source came from (upstream URL, git commit, patches)
/// - How it was built (recipe, dependencies, environment)
/// - Who vouches for it (signatures, transparency logs)
/// - What's in it (merkle root, component hashes)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManifestProvenance {
    // === Source Layer ===
    /// URL where the source was fetched from
    #[serde(default)]
    pub upstream_url: Option<String>,

    /// Hash of the upstream source archive (sha256:...)
    #[serde(default)]
    pub upstream_hash: Option<String>,

    /// Git commit hash if built from git
    #[serde(default)]
    pub git_commit: Option<String>,

    /// When the source was fetched (ISO 8601)
    #[serde(default)]
    pub fetch_timestamp: Option<String>,

    /// Patches applied to source
    #[serde(default)]
    pub patches: Vec<ProvenancePatch>,

    // === Build Layer ===
    /// Hash of the recipe file used to build
    #[serde(default)]
    pub recipe_hash: Option<String>,

    /// Build timestamp (ISO 8601)
    #[serde(default)]
    pub build_timestamp: Option<String>,

    /// Architecture of the build host
    #[serde(default)]
    pub host_arch: Option<String>,

    /// Kernel version of the build host
    #[serde(default)]
    pub host_kernel: Option<String>,

    /// Build dependencies with their DNA hashes
    #[serde(default)]
    pub build_deps: Vec<ProvenanceDep>,

    /// M1a source/build origin class, such as native-built.
    #[serde(default)]
    pub origin_class: Option<String>,

    /// M1a build hardening level, such as host or sandboxed.
    #[serde(default)]
    pub hardening_level: Option<String>,

    /// Unsigned M2a hermetic build evidence, when a hermetic path produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hermetic_evidence: Option<crate::recipe::hermetic::HermeticBuildEvidence>,

    /// Signed M2 build attestation used by artifact-form publish gates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,

    /// Foreign package conversion boundary attested for release publish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,

    // === Signature Layer ===
    /// Signatures on this package
    #[serde(default)]
    pub signatures: Vec<ProvenanceSignature>,

    /// Sigstore Rekor transparency log index
    #[serde(default)]
    pub rekor_log_index: Option<u64>,

    /// SPDX SBOM hash
    #[serde(default)]
    pub sbom_spdx: Option<String>,

    // === Content Layer ===
    /// Merkle root hash of all file content hashes
    #[serde(default)]
    pub merkle_root: Option<String>,

    /// DNA hash - unique identifier for this provenance chain
    #[serde(default)]
    pub dna_hash: Option<String>,
}

/// A patch in the provenance chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenancePatch {
    /// URL or path to the patch
    #[serde(default)]
    pub url: Option<String>,

    /// Hash of the patch file
    pub hash: String,

    /// Who authored the patch
    #[serde(default)]
    pub author: Option<String>,

    /// Reason for the patch
    #[serde(default)]
    pub reason: Option<String>,
}

/// A build dependency with provenance tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceDep {
    /// Package name
    pub name: String,

    /// Package version
    pub version: String,

    /// DNA hash of the dependency (recursive provenance)
    #[serde(default)]
    pub dna_hash: Option<String>,
}

/// A signature in the provenance chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceSignature {
    /// Key identifier (email, fingerprint)
    pub keyid: String,

    /// Signature data (base64)
    pub sig: String,

    /// Scope: build, review, security, audit
    #[serde(default = "default_sig_scope")]
    pub scope: String,

    /// Timestamp (ISO 8601)
    #[serde(default)]
    pub timestamp: Option<String>,
}

fn default_sig_scope() -> String {
    "build".to_string()
}
