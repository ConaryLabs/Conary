// src/provenance/content.rs

//! Content layer provenance - what's in the package

use super::CanonicalBytes;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Content layer provenance information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContentProvenance {
    /// Merkle root hash of all file content hashes
    #[serde(default)]
    pub merkle_root: Option<String>,

    /// Per-component content hashes
    #[serde(default)]
    pub component_hashes: BTreeMap<String, ComponentHash>,

    /// CDC chunk manifest (for delta updates)
    #[serde(default)]
    pub chunk_manifest: Vec<ChunkInfo>,

    /// Total uncompressed size in bytes
    #[serde(default)]
    pub total_size: u64,

    /// Total number of files
    #[serde(default)]
    pub file_count: u64,
}

impl ContentProvenance {
    /// Create new content provenance with merkle root
    pub fn new(merkle_root: &str) -> Self {
        Self {
            merkle_root: Some(merkle_root.to_string()),
            ..Default::default()
        }
    }

    /// Add a component hash
    pub fn add_component(&mut self, name: &str, hash: ComponentHash) {
        self.component_hashes.insert(name.to_string(), hash);
    }

    /// Add a chunk to the manifest
    pub fn add_chunk(&mut self, chunk: ChunkInfo) {
        self.total_size += chunk.size;
        self.chunk_manifest.push(chunk);
    }

    /// Get the merkle root or compute a placeholder
    pub fn root_hash(&self) -> &str {
        self.merkle_root.as_deref().unwrap_or("unknown")
    }

    /// Check if content matches expected hash
    pub fn verify(&self, expected_merkle: &str) -> bool {
        self.merkle_root.as_deref() == Some(expected_merkle)
    }
}

impl CanonicalBytes for ContentProvenance {
    fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        if let Some(ref root) = self.merkle_root {
            bytes.extend_from_slice(b"merkle:");
            bytes.extend_from_slice(root.as_bytes());
            bytes.push(0);
        }

        // BTreeMap is already sorted by key
        for (name, hash) in &self.component_hashes {
            bytes.extend_from_slice(b"component:");
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(b':');
            bytes.extend_from_slice(hash.hash.as_bytes());
            bytes.push(0);
        }

        // Chunks are ordered in the manifest
        for chunk in &self.chunk_manifest {
            bytes.extend_from_slice(b"chunk:");
            bytes.extend_from_slice(chunk.hash.as_bytes());
            bytes.push(0);
        }

        bytes
    }
}

/// Hash information for a component
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHash {
    /// Content hash of the component
    pub hash: String,

    /// Size of the component in bytes
    pub size: u64,

    /// Number of files in the component
    pub file_count: u64,
}

impl ComponentHash {
    /// Create a new component hash
    pub fn new(hash: &str, size: u64, file_count: u64) -> Self {
        Self {
            hash: hash.to_string(),
            size,
            file_count,
        }
    }
}

/// Information about a CDC chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkInfo {
    /// Content hash of the chunk
    pub hash: String,

    /// Size of the chunk in bytes
    pub size: u64,

    /// Offset in the logical stream (for reconstruction)
    #[serde(default)]
    pub offset: u64,
}

impl ChunkInfo {
    /// Create a new chunk info
    pub fn new(hash: &str, size: u64, offset: u64) -> Self {
        Self {
            hash: hash.to_string(),
            size,
            offset,
        }
    }
}

/// Builder for constructing content provenance during package creation
pub struct ContentProvenanceBuilder {
    files: Vec<FileHash>,
    chunks: Vec<ChunkInfo>,
    components: BTreeMap<String, Vec<FileHash>>,
}

impl ContentProvenanceBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            chunks: Vec::new(),
            components: BTreeMap::new(),
        }
    }

    /// Add a file with its hash
    pub fn add_file(&mut self, path: &str, hash: &str, size: u64, component: &str) {
        let file_hash = FileHash {
            path: path.to_string(),
            hash: hash.to_string(),
            size,
        };

        self.files.push(file_hash.clone());
        self.components
            .entry(component.to_string())
            .or_default()
            .push(file_hash);
    }

    /// Add a chunk
    pub fn add_chunk(&mut self, hash: &str, size: u64, offset: u64) {
        self.chunks.push(ChunkInfo::new(hash, size, offset));
    }

    /// Build the content provenance
    pub fn build(self) -> ContentProvenance {
        use sha2::{Sha256, Digest};

        // Compute merkle root from sorted file hashes
        let mut hasher = Sha256::new();
        let mut sorted_files = self.files.clone();
        sorted_files.sort_by(|a, b| a.path.cmp(&b.path));

        for file in &sorted_files {
            hasher.update(file.path.as_bytes());
            hasher.update(file.hash.as_bytes());
        }

        let merkle_root = format!("sha256:{}", hex::encode(hasher.finalize()));

        // Compute component hashes
        let mut component_hashes = BTreeMap::new();
        for (name, files) in &self.components {
            let mut comp_hasher = Sha256::new();
            let mut size = 0u64;

            for file in files {
                comp_hasher.update(file.hash.as_bytes());
                size += file.size;
            }

            component_hashes.insert(
                name.clone(),
                ComponentHash::new(
                    &format!("sha256:{}", hex::encode(comp_hasher.finalize())),
                    size,
                    files.len() as u64,
                ),
            );
        }

        let total_size: u64 = self.files.iter().map(|f| f.size).sum();
        let file_count = self.files.len() as u64;

        ContentProvenance {
            merkle_root: Some(merkle_root),
            component_hashes,
            chunk_manifest: self.chunks,
            total_size,
            file_count,
        }
    }
}

impl Default for ContentProvenanceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Internal struct for tracking file hashes
#[derive(Debug, Clone)]
struct FileHash {
    path: String,
    hash: String,
    size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_provenance() {
        let mut content = ContentProvenance::new("sha256:merkleroot");
        content.add_component("runtime", ComponentHash::new("sha256:runtime", 1000, 5));
        content.add_component("lib", ComponentHash::new("sha256:lib", 5000, 10));

        assert_eq!(content.component_hashes.len(), 2);
        assert!(content.verify("sha256:merkleroot"));
    }

    #[test]
    fn test_builder() {
        let mut builder = ContentProvenanceBuilder::new();

        builder.add_file("/usr/bin/app", "sha256:abc", 1000, "runtime");
        builder.add_file("/usr/lib/libapp.so", "sha256:def", 5000, "lib");
        builder.add_chunk("sha256:chunk1", 2000, 0);
        builder.add_chunk("sha256:chunk2", 4000, 2000);

        let content = builder.build();

        assert!(content.merkle_root.is_some());
        assert_eq!(content.component_hashes.len(), 2);
        assert_eq!(content.chunk_manifest.len(), 2);
        assert_eq!(content.file_count, 2);
        assert_eq!(content.total_size, 6000);
    }

    #[test]
    fn test_canonical_bytes_deterministic() {
        let content1 = ContentProvenance::new("sha256:merkle");
        let content2 = ContentProvenance::new("sha256:merkle");

        assert_eq!(content1.canonical_bytes(), content2.canonical_bytes());
    }
}
