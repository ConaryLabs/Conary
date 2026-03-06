// conary-core/src/delta/applier.rs

//! Delta applier to reconstruct new version from old version + delta
//!
//! Uses zstd dictionary decompression with the old version as dictionary.

use crate::error::{Error, Result};
use crate::filesystem::CasStore;
use std::io::Read;
use std::path::Path;
use tracing::{debug, info};

/// Maximum size for decompressed delta output (2 GiB)
const MAX_DELTA_OUTPUT_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// Delta applier to reconstruct new version from old version + delta
pub struct DeltaApplier {
    cas: CasStore,
}

impl DeltaApplier {
    /// Create a new delta applier
    pub fn new(cas_root: &Path) -> Result<Self> {
        let cas = CasStore::new(cas_root)?;
        Ok(Self { cas })
    }

    /// Apply a delta to create new version
    ///
    /// # Arguments
    /// * `old_hash` - SHA-256 hash of old version (stored in CAS)
    /// * `delta_path` - Path to delta file
    /// * `expected_new_hash` - Expected SHA-256 hash of new version (for verification)
    ///
    /// # Returns
    /// The actual hash of the newly created version
    ///
    /// # Errors
    /// Returns error if hash mismatch detected
    pub fn apply_delta(
        &self,
        old_hash: &str,
        delta_path: &Path,
        expected_new_hash: &str,
    ) -> Result<String> {
        info!(
            "Applying delta to {} (expecting {})",
            old_hash.get(..8).unwrap_or(old_hash),
            expected_new_hash.get(..8).unwrap_or(expected_new_hash)
        );

        // Retrieve old version from CAS
        let old_content = self.cas.retrieve(old_hash)?;
        debug!("Old version retrieved: {} bytes", old_content.len());

        // Read delta file
        let delta = std::fs::read(delta_path)
            .map_err(|e| Error::IoError(format!("Failed to read delta file: {}", e)))?;

        debug!("Delta loaded: {} bytes", delta.len());

        // Decompress delta using old content as dictionary
        let new_content = self.decompress_with_dictionary(&delta, &old_content)?;
        debug!("New version reconstructed: {} bytes", new_content.len());

        // Store in CAS and get hash
        let actual_hash = self.cas.store(&new_content)?;

        // Verify hash matches expected
        if actual_hash != expected_new_hash {
            return Err(Error::ChecksumMismatch {
                expected: expected_new_hash.to_string(),
                actual: actual_hash,
            });
        }

        info!(
            "Delta applied successfully: {} bytes -> {} bytes",
            old_content.len(),
            new_content.len()
        );

        Ok(actual_hash)
    }

    /// Decompress data using dictionary decompression with size limit
    fn decompress_with_dictionary(&self, compressed: &[u8], dictionary: &[u8]) -> Result<Vec<u8>> {
        // Create decoder dictionary from old version (copied for 'static lifetime)
        let decoder_dict = zstd::dict::DecoderDictionary::copy(dictionary);

        // Decompress using the dictionary
        let mut decoder = zstd::Decoder::with_prepared_dictionary(compressed, &decoder_dict)
            .map_err(|e| Error::DeltaError(format!("Failed to create decoder: {}", e)))?;

        let mut decompressed = Vec::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = decoder
                .read(&mut buf)
                .map_err(|e| Error::DeltaError(format!("Failed to read decompressed data: {}", e)))?;
            if n == 0 {
                break;
            }
            decompressed.extend_from_slice(&buf[..n]);
            if decompressed.len() as u64 > MAX_DELTA_OUTPUT_SIZE {
                return Err(Error::DeltaError(format!(
                    "Delta output exceeds maximum size limit ({} bytes)",
                    MAX_DELTA_OUTPUT_SIZE
                )));
            }
        }

        Ok(decompressed)
    }
}
