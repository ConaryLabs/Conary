// conary-core/src/provenance/dna.rs

//! Package DNA - unique identifier for full provenance

use serde::{Deserialize, Serialize};
use std::fmt;

/// Errors that can occur when creating a `DnaHash`
#[derive(Debug, thiserror::Error)]
pub enum DnaHashError {
    /// Input bytes are not the required 32 bytes
    #[error("DNA hash requires {expected} bytes, got {actual}")]
    InputTooShort { expected: usize, actual: usize },

    /// Hex decoding failed
    #[error("hex decode error: {0}")]
    HexDecode(#[from] hex::FromHexError),
}

/// A DNA hash uniquely identifies a package's complete provenance chain
///
/// This is computed by hashing the canonical representations of:
/// - Source provenance (upstream URL, hashes, patches)
/// - Build provenance (recipe, dependencies with their DNA hashes, environment)
/// - Signature provenance (builder, reviewers, transparency logs)
/// - Content provenance (merkle root, component hashes)
///
/// Two packages with the same DNA hash are provably from the same source,
/// built the same way, with the same dependencies.
#[derive(Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DnaHash {
    /// The hash bytes (SHA-256)
    #[serde(with = "hex_serde")]
    bytes: [u8; 32],
}

impl DnaHash {
    /// Create a DNA hash from raw bytes
    ///
    /// Returns an error if `bytes` is not exactly 32 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DnaHashError> {
        if bytes.len() != 32 {
            return Err(DnaHashError::InputTooShort {
                expected: 32,
                actual: bytes.len(),
            });
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Ok(Self { bytes: arr })
    }

    /// Create a DNA hash from a hex string
    pub fn from_hex(hex: &str) -> Result<Self, DnaHashError> {
        let hex = hex.strip_prefix("sha256:").unwrap_or(hex);
        let bytes = hex::decode(hex).map_err(DnaHashError::HexDecode)?;
        Self::from_bytes(&bytes)
    }

    /// Get the hash as a hex string with sha256: prefix
    pub fn to_hex(&self) -> String {
        format!("sha256:{}", hex::encode(self.bytes))
    }

    /// Get the raw bytes
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// Get a short form for display (first 12 hex chars)
    pub fn short(&self) -> String {
        hex::encode(&self.bytes[..6])
    }
}

impl fmt::Debug for DnaHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DnaHash({})", self.short())
    }
}

impl fmt::Display for DnaHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", hex::encode(self.bytes))
    }
}

/// Serde helper for hex encoding/decoding
mod hex_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom(format!(
                "expected 32 bytes for DnaHash, got {}",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }
}

/// Complete Package DNA record for database storage and queries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageDna {
    /// The DNA hash
    pub hash: DnaHash,

    /// Package name
    pub name: String,

    /// Package version
    pub version: String,

    /// Source provenance JSON
    pub source_json: String,

    /// Build provenance JSON
    pub build_json: String,

    /// Signature provenance JSON
    pub signatures_json: String,

    /// Rekor log index (if registered)
    pub rekor_log_index: Option<u64>,
}

impl PackageDna {
    /// Create a new package DNA record
    pub fn new(
        hash: DnaHash,
        name: &str,
        version: &str,
        source_json: String,
        build_json: String,
        signatures_json: String,
    ) -> Self {
        Self {
            hash,
            name: name.to_string(),
            version: version.to_string(),
            source_json,
            build_json,
            signatures_json,
            rekor_log_index: None,
        }
    }

    /// Set the Rekor log index
    pub fn with_rekor(mut self, log_index: u64) -> Self {
        self.rekor_log_index = Some(log_index);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dna_hash_from_bytes() {
        let bytes = [0x42u8; 32];
        let hash = DnaHash::from_bytes(&bytes).unwrap();
        assert_eq!(hash.as_bytes(), &bytes);
    }

    #[test]
    fn test_dna_hash_from_bytes_short_input() {
        let short = [0u8; 16];
        let result = DnaHash::from_bytes(&short);
        assert!(result.is_err());
    }

    #[test]
    fn test_dna_hash_hex_roundtrip() {
        let bytes = [0xABu8; 32];
        let hash = DnaHash::from_bytes(&bytes).unwrap();
        let hex = hash.to_hex();
        let restored = DnaHash::from_hex(&hex).unwrap();
        assert_eq!(hash, restored);
    }

    #[test]
    fn test_dna_hash_short() {
        let bytes = [
            0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let hash = DnaHash::from_bytes(&bytes).unwrap();
        assert_eq!(hash.short(), "123456789abc");
    }

    #[test]
    fn test_dna_hash_equality() {
        let hash1 = DnaHash::from_bytes(&[1u8; 32]).unwrap();
        let hash2 = DnaHash::from_bytes(&[1u8; 32]).unwrap();
        let hash3 = DnaHash::from_bytes(&[2u8; 32]).unwrap();

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_package_dna() {
        let hash = DnaHash::from_bytes(&[0xABu8; 32]).unwrap();
        let dna = PackageDna::new(
            hash,
            "nginx",
            "1.24.0",
            "{}".to_string(),
            "{}".to_string(),
            "{}".to_string(),
        )
        .with_rekor(12345678);

        assert_eq!(dna.name, "nginx");
        assert_eq!(dna.rekor_log_index, Some(12345678));
    }

    #[test]
    fn test_json_roundtrip() {
        let hash = DnaHash::from_bytes(&[0x42u8; 32]).unwrap();
        let json = serde_json::to_string(&hash).unwrap();
        let restored: DnaHash = serde_json::from_str(&json).unwrap();
        assert_eq!(hash, restored);
    }
}
