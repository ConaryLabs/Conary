// src/delta/applier.rs

//! Delta applier to reconstruct new version from old version + delta
//!
//! Uses zstd dictionary decompression with the old version as dictionary.

use crate::error::{Error, Result};
use crate::filesystem::CasStore;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tracing::{debug, info};

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
            &old_hash[..8],
            &expected_new_hash[..8]
        );

        // Retrieve old version from CAS
        let old_content = self.cas.retrieve(old_hash)?;
        debug!("Old version retrieved: {} bytes", old_content.len());

        // Read delta file
        let mut delta_file = File::open(delta_path).map_err(|e| {
            Error::IoError(format!("Failed to open delta file: {}", e))
        })?;
        let mut delta = Vec::new();
        delta_file.read_to_end(&mut delta).map_err(|e| {
            Error::IoError(format!("Failed to read delta file: {}", e))
        })?;

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

    /// Decompress data using dictionary decompression
    fn decompress_with_dictionary(&self, compressed: &[u8], dictionary: &[u8]) -> Result<Vec<u8>> {
        // Create decoder dictionary from old version (copied for 'static lifetime)
        let decoder_dict = zstd::dict::DecoderDictionary::copy(dictionary);

        // Decompress using the dictionary
        let mut decoder = zstd::Decoder::with_prepared_dictionary(compressed, &decoder_dict)
            .map_err(|e| Error::DeltaError(format!("Failed to create decoder: {}", e)))?;

        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .map_err(|e| Error::DeltaError(format!("Failed to read decompressed data: {}", e)))?;

        Ok(decompressed)
    }
}
