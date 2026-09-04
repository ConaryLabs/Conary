// crates/conary-core/src/ccs/chunking.rs

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

use anyhow::{Context, Result, bail};
use fastcdc::v2020::{FastCDC, StreamCDC};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// One ordered content-object reference produced by the canonical chunker.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkReference {
    pub sha256: String,
    pub size: u32,
}

/// Default chunk size parameters (in bytes)
/// These should be chosen carefully and kept stable - changing them
/// invalidates all existing chunks in the store.
pub const MIN_CHUNK_SIZE: u32 = 16 * 1024; // 16 KB minimum
pub const AVG_CHUNK_SIZE: u32 = 64 * 1024; // 64 KB average (sweet spot)
pub const MAX_CHUNK_SIZE: u32 = 256 * 1024; // 256 KB maximum

/// A single chunk produced by CDC
#[derive(Debug, Clone)]
pub struct Chunk {
    /// SHA-256 hash of the chunk content
    hash: [u8; 32],
    /// Offset in the original file
    offset: u64,
    /// Length of the chunk
    length: u32,
    /// The actual data (only populated during chunking, not storage)
    data: Vec<u8>,
}

impl Chunk {
    /// SHA-256 identity computed over this chunk's exact bytes by the chunker.
    #[must_use]
    pub const fn hash(&self) -> &[u8; 32] {
        &self.hash
    }

    /// Contiguous byte offset in the source stream.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Exact number of authenticated bytes in this chunk.
    #[must_use]
    pub const fn length(&self) -> u32 {
        self.length
    }

    /// Exact bytes bound to [`Self::hash`].
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Get the hash as a hex string
    pub fn hash_hex(&self) -> String {
        hex::encode(self.hash)
    }

    /// Get the CAS-style path for this chunk (e.g., "ab/cdef1234...")
    pub fn cas_path(&self) -> PathBuf {
        crate::filesystem::object_path(Path::new(""), &self.hash_hex())
            .expect("hex-encoded chunk hash should always produce a valid CAS path")
    }

    /// Project the streamed chunk into signed content authority.
    pub fn reference(&self) -> ChunkReference {
        ChunkReference {
            sha256: self.hash_hex(),
            size: self.length,
        }
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
        let chunker = FastCDC::new(
            data,
            self.min_size as usize,
            self.avg_size as usize,
            self.max_size as usize,
        );
        let mut chunks = Vec::new();

        for entry in chunker {
            let chunk_data = &data[entry.offset..entry.offset + entry.length];
            let hash = crate::hash::sha256_bytes(chunk_data);

            chunks.push(Chunk {
                hash,
                offset: entry.offset as u64,
                length: entry.length as u32,
                data: chunk_data.to_vec(),
            });
        }

        chunks
    }

    /// Stream chunks from a reader through a bounded buffer.
    ///
    /// The visitor receives one complete chunk at a time, so callers can persist
    /// or otherwise consume arbitrarily large artifacts without retaining the
    /// artifact or all of its chunks in memory. The returned size is the exact
    /// number of contiguous source bytes processed.
    pub fn visit_reader_chunks<R, F>(&self, reader: R, mut visit: F) -> Result<u64>
    where
        R: Read,
        F: FnMut(&Chunk) -> Result<()>,
    {
        let chunker = StreamCDC::new(
            reader,
            self.min_size as usize,
            self.avg_size as usize,
            self.max_size as usize,
        );
        let mut processed = 0_u64;

        for entry in chunker {
            let entry = entry.context("stream content-defined chunk")?;
            if entry.offset != processed {
                bail!(
                    "streamed chunk offset is not contiguous: expected {processed}, got {}",
                    entry.offset
                );
            }
            if entry.data.len() != entry.length {
                bail!(
                    "streamed chunk length disagrees with its data: {} != {}",
                    entry.length,
                    entry.data.len()
                );
            }
            let length =
                u32::try_from(entry.length).context("streamed chunk length exceeds u32")?;
            let chunk = Chunk {
                hash: crate::hash::sha256_bytes(&entry.data),
                offset: entry.offset,
                length,
                data: entry.data,
            };
            visit(&chunk)?;
            processed = processed
                .checked_add(u64::from(length))
                .context("streamed artifact size overflow")?;
        }

        Ok(processed)
    }

    /// Chunk a file
    ///
    /// This aggregate API retains all chunk bytes in the result. Use
    /// [`Self::visit_reader_chunks`] when the caller can consume chunks
    /// incrementally and needs bounded memory.
    pub fn chunk_file(&self, path: &Path) -> Result<ChunkedFile> {
        let mut file =
            File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;

        let metadata = file.metadata()?;
        let size = metadata.len();

        let mut data = Vec::with_capacity(size as usize);
        file.read_to_end(&mut data)?;

        // Calculate file hash
        let file_hash: [u8; 32] = crate::hash::sha256_bytes(&data);

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
        crate::filesystem::object_path(&self.root, &hex)
            .expect("hex-encoded chunk hash should always produce a valid CAS path")
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

        // Write to a unique temp file then rename (atomic).
        // Using a unique temp name avoids TOCTOU races when multiple
        // processes store the same chunk concurrently.
        let parent = path.parent().unwrap_or(Path::new("."));
        let mut temp_file = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("Failed to create temp file in {}", parent.display()))?;
        temp_file.write_all(&chunk.data)?;
        temp_file.as_file().sync_all()?;

        match temp_file.persist(&path) {
            Ok(_) => Ok(true), // Newly stored
            Err(e) => {
                // If the destination already exists (another process stored it
                // concurrently), treat as successful deduplication rather than
                // a hard error -- the chunk content is identical by hash.
                if e.error.kind() == std::io::ErrorKind::AlreadyExists || path.exists() {
                    Ok(false) // Already exists (concurrent write)
                } else {
                    Err(anyhow::anyhow!(
                        "Failed to persist chunk to {}: {}",
                        path.display(),
                        e.error
                    ))
                }
            }
        }
    }

    /// Retrieve a chunk by hash
    pub fn get_chunk(&self, hash: &[u8; 32]) -> Result<Vec<u8>> {
        let path = self.chunk_path(hash);
        let mut file = File::open(&path)
            .with_context(|| format!("Failed to read chunk: {}", path.display()))?;
        let mut data = Vec::with_capacity(MAX_CHUNK_SIZE as usize);
        Read::by_ref(&mut file)
            .take(u64::from(MAX_CHUNK_SIZE) + 1)
            .read_to_end(&mut data)
            .with_context(|| format!("Failed to read chunk: {}", path.display()))?;
        if data.len() > MAX_CHUNK_SIZE as usize {
            bail!(
                "stored chunk exceeds the {} byte maximum: {}",
                MAX_CHUNK_SIZE,
                path.display()
            );
        }
        let expected_hash = hex::encode(hash);
        crate::hash::verify_sha256(&data, &expected_hash).map_err(|error| {
            anyhow::anyhow!("chunk hash mismatch for {}: {}", path.display(), error)
        })?;
        Ok(data)
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
                    let hex = format!("{}{}", prefix.to_string_lossy(), suffix.to_string_lossy());
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

#[cfg(test)]
mod tests;
