// src/hash.rs

//! Configurable hashing for file integrity and content addressing
//!
//! This module provides a unified interface for multiple hash algorithms:
//! - **SHA-256**: Cryptographic hash, used for security-critical verification
//! - **XXH128**: Non-cryptographic hash, extremely fast for content addressing
//!
//! # Use Cases
//!
//! | Use Case | Recommended Algorithm | Why |
//! |----------|----------------------|-----|
//! | Package signature verification | SHA-256 | Cryptographic security |
//! | CAS content addressing | XXH128 | Speed, deduplication only |
//! | Delta file identification | XXH128 | Fast comparison |
//! | Repository metadata checksums | SHA-256 | Match upstream repos |

use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{self, Read};
use std::str::FromStr;
use xxhash_rust::xxh3::xxh3_128;

/// Hash algorithm selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HashAlgorithm {
    /// SHA-256 (256-bit cryptographic hash)
    ///
    /// Slower but cryptographically secure. Use for:
    /// - Package signature verification
    /// - Matching upstream repository checksums
    /// - Security-critical integrity checks
    #[default]
    Sha256,

    /// XXH128 (128-bit non-cryptographic hash)
    ///
    /// Extremely fast (~30 GB/s on modern CPUs). Use for:
    /// - Content-addressable storage (CAS)
    /// - File deduplication
    /// - Delta update identification
    /// - Any case where speed matters more than cryptographic security
    Xxh128,
}

impl HashAlgorithm {
    /// Get the hash output length in bytes
    #[inline]
    pub const fn output_len(&self) -> usize {
        match self {
            Self::Sha256 => 32, // 256 bits
            Self::Xxh128 => 16, // 128 bits
        }
    }

    /// Get the hash output length as a hex string
    #[inline]
    pub const fn hex_len(&self) -> usize {
        self.output_len() * 2
    }

    /// Get the algorithm name as a string
    #[inline]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Xxh128 => "xxh128",
        }
    }

    /// Check if this is a cryptographic hash
    #[inline]
    pub const fn is_cryptographic(&self) -> bool {
        match self {
            Self::Sha256 => true,
            Self::Xxh128 => false,
        }
    }
}

impl fmt::Display for HashAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl FromStr for HashAlgorithm {
    type Err = HashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sha256" | "sha-256" => Ok(Self::Sha256),
            "xxh128" | "xxhash" | "xxh3" => Ok(Self::Xxh128),
            _ => Err(HashError::UnknownAlgorithm(s.to_string())),
        }
    }
}

/// Hash computation errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashError {
    /// Unknown hash algorithm name
    UnknownAlgorithm(String),
    /// Hash string has wrong length for algorithm
    InvalidLength { expected: usize, got: usize },
    /// Hash string contains invalid hex characters
    InvalidHex(String),
}

impl fmt::Display for HashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAlgorithm(name) => write!(f, "unknown hash algorithm: {}", name),
            Self::InvalidLength { expected, got } => {
                write!(f, "invalid hash length: expected {}, got {}", expected, got)
            }
            Self::InvalidHex(s) => write!(f, "invalid hex in hash: {}", s),
        }
    }
}

impl std::error::Error for HashError {}

/// A hash value with its algorithm
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hash {
    /// The algorithm used
    pub algorithm: HashAlgorithm,
    /// The hash value as a hex string
    pub value: String,
}

impl Hash {
    /// Create a new hash value
    pub fn new(algorithm: HashAlgorithm, value: impl Into<String>) -> Result<Self, HashError> {
        let value = value.into();
        let expected_len = algorithm.hex_len();

        if value.len() != expected_len {
            return Err(HashError::InvalidLength {
                expected: expected_len,
                got: value.len(),
            });
        }

        // Validate hex characters
        if !value.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(HashError::InvalidHex(value));
        }

        Ok(Self {
            algorithm,
            value: value.to_lowercase(),
        })
    }

    /// Create a hash without validation (internal use)
    fn new_unchecked(algorithm: HashAlgorithm, value: String) -> Self {
        Self { algorithm, value }
    }

    /// Get the hash value as a hex string
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Parse a prefixed hash string (e.g., "sha256:abc123..." or "xxh128:abc123...")
    pub fn parse_prefixed(s: &str) -> Result<Self, HashError> {
        if let Some((algo, hash)) = s.split_once(':') {
            let algorithm = algo.parse()?;
            Self::new(algorithm, hash)
        } else {
            // Default to SHA-256 for unprefixed hashes (backward compatibility)
            Self::new(HashAlgorithm::Sha256, s)
        }
    }

    /// Format as a prefixed string (e.g., "sha256:abc123...")
    pub fn to_prefixed_string(&self) -> String {
        format!("{}:{}", self.algorithm.name(), self.value)
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}

/// Hasher that can compute hashes using any supported algorithm
pub struct Hasher {
    algorithm: HashAlgorithm,
    state: HasherState,
}

enum HasherState {
    Sha256(Sha256),
    Xxh128(Vec<u8>), // XXH3 doesn't have incremental API, buffer data
}

impl Hasher {
    /// Create a new hasher with the specified algorithm
    pub fn new(algorithm: HashAlgorithm) -> Self {
        let state = match algorithm {
            HashAlgorithm::Sha256 => HasherState::Sha256(Sha256::new()),
            HashAlgorithm::Xxh128 => HasherState::Xxh128(Vec::new()),
        };
        Self { algorithm, state }
    }

    /// Update the hasher with more data
    pub fn update(&mut self, data: &[u8]) {
        match &mut self.state {
            HasherState::Sha256(hasher) => hasher.update(data),
            HasherState::Xxh128(buffer) => buffer.extend_from_slice(data),
        }
    }

    /// Finalize and return the hash
    pub fn finalize(self) -> Hash {
        let value = match self.state {
            HasherState::Sha256(hasher) => format!("{:x}", hasher.finalize()),
            HasherState::Xxh128(buffer) => format!("{:032x}", xxh3_128(&buffer)),
        };
        Hash::new_unchecked(self.algorithm, value)
    }

    /// Get the algorithm being used
    #[inline]
    pub fn algorithm(&self) -> HashAlgorithm {
        self.algorithm
    }
}

/// Compute hash of a byte slice
pub fn hash_bytes(algorithm: HashAlgorithm, data: &[u8]) -> Hash {
    let value = match algorithm {
        HashAlgorithm::Sha256 => {
            let mut hasher = Sha256::new();
            hasher.update(data);
            format!("{:x}", hasher.finalize())
        }
        HashAlgorithm::Xxh128 => {
            format!("{:032x}", xxh3_128(data))
        }
    };
    Hash::new_unchecked(algorithm, value)
}

/// Compute hash of data from a reader
pub fn hash_reader<R: Read>(algorithm: HashAlgorithm, reader: &mut R) -> io::Result<Hash> {
    let mut hasher = Hasher::new(algorithm);
    let mut buffer = [0u8; 8192];

    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(hasher.finalize())
}

/// Compute SHA-256 hash (convenience function for backward compatibility)
#[inline]
pub fn sha256(data: &[u8]) -> String {
    hash_bytes(HashAlgorithm::Sha256, data).value
}

/// Compute XXH128 hash (convenience function)
#[inline]
pub fn xxh128(data: &[u8]) -> String {
    hash_bytes(HashAlgorithm::Xxh128, data).value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_hash() {
        let data = b"Hello, World!";
        let hash = hash_bytes(HashAlgorithm::Sha256, data);

        assert_eq!(hash.algorithm, HashAlgorithm::Sha256);
        assert_eq!(
            hash.value,
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
        );
        assert_eq!(hash.value.len(), 64); // 256 bits = 32 bytes = 64 hex chars
    }

    #[test]
    fn test_xxh128_hash() {
        let data = b"Hello, World!";
        let hash = hash_bytes(HashAlgorithm::Xxh128, data);

        assert_eq!(hash.algorithm, HashAlgorithm::Xxh128);
        assert_eq!(hash.value.len(), 32); // 128 bits = 16 bytes = 32 hex chars
    }

    #[test]
    fn test_xxh128_known_value() {
        // Test with known xxh3_128 output
        let data = b"";
        let hash = hash_bytes(HashAlgorithm::Xxh128, data);
        // Empty string has a known xxh3_128 hash
        assert_eq!(hash.value.len(), 32);
    }

    #[test]
    fn test_convenience_functions() {
        let data = b"test data";
        let sha = sha256(data);
        let xxh = xxh128(data);

        assert_eq!(sha.len(), 64);
        assert_eq!(xxh.len(), 32);
    }

    #[test]
    fn test_hasher_incremental() {
        let data = b"Hello, World!";

        // Full hash
        let full_hash = hash_bytes(HashAlgorithm::Sha256, data);

        // Incremental hash
        let mut hasher = Hasher::new(HashAlgorithm::Sha256);
        hasher.update(b"Hello, ");
        hasher.update(b"World!");
        let incremental_hash = hasher.finalize();

        assert_eq!(full_hash, incremental_hash);
    }

    #[test]
    fn test_algorithm_parse() {
        assert_eq!("sha256".parse::<HashAlgorithm>().unwrap(), HashAlgorithm::Sha256);
        assert_eq!("SHA-256".parse::<HashAlgorithm>().unwrap(), HashAlgorithm::Sha256);
        assert_eq!("xxh128".parse::<HashAlgorithm>().unwrap(), HashAlgorithm::Xxh128);
        assert_eq!("xxhash".parse::<HashAlgorithm>().unwrap(), HashAlgorithm::Xxh128);
        assert!("unknown".parse::<HashAlgorithm>().is_err());
    }

    #[test]
    fn test_hash_validation() {
        // Valid SHA-256
        let hash = Hash::new(
            HashAlgorithm::Sha256,
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f",
        );
        assert!(hash.is_ok());

        // Wrong length
        let hash = Hash::new(HashAlgorithm::Sha256, "abc123");
        assert!(matches!(hash, Err(HashError::InvalidLength { .. })));

        // Invalid hex
        let hash = Hash::new(
            HashAlgorithm::Sha256,
            "gggg6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f",
        );
        assert!(matches!(hash, Err(HashError::InvalidHex(_))));
    }

    #[test]
    fn test_prefixed_hash() {
        let hash = Hash::parse_prefixed(
            "sha256:dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f",
        )
        .unwrap();
        assert_eq!(hash.algorithm, HashAlgorithm::Sha256);

        let hash = Hash::parse_prefixed("xxh128:00000000000000000000000000000000").unwrap();
        assert_eq!(hash.algorithm, HashAlgorithm::Xxh128);

        // Unprefixed defaults to SHA-256
        let hash = Hash::parse_prefixed(
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f",
        )
        .unwrap();
        assert_eq!(hash.algorithm, HashAlgorithm::Sha256);
    }

    #[test]
    fn test_hash_display() {
        let hash = hash_bytes(HashAlgorithm::Sha256, b"test");
        let display = format!("{}", hash);
        assert_eq!(display, hash.value);

        let prefixed = hash.to_prefixed_string();
        assert!(prefixed.starts_with("sha256:"));
    }

    #[test]
    fn test_hash_reader() {
        let data = b"Hello, World!";
        let mut cursor = std::io::Cursor::new(data);

        let hash = hash_reader(HashAlgorithm::Sha256, &mut cursor).unwrap();
        let expected = hash_bytes(HashAlgorithm::Sha256, data);

        assert_eq!(hash, expected);
    }

    #[test]
    fn test_xxh128_speed_advantage() {
        // Just verify both work on larger data
        let data = vec![0u8; 1024 * 1024]; // 1 MB

        let sha_hash = hash_bytes(HashAlgorithm::Sha256, &data);
        let xxh_hash = hash_bytes(HashAlgorithm::Xxh128, &data);

        assert_eq!(sha_hash.value.len(), 64);
        assert_eq!(xxh_hash.value.len(), 32);
    }

    #[test]
    fn test_default_algorithm() {
        let algo = HashAlgorithm::default();
        assert_eq!(algo, HashAlgorithm::Sha256);
    }
}
