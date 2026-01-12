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

mod applier;
mod generator;
mod metrics;

pub use applier::DeltaApplier;
pub use generator::DeltaGenerator;
pub use metrics::{DeltaMetrics, MAX_DELTA_RATIO};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::filesystem::CasStore;
    use tempfile::TempDir;

    fn create_test_cas() -> (TempDir, CasStore) {
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();
        (temp_dir, cas)
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
