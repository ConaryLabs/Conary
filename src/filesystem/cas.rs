// src/filesystem/cas.rs

//! Content-addressable storage (CAS) for files
//!
//! Files are stored by their SHA-256 hash, enabling deduplication
//! and efficient rollback support, similar to git's object storage.

use crate::error::Result;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::debug;

/// Content-addressable storage manager
pub struct CasStore {
    /// Root directory for object storage (e.g., /var/lib/conary/objects)
    objects_dir: PathBuf,
}

impl CasStore {
    /// Create a new CAS store with the given objects directory
    pub fn new<P: AsRef<Path>>(objects_dir: P) -> Result<Self> {
        let objects_dir = objects_dir.as_ref().to_path_buf();

        // Create objects directory if it doesn't exist
        if !objects_dir.exists() {
            fs::create_dir_all(&objects_dir)?;
            debug!("Created CAS objects directory: {:?}", objects_dir);
        }

        Ok(Self { objects_dir })
    }

    /// Store file content in CAS and return its SHA-256 hash
    ///
    /// The content is stored at: objects/{first2}/{rest_of_hash}
    /// If the content already exists (same hash), this is a no-op (deduplication).
    pub fn store(&self, content: &[u8]) -> Result<String> {
        // Compute SHA-256 hash
        let hash = Self::compute_hash(content);

        // Get storage path
        let path = self.hash_to_path(&hash);

        // If already exists, skip (deduplication)
        if path.exists() {
            debug!("Content already in CAS: {}", hash);
            return Ok(hash);
        }

        // Create parent directory
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write content atomically (write to temp, then rename)
        let temp_path = path.with_extension("tmp");
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(content)?;
        file.sync_all()?;

        // Atomic rename
        fs::rename(&temp_path, &path)?;

        debug!("Stored content in CAS: {} ({} bytes)", hash, content.len());
        Ok(hash)
    }

    /// Retrieve file content from CAS by hash
    pub fn retrieve(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.hash_to_path(hash);

        if !path.exists() {
            return Err(crate::Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Content not found in CAS: {}", hash),
            )));
        }

        let mut file = fs::File::open(&path)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;

        // Verify hash
        let computed_hash = Self::compute_hash(&content);
        if computed_hash != hash {
            return Err(crate::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Hash mismatch: expected {}, got {}",
                    hash, computed_hash
                ),
            )));
        }

        debug!("Retrieved content from CAS: {} ({} bytes)", hash, content.len());
        Ok(content)
    }

    /// Check if content with given hash exists in CAS
    pub fn exists(&self, hash: &str) -> bool {
        self.hash_to_path(hash).exists()
    }

    /// Get the filesystem path for a given hash
    ///
    /// Path format: objects/{first2}/{remaining}
    /// Example: abc123... -> objects/ab/c123...
    pub fn hash_to_path(&self, hash: &str) -> PathBuf {
        if hash.len() < 2 {
            return self.objects_dir.join(hash);
        }

        let (prefix, suffix) = hash.split_at(2);
        self.objects_dir.join(prefix).join(suffix)
    }

    /// Compute SHA-256 hash of content
    pub fn compute_hash(content: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content);
        format!("{:x}", hasher.finalize())
    }

    /// Get the objects directory path
    pub fn objects_dir(&self) -> &Path {
        &self.objects_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_compute_hash() {
        let content = b"Hello, World!";
        let hash = CasStore::compute_hash(content);
        assert_eq!(
            hash,
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
        );
    }

    #[test]
    fn test_store_and_retrieve() {
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();

        let content = b"Test content for CAS";
        let hash = cas.store(content).unwrap();

        // Verify stored content
        let retrieved = cas.retrieve(&hash).unwrap();
        assert_eq!(content, retrieved.as_slice());
    }

    #[test]
    fn test_deduplication() {
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();

        let content = b"Duplicate content";
        let hash1 = cas.store(content).unwrap();
        let hash2 = cas.store(content).unwrap();

        // Same content should give same hash
        assert_eq!(hash1, hash2);

        // Should exist in CAS
        assert!(cas.exists(&hash1));
    }

    #[test]
    fn test_hash_to_path() {
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();

        let hash = "abc123def456";
        let path = cas.hash_to_path(hash);

        let expected = temp_dir.path().join("ab").join("c123def456");
        assert_eq!(path, expected);
    }

    #[test]
    fn test_retrieve_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();

        let result = cas.retrieve("nonexistent_hash");
        assert!(result.is_err());
    }
}
