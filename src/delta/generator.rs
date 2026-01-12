// src/delta/generator.rs

//! Delta generator using zstd dictionary compression
//!
//! Creates compressed deltas using the old version as a dictionary,
//! providing excellent compression for similar files.

use crate::error::{Error, Result};
use crate::filesystem::CasStore;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use tracing::{debug, info};

use super::DeltaMetrics;

/// Default zstd compression level (3 = fast, good compression)
const COMPRESSION_LEVEL: i32 = 3;

/// Delta generator using zstd dictionary compression
pub struct DeltaGenerator {
    pub(crate) cas: CasStore,
}

impl DeltaGenerator {
    /// Create a new delta generator
    pub fn new(cas_root: &Path) -> Result<Self> {
        let cas = CasStore::new(cas_root)?;
        Ok(Self { cas })
    }

    /// Generate a delta from old version to new version
    ///
    /// # Arguments
    /// * `old_hash` - SHA-256 hash of old version (stored in CAS)
    /// * `new_hash` - SHA-256 hash of new version (stored in CAS)
    /// * `output_path` - Where to write the delta file
    ///
    /// # Returns
    /// Delta metrics (sizes and compression ratio)
    pub fn generate_delta(
        &self,
        old_hash: &str,
        new_hash: &str,
        output_path: &Path,
    ) -> Result<DeltaMetrics> {
        info!(
            "Generating delta from {} to {}",
            &old_hash[..8],
            &new_hash[..8]
        );

        // Retrieve old and new versions from CAS
        let old_content = self.cas.retrieve(old_hash)?;
        let new_content = self.cas.retrieve(new_hash)?;

        debug!(
            "Old version: {} bytes, New version: {} bytes",
            old_content.len(),
            new_content.len()
        );

        // Generate delta using zstd with old content as dictionary
        let delta = self.compress_with_dictionary(&new_content, &old_content)?;

        // Write delta to output file
        let mut file = File::create(output_path).map_err(|e| {
            Error::IoError(format!("Failed to create delta file: {}", e))
        })?;
        file.write_all(&delta).map_err(|e| {
            Error::IoError(format!("Failed to write delta file: {}", e))
        })?;

        // Calculate metrics
        let metrics = DeltaMetrics::new(
            old_content.len() as u64,
            new_content.len() as u64,
            delta.len() as u64,
        );

        info!(
            "Delta generated: {} bytes ({:.1}% of original, {:.1}% saved)",
            metrics.delta_size,
            metrics.compression_ratio * 100.0,
            metrics.savings_percentage()
        );

        Ok(metrics)
    }

    /// Compress data using dictionary compression
    fn compress_with_dictionary(&self, data: &[u8], dictionary: &[u8]) -> Result<Vec<u8>> {
        // Create encoder dictionary from old version (copied for 'static lifetime)
        let encoder_dict = zstd::dict::EncoderDictionary::copy(dictionary, COMPRESSION_LEVEL);

        // Compress new version using the dictionary
        let mut encoder = zstd::Encoder::with_prepared_dictionary(Vec::new(), &encoder_dict)
            .map_err(|e| Error::DeltaError(format!("Failed to create encoder: {}", e)))?;

        encoder
            .write_all(data)
            .map_err(|e| Error::DeltaError(format!("Failed to write data: {}", e)))?;

        encoder
            .finish()
            .map_err(|e| Error::DeltaError(format!("Failed to finish compression: {}", e)))
    }
}
