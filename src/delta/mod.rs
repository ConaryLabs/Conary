// src/delta/mod.rs

//! Delta compression for efficient package updates
//!
//! This module provides delta generation and application using zstd dictionary compression.
//! Deltas allow downloading only the changed portions of files, significantly reducing
//! bandwidth usage for package updates.
//!
//! # Architecture
//!
//! - **DeltaGenerator**: Creates compressed deltas using old version as dictionary
//! - **DeltaApplier**: Applies deltas to old versions to produce new versions
//! - **DeltaMetrics**: Tracks bandwidth savings and delta effectiveness
//!
//! # Delta Format
//!
//! Deltas are created using zstd compression with the old file as a dictionary:
//! ```text
//! delta = zstd_compress(new_content, dictionary=old_content)
//! ```
//!
//! This provides excellent compression for similar files (e.g., updated binaries).

use crate::error::{Error, Result};
use crate::filesystem::CasStore;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tracing::{debug, info};

/// Default zstd compression level (3 = fast, good compression)
const COMPRESSION_LEVEL: i32 = 3;

/// Maximum delta size as percentage of full file (fallback if delta too large)
const MAX_DELTA_RATIO: f64 = 0.9;

/// Delta generation metrics
#[derive(Debug, Clone)]
pub struct DeltaMetrics {
    pub old_size: u64,
    pub new_size: u64,
    pub delta_size: u64,
    pub compression_ratio: f64,
    pub bandwidth_saved: i64,
}

impl DeltaMetrics {
    /// Calculate metrics from sizes
    pub fn new(old_size: u64, new_size: u64, delta_size: u64) -> Self {
        let compression_ratio = if new_size > 0 {
            delta_size as f64 / new_size as f64
        } else {
            1.0
        };

        let bandwidth_saved = new_size as i64 - delta_size as i64;

        Self {
            old_size,
            new_size,
            delta_size,
            compression_ratio,
            bandwidth_saved,
        }
    }

    /// Check if delta is worthwhile (smaller than threshold)
    pub fn is_worthwhile(&self) -> bool {
        self.compression_ratio < MAX_DELTA_RATIO
    }

    /// Get percentage of bandwidth saved
    pub fn savings_percentage(&self) -> f64 {
        if self.new_size > 0 {
            (self.bandwidth_saved as f64 / self.new_size as f64) * 100.0
        } else {
            0.0
        }
    }
}

/// Delta generator using zstd dictionary compression
pub struct DeltaGenerator {
    cas: CasStore,
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_cas() -> (TempDir, CasStore) {
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();
        (temp_dir, cas)
    }

    #[test]
    fn test_delta_metrics_calculation() {
        let metrics = DeltaMetrics::new(1000, 1200, 300);

        assert_eq!(metrics.old_size, 1000);
        assert_eq!(metrics.new_size, 1200);
        assert_eq!(metrics.delta_size, 300);
        assert_eq!(metrics.compression_ratio, 0.25); // 300/1200
        assert_eq!(metrics.bandwidth_saved, 900); // 1200 - 300
        assert!((metrics.savings_percentage() - 75.0).abs() < 0.1);
        assert!(metrics.is_worthwhile());
    }

    #[test]
    fn test_delta_metrics_not_worthwhile() {
        // Delta is 95% of original size - not worthwhile
        let metrics = DeltaMetrics::new(1000, 1000, 950);
        assert!(!metrics.is_worthwhile());
    }

    #[test]
    fn test_delta_generation_and_application() {
        let (temp, cas) = create_test_cas();
        let delta_path = temp.path().join("test.delta");

        // Store old and new versions
        let old_content = b"Hello, World! This is the old version.";
        let new_content = b"Hello, World! This is the NEW version with more text!";

        let old_hash = cas.store(old_content).unwrap();
        let new_hash = cas.store(new_content).unwrap();

        // Generate delta (use same CAS root)
        let generator = DeltaGenerator::new(temp.path()).unwrap();
        let metrics = generator
            .generate_delta(&old_hash, &new_hash, &delta_path)
            .unwrap();

        // Verify metrics
        assert_eq!(metrics.old_size, old_content.len() as u64);
        assert_eq!(metrics.new_size, new_content.len() as u64);
        assert!(metrics.delta_size > 0);
        assert!(metrics.delta_size < new_content.len() as u64); // Should be smaller

        // Apply delta (use same CAS root)
        let applier = DeltaApplier::new(temp.path()).unwrap();
        let result_hash = applier
            .apply_delta(&old_hash, &delta_path, &new_hash)
            .unwrap();

        // Verify hash matches
        assert_eq!(result_hash, new_hash);

        // Verify content matches
        let result_content = cas.retrieve(&result_hash).unwrap();
        assert_eq!(result_content, new_content);
    }

    #[test]
    fn test_delta_application_hash_mismatch() {
        let (temp, cas) = create_test_cas();
        let delta_path = temp.path().join("test.delta");

        // Store old and new versions
        let old_content = b"Old content";
        let new_content = b"New content";

        let old_hash = cas.store(old_content).unwrap();
        let new_hash = cas.store(new_content).unwrap();

        // Generate delta (use same CAS root)
        let generator = DeltaGenerator::new(temp.path()).unwrap();
        generator
            .generate_delta(&old_hash, &new_hash, &delta_path)
            .unwrap();

        // Try to apply with wrong expected hash (use same CAS root)
        let applier = DeltaApplier::new(temp.path()).unwrap();
        let wrong_hash = "0000000000000000000000000000000000000000000000000000000000000000";
        let result = applier.apply_delta(&old_hash, &delta_path, wrong_hash);

        // Should fail with checksum mismatch
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::ChecksumMismatch { .. } => (),
            _ => panic!("Expected ChecksumMismatch error"),
        }
    }

    #[test]
    fn test_delta_with_large_difference() {
        let (temp, cas) = create_test_cas();
        let delta_path = temp.path().join("test.delta");

        // Create significantly different content
        let old_content = vec![0u8; 10000]; // 10KB of zeros
        let new_content = vec![255u8; 10000]; // 10KB of 255s

        let old_hash = cas.store(&old_content).unwrap();
        let new_hash = cas.store(&new_content).unwrap();

        // Generate and apply delta (use same CAS root)
        let generator = DeltaGenerator::new(temp.path()).unwrap();
        let metrics = generator
            .generate_delta(&old_hash, &new_hash, &delta_path)
            .unwrap();

        // Even with large difference, should still work
        assert!(metrics.delta_size > 0);

        let applier = DeltaApplier::new(temp.path()).unwrap();
        let result_hash = applier
            .apply_delta(&old_hash, &delta_path, &new_hash)
            .unwrap();

        assert_eq!(result_hash, new_hash);
    }

    #[test]
    fn test_delta_with_similar_content() {
        let (temp, _cas) = create_test_cas();
        let delta_path = temp.path().join("test.delta");

        // Create similar content (simulating a patch update)
        let old_content = "fn main() { println!(\"version 1.0\"); }".repeat(100);
        let new_content = "fn main() { println!(\"version 1.1\"); }".repeat(100);

        // Store using the generator's CAS (same root)
        let generator = DeltaGenerator::new(temp.path()).unwrap();
        let old_hash = generator.cas.store(old_content.as_bytes()).unwrap();
        let new_hash = generator.cas.store(new_content.as_bytes()).unwrap();

        // Generate delta
        let metrics = generator
            .generate_delta(&old_hash, &new_hash, &delta_path)
            .unwrap();

        // Should achieve good compression on similar content
        assert!(metrics.is_worthwhile());
        assert!(metrics.compression_ratio < 0.5); // Should be less than 50%

        // Apply and verify (use same CAS root)
        let applier = DeltaApplier::new(temp.path()).unwrap();
        let result_hash = applier
            .apply_delta(&old_hash, &delta_path, &new_hash)
            .unwrap();

        assert_eq!(result_hash, new_hash);
    }
}
