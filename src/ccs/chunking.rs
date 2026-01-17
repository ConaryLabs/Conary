// src/ccs/chunking.rs

//! Content-Defined Chunking (CDC) for efficient deduplication and delta updates.
//!
//! This module uses FastCDC to split files into variable-size chunks based on
//! content boundaries. The key property: if you change one byte in a 100MB file,
//! only 1-2 chunks change (not the entire file).
//!
//! This enables:
//! - Cross-package deduplication (glibc chunks shared by everything)
//! - Implicit delta updates (download only missing chunks)
//! - Efficient repository storage (chunks stored once, referenced many times)

use anyhow::{Context, Result};
use fastcdc::v2020::FastCDC;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Default chunk size parameters (in bytes)
/// These should be chosen carefully and kept stable - changing them
/// invalidates all existing chunks in the store.
pub const MIN_CHUNK_SIZE: u32 = 16 * 1024;      // 16 KB minimum
pub const AVG_CHUNK_SIZE: u32 = 64 * 1024;      // 64 KB average (sweet spot)
pub const MAX_CHUNK_SIZE: u32 = 256 * 1024;     // 256 KB maximum

/// A single chunk produced by CDC
#[derive(Debug, Clone)]
pub struct Chunk {
    /// SHA-256 hash of the chunk content
    pub hash: [u8; 32],
    /// Offset in the original file
    pub offset: u64,
    /// Length of the chunk
    pub length: u32,
    /// The actual data (only populated during chunking, not storage)
    pub data: Vec<u8>,
}

impl Chunk {
    /// Get the hash as a hex string
    pub fn hash_hex(&self) -> String {
        hex::encode(self.hash)
    }

    /// Get the CAS-style path for this chunk (e.g., "ab/cdef1234...")
    pub fn cas_path(&self) -> PathBuf {
        let hex = self.hash_hex();
        PathBuf::from(&hex[..2]).join(&hex[2..])
    }
}

/// Result of chunking a file
#[derive(Debug)]
pub struct ChunkedFile {
    /// Original file path
    pub path: PathBuf,
    /// Original file size
    pub size: u64,
    /// SHA-256 of the entire file (for verification)
    pub file_hash: [u8; 32],
    /// Ordered list of chunks
    pub chunks: Vec<Chunk>,
}

impl ChunkedFile {
    /// Get total number of chunks
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Get list of unique chunk hashes
    pub fn unique_hashes(&self) -> Vec<[u8; 32]> {
        let mut seen = std::collections::HashSet::new();
        self.chunks
            .iter()
            .filter(|c| seen.insert(c.hash))
            .map(|c| c.hash)
            .collect()
    }

    /// Calculate how many bytes would need to be downloaded if we already
    /// have the chunks in `existing_hashes`
    pub fn bytes_needed(&self, existing_hashes: &std::collections::HashSet<[u8; 32]>) -> u64 {
        self.chunks
            .iter()
            .filter(|c| !existing_hashes.contains(&c.hash))
            .map(|c| u64::from(c.length))
            .sum()
    }
}

/// Content-Defined Chunker using FastCDC
pub struct Chunker {
    min_size: u32,
    avg_size: u32,
    max_size: u32,
}

impl Default for Chunker {
    fn default() -> Self {
        Self::new()
    }
}

impl Chunker {
    /// Create a new chunker with default parameters
    pub fn new() -> Self {
        Self {
            min_size: MIN_CHUNK_SIZE,
            avg_size: AVG_CHUNK_SIZE,
            max_size: MAX_CHUNK_SIZE,
        }
    }

    /// Create a chunker with custom parameters
    pub fn with_sizes(min: u32, avg: u32, max: u32) -> Self {
        Self {
            min_size: min,
            avg_size: avg,
            max_size: max,
        }
    }

    /// Chunk a byte slice
    pub fn chunk_bytes(&self, data: &[u8]) -> Vec<Chunk> {
        let chunker = FastCDC::new(data, self.min_size, self.avg_size, self.max_size);
        let mut chunks = Vec::new();

        for entry in chunker {
            let chunk_data = &data[entry.offset..entry.offset + entry.length];
            let hash = Sha256::digest(chunk_data);

            chunks.push(Chunk {
                hash: hash.into(),
                offset: entry.offset as u64,
                length: entry.length as u32,
                data: chunk_data.to_vec(),
            });
        }

        chunks
    }

    /// Chunk a file
    pub fn chunk_file(&self, path: &Path) -> Result<ChunkedFile> {
        let mut file = File::open(path)
            .with_context(|| format!("Failed to open file: {}", path.display()))?;

        let metadata = file.metadata()?;
        let size = metadata.len();

        // Read entire file into memory (for files up to a few hundred MB this is fine)
        // For very large files, we'd want a streaming approach
        let mut data = Vec::with_capacity(size as usize);
        file.read_to_end(&mut data)?;

        // Calculate file hash
        let file_hash: [u8; 32] = Sha256::digest(&data).into();

        // Chunk the data
        let chunks = self.chunk_bytes(&data);

        Ok(ChunkedFile {
            path: path.to_path_buf(),
            size,
            file_hash,
            chunks,
        })
    }
}

/// Chunk store for persisting and retrieving chunks
pub struct ChunkStore {
    /// Root directory for chunk storage
    root: PathBuf,
}

impl ChunkStore {
    /// Create a new chunk store at the given path
    pub fn new(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("Failed to create chunk store: {}", root.display()))?;

        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// Get the full path for a chunk hash
    fn chunk_path(&self, hash: &[u8; 32]) -> PathBuf {
        let hex = hex::encode(hash);
        self.root.join(&hex[..2]).join(&hex[2..])
    }

    /// Check if a chunk exists in the store
    pub fn has_chunk(&self, hash: &[u8; 32]) -> bool {
        self.chunk_path(hash).exists()
    }

    /// Store a chunk (idempotent - won't overwrite if exists)
    pub fn store_chunk(&self, chunk: &Chunk) -> Result<bool> {
        let path = self.chunk_path(&chunk.hash);

        if path.exists() {
            return Ok(false); // Already exists
        }

        // Create parent directory
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write to temp file then rename (atomic)
        let temp_path = path.with_extension("tmp");
        let mut file = File::create(&temp_path)?;
        file.write_all(&chunk.data)?;
        file.sync_all()?;

        std::fs::rename(&temp_path, &path)?;

        Ok(true) // Newly stored
    }

    /// Retrieve a chunk by hash
    pub fn get_chunk(&self, hash: &[u8; 32]) -> Result<Vec<u8>> {
        let path = self.chunk_path(hash);
        std::fs::read(&path)
            .with_context(|| format!("Failed to read chunk: {}", path.display()))
    }

    /// Store all chunks from a chunked file, returning count of new chunks
    pub fn store_chunked_file(&self, chunked: &ChunkedFile) -> Result<StoreStats> {
        let mut stats = StoreStats::default();

        for chunk in &chunked.chunks {
            if self.store_chunk(chunk)? {
                stats.new_chunks += 1;
                stats.new_bytes += u64::from(chunk.length);
            } else {
                stats.existing_chunks += 1;
                stats.deduped_bytes += u64::from(chunk.length);
            }
        }

        stats.total_chunks = chunked.chunks.len();
        stats.file_size = chunked.size;

        Ok(stats)
    }

    /// Reassemble a file from its chunk list
    pub fn reassemble(&self, chunks: &[[u8; 32]]) -> Result<Vec<u8>> {
        let mut data = Vec::new();

        for hash in chunks {
            let chunk_data = self.get_chunk(hash)?;
            data.extend_from_slice(&chunk_data);
        }

        Ok(data)
    }

    /// Get all chunk hashes in the store
    pub fn list_chunks(&self) -> Result<Vec<[u8; 32]>> {
        let mut hashes = Vec::new();

        for entry in walkdir::WalkDir::new(&self.root)
            .min_depth(2)
            .max_depth(2)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                // Reconstruct hash from path
                if let (Some(prefix), Some(suffix)) = (
                    entry.path().parent().and_then(|p| p.file_name()),
                    entry.path().file_name(),
                ) {
                    let hex = format!(
                        "{}{}",
                        prefix.to_string_lossy(),
                        suffix.to_string_lossy()
                    );
                    if hex.len() == 64
                        && let Ok(bytes) = hex::decode(&hex)
                        && bytes.len() == 32
                    {
                        let mut hash = [0u8; 32];
                        hash.copy_from_slice(&bytes);
                        hashes.push(hash);
                    }
                }
            }
        }

        Ok(hashes)
    }
}

/// Statistics from storing chunks
#[derive(Debug, Default)]
pub struct StoreStats {
    /// Total chunks in the file
    pub total_chunks: usize,
    /// Chunks that were newly stored
    pub new_chunks: usize,
    /// Chunks that already existed (deduped)
    pub existing_chunks: usize,
    /// Original file size
    pub file_size: u64,
    /// Bytes newly written to store
    pub new_bytes: u64,
    /// Bytes saved by deduplication
    pub deduped_bytes: u64,
}

impl StoreStats {
    /// Calculate deduplication ratio (0.0 = no dedup, 1.0 = 100% dedup)
    pub fn dedup_ratio(&self) -> f64 {
        if self.file_size == 0 {
            return 0.0;
        }
        self.deduped_bytes as f64 / self.file_size as f64
    }
}

/// Compare two chunked files and calculate the delta
pub fn calculate_delta(old: &ChunkedFile, new: &ChunkedFile) -> DeltaStats {
    let old_hashes: std::collections::HashSet<_> = old.chunks.iter().map(|c| c.hash).collect();
    let new_hashes: std::collections::HashSet<_> = new.chunks.iter().map(|c| c.hash).collect();

    let shared: std::collections::HashSet<_> = old_hashes.intersection(&new_hashes).collect();

    let mut shared_bytes = 0u64;
    let mut new_bytes = 0u64;

    for chunk in &new.chunks {
        if shared.contains(&chunk.hash) {
            shared_bytes += u64::from(chunk.length);
        } else {
            new_bytes += u64::from(chunk.length);
        }
    }

    DeltaStats {
        old_size: old.size,
        new_size: new.size,
        old_chunks: old.chunks.len(),
        new_chunks: new.chunks.len(),
        shared_chunks: shared.len(),
        shared_bytes,
        new_bytes,
    }
}

/// Statistics about the delta between two versions
#[derive(Debug)]
pub struct DeltaStats {
    pub old_size: u64,
    pub new_size: u64,
    pub old_chunks: usize,
    pub new_chunks: usize,
    pub shared_chunks: usize,
    pub shared_bytes: u64,
    pub new_bytes: u64,
}

impl DeltaStats {
    /// Calculate bandwidth savings (what percentage of new file is shared)
    pub fn savings_ratio(&self) -> f64 {
        if self.new_size == 0 {
            return 0.0;
        }
        self.shared_bytes as f64 / self.new_size as f64
    }

    /// What would actually need to be downloaded
    pub fn download_size(&self) -> u64 {
        self.new_bytes
    }
}

/// Manifest entry for a chunked file (for serialization)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChunkManifestEntry {
    /// File path (relative)
    pub path: String,
    /// File size
    pub size: u64,
    /// SHA-256 of complete file
    pub file_hash: String,
    /// Ordered list of chunk hashes
    pub chunks: Vec<String>,
}

impl From<&ChunkedFile> for ChunkManifestEntry {
    fn from(cf: &ChunkedFile) -> Self {
        Self {
            path: cf.path.to_string_lossy().to_string(),
            size: cf.size,
            file_hash: hex::encode(cf.file_hash),
            chunks: cf.chunks.iter().map(|c| c.hash_hex()).collect(),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_chunk_bytes_basic() {
        let chunker = Chunker::new();

        // Create some test data (needs to be larger than min chunk size)
        let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();

        let chunks = chunker.chunk_bytes(&data);

        // Should have at least one chunk
        assert!(!chunks.is_empty());

        // Total length should equal original
        let total_len: u64 = chunks.iter().map(|c| u64::from(c.length)).sum();
        assert_eq!(total_len, data.len() as u64);

        // Chunks should be contiguous
        let mut offset = 0u64;
        for chunk in &chunks {
            assert_eq!(chunk.offset, offset);
            offset += u64::from(chunk.length);
        }
    }

    #[test]
    fn test_same_content_same_chunks() {
        let chunker = Chunker::new();

        // Same data should produce same chunks
        let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();

        let chunks1 = chunker.chunk_bytes(&data);
        let chunks2 = chunker.chunk_bytes(&data);

        assert_eq!(chunks1.len(), chunks2.len());
        for (c1, c2) in chunks1.iter().zip(chunks2.iter()) {
            assert_eq!(c1.hash, c2.hash);
        }
    }

    /// Generate pseudo-random data using a simple LCG
    fn pseudo_random_data(seed: u64, len: usize) -> Vec<u8> {
        let mut x = seed;
        (0..len)
            .map(|_| {
                // LCG from Knuth MMIX
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                (x >> 32) as u8
            })
            .collect()
    }

    #[test]
    fn test_small_change_few_chunks_differ() {
        let chunker = Chunker::new();

        // Create realistic pseudo-random test data
        let data1 = pseudo_random_data(42, 500_000);
        let mut data2 = data1.clone();

        // Change one byte near the middle
        data2[250_000] = data2[250_000].wrapping_add(1);

        let chunks1 = chunker.chunk_bytes(&data1);
        let chunks2 = chunker.chunk_bytes(&data2);

        // Count differing chunks
        let hashes1: std::collections::HashSet<_> = chunks1.iter().map(|c| c.hash).collect();
        let hashes2: std::collections::HashSet<_> = chunks2.iter().map(|c| c.hash).collect();

        let shared = hashes1.intersection(&hashes2).count();
        let different = hashes1.symmetric_difference(&hashes2).count();

        // CDC property: a single byte change should only affect ~1 chunk
        // Most chunks should be shared
        println!(
            "Chunks: {} original, {} modified, {} shared, {} different",
            chunks1.len(),
            chunks2.len(),
            shared,
            different
        );

        // At least half of chunks should be shared (conservative bound)
        assert!(
            shared >= chunks1.len().saturating_sub(5),
            "Most chunks should be shared: {} shared of {}",
            shared,
            chunks1.len()
        );
    }

    #[test]
    fn test_chunk_store() {
        let temp_dir = TempDir::new().unwrap();
        let store = ChunkStore::new(temp_dir.path()).unwrap();
        let chunker = Chunker::new();

        let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
        let chunks = chunker.chunk_bytes(&data);

        // Store first chunk
        let chunk = &chunks[0];
        assert!(!store.has_chunk(&chunk.hash));

        let stored = store.store_chunk(chunk).unwrap();
        assert!(stored); // Should be newly stored
        assert!(store.has_chunk(&chunk.hash));

        // Store again - should be idempotent
        let stored_again = store.store_chunk(chunk).unwrap();
        assert!(!stored_again); // Already exists

        // Retrieve and verify
        let retrieved = store.get_chunk(&chunk.hash).unwrap();
        assert_eq!(retrieved, chunk.data);
    }

    #[test]
    fn test_chunk_file() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.bin");

        // Create test file
        let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
        std::fs::write(&test_file, &data).unwrap();

        let chunker = Chunker::new();
        let chunked = chunker.chunk_file(&test_file).unwrap();

        assert_eq!(chunked.size, data.len() as u64);
        assert!(!chunked.chunks.is_empty());

        // Verify file hash
        let expected_hash: [u8; 32] = sha2::Sha256::digest(&data).into();
        assert_eq!(chunked.file_hash, expected_hash);
    }

    #[test]
    fn test_delta_calculation() {
        let chunker = Chunker::new();

        // Use pseudo-random data for realistic CDC behavior
        let data1 = pseudo_random_data(123, 500_000);
        let mut data2 = data1.clone();

        // Modify a small portion (simulates a code change)
        for i in 250_000..250_200 {
            data2[i] = 0xFF;
        }

        let temp_dir = TempDir::new().unwrap();
        let file1 = temp_dir.path().join("v1.bin");
        let file2 = temp_dir.path().join("v2.bin");

        std::fs::write(&file1, &data1).unwrap();
        std::fs::write(&file2, &data2).unwrap();

        let chunked1 = chunker.chunk_file(&file1).unwrap();
        let chunked2 = chunker.chunk_file(&file2).unwrap();

        let delta = calculate_delta(&chunked1, &chunked2);

        println!("Delta stats:");
        println!("  Old: {} bytes, {} chunks", delta.old_size, delta.old_chunks);
        println!("  New: {} bytes, {} chunks", delta.new_size, delta.new_chunks);
        println!(
            "  Shared: {} chunks, {} bytes",
            delta.shared_chunks, delta.shared_bytes
        );
        println!(
            "  Download: {} bytes ({:.1}% savings)",
            delta.download_size(),
            delta.savings_ratio() * 100.0
        );

        // Should have significant savings - a 200-byte change in 500KB
        // should only affect 1-2 chunks
        assert!(
            delta.savings_ratio() > 0.5,
            "Should share >50% of content: {:.1}% savings",
            delta.savings_ratio() * 100.0
        );
    }

    #[test]
    fn test_reassemble() {
        let temp_dir = TempDir::new().unwrap();
        let store = ChunkStore::new(&temp_dir.path().join("chunks")).unwrap();
        let chunker = Chunker::new();

        // Create and chunk test data
        let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
        let chunks = chunker.chunk_bytes(&data);

        // Store all chunks
        for chunk in &chunks {
            store.store_chunk(chunk).unwrap();
        }

        // Reassemble
        let hashes: Vec<[u8; 32]> = chunks.iter().map(|c| c.hash).collect();
        let reassembled = store.reassemble(&hashes).unwrap();

        assert_eq!(reassembled, data);
    }

    #[test]
    fn test_store_stats() {
        let temp_dir = TempDir::new().unwrap();
        let store = ChunkStore::new(&temp_dir.path().join("chunks")).unwrap();
        let chunker = Chunker::new();

        // Create test file
        let test_file = temp_dir.path().join("test.bin");
        let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
        std::fs::write(&test_file, &data).unwrap();

        let chunked = chunker.chunk_file(&test_file).unwrap();

        // First store - all new
        let stats1 = store.store_chunked_file(&chunked).unwrap();
        assert_eq!(stats1.existing_chunks, 0);
        assert!(stats1.new_chunks > 0);
        assert_eq!(stats1.dedup_ratio(), 0.0);

        // Second store - all deduped
        let stats2 = store.store_chunked_file(&chunked).unwrap();
        assert_eq!(stats2.new_chunks, 0);
        assert!(stats2.existing_chunks > 0);
        assert!((stats2.dedup_ratio() - 1.0).abs() < 0.01);
    }

    /// Test CDC with the conary binary itself (run with --ignored)
    #[test]
    #[ignore]
    fn test_cdc_real_binary() {
        use std::collections::HashSet;

        let binary_path = std::path::Path::new("target/release/conary");
        if !binary_path.exists() {
            println!("Skipping: binary not found at {:?}", binary_path);
            return;
        }

        let data = std::fs::read(binary_path).unwrap();
        println!("\nTesting CDC on real binary:");
        println!("  Binary: {:?}", binary_path);
        println!(
            "  Size: {} bytes ({:.2} MB)",
            data.len(),
            data.len() as f64 / 1_048_576.0
        );

        let chunker = Chunker::new();
        let chunks = chunker.chunk_bytes(&data);

        println!("\n  Chunk distribution:");
        println!("  Total chunks: {}", chunks.len());

        let sizes: Vec<u32> = chunks.iter().map(|c| c.length).collect();
        let total: u64 = sizes.iter().map(|&s| s as u64).sum();
        let avg = total as usize / sizes.len();
        let min = *sizes.iter().min().unwrap() as usize;
        let max = *sizes.iter().max().unwrap() as usize;

        println!("  Min chunk: {} bytes ({:.1} KB)", min, min as f64 / 1024.0);
        println!("  Max chunk: {} bytes ({:.1} KB)", max, max as f64 / 1024.0);
        println!("  Avg chunk: {} bytes ({:.1} KB)", avg, avg as f64 / 1024.0);

        // Simulate a small change (like a version string update)
        let mut modified = data.clone();
        if modified.len() > 10000 {
            // Change a few bytes to simulate a patch
            for i in 10000..10010 {
                modified[i] = modified[i].wrapping_add(1);
            }
        }

        let chunks2 = chunker.chunk_bytes(&modified);

        // Compare hashes
        let hashes1: HashSet<_> = chunks.iter().map(|c| c.hash).collect();
        let hashes2: HashSet<_> = chunks2.iter().map(|c| c.hash).collect();

        let shared = hashes1.intersection(&hashes2).count();
        let savings = shared as f64 / chunks.len() as f64;
        let download_chunks = chunks.len().saturating_sub(shared);
        let download_bytes: u64 = chunks2
            .iter()
            .filter(|c| !hashes1.contains(&c.hash))
            .map(|c| c.length as u64)
            .sum();

        println!("\n  Simulated 10-byte patch:");
        println!("    Original chunks: {}", chunks.len());
        println!("    Modified chunks: {}", chunks2.len());
        println!("    Shared chunks: {} ({:.1}%)", shared, savings * 100.0);
        println!(
            "    Would download: {} chunks, {} bytes ({:.1} KB)",
            download_chunks,
            download_bytes,
            download_bytes as f64 / 1024.0
        );
        println!(
            "    Bandwidth savings: {:.1}%",
            (1.0 - download_bytes as f64 / data.len() as f64) * 100.0
        );

        // CDC should provide significant savings
        assert!(
            savings > 0.9,
            "Should share >90% of chunks for small change"
        );
    }
}
