// src/filesystem/cas.rs

//! Content-addressable storage (CAS) for files
//!
//! Files are stored by their content hash, enabling deduplication
//! and efficient rollback support, similar to git's object storage.
//!
//! # Hash Algorithm Selection
//!
//! The CAS supports multiple hash algorithms:
//! - **SHA-256** (default): Cryptographic hash for security-critical use
//! - **XXH128**: Fast non-cryptographic hash for pure deduplication
//!
//! Use `CasStore::with_algorithm()` to select the hash algorithm.

use crate::error::Result;
use crate::hash::{self, HashAlgorithm};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::debug;

/// Content-addressable storage manager
#[derive(Clone)]
pub struct CasStore {
    /// Root directory for object storage (e.g., /var/lib/conary/objects)
    objects_dir: PathBuf,
    /// Hash algorithm to use for content addressing
    algorithm: HashAlgorithm,
}

impl CasStore {
    /// Create a new CAS store with the given objects directory
    ///
    /// Uses SHA-256 by default. Use `with_algorithm()` for other hash algorithms.
    pub fn new<P: AsRef<Path>>(objects_dir: P) -> Result<Self> {
        Self::with_algorithm(objects_dir, HashAlgorithm::Sha256)
    }

    /// Create a new CAS store with a specific hash algorithm
    ///
    /// # Arguments
    ///
    /// * `objects_dir` - Directory to store content-addressed objects
    /// * `algorithm` - Hash algorithm to use (SHA-256 or XXH128)
    ///
    /// # Example
    ///
    /// ```ignore
    /// use conary::filesystem::CasStore;
    /// use conary::hash::HashAlgorithm;
    ///
    /// // Fast CAS for local deduplication
    /// let fast_cas = CasStore::with_algorithm("/var/lib/conary/objects", HashAlgorithm::Xxh128)?;
    ///
    /// // Secure CAS for package verification
    /// let secure_cas = CasStore::with_algorithm("/var/lib/conary/objects", HashAlgorithm::Sha256)?;
    /// ```
    pub fn with_algorithm<P: AsRef<Path>>(objects_dir: P, algorithm: HashAlgorithm) -> Result<Self> {
        let objects_dir = objects_dir.as_ref().to_path_buf();

        // Create objects directory if it doesn't exist
        if !objects_dir.exists() {
            fs::create_dir_all(&objects_dir)?;
            debug!(
                "Created CAS objects directory: {:?} (algorithm: {})",
                objects_dir, algorithm
            );
        }

        Ok(Self {
            objects_dir,
            algorithm,
        })
    }

    /// Get the hash algorithm used by this CAS
    #[inline]
    pub fn algorithm(&self) -> HashAlgorithm {
        self.algorithm
    }

    /// Store file content in CAS and return its hash
    ///
    /// The content is stored at: objects/{first2}/{rest_of_hash}
    /// If the content already exists (same hash), this is a no-op (deduplication).
    pub fn store(&self, content: &[u8]) -> Result<String> {
        // Compute hash using configured algorithm
        let hash = self.compute_hash(content);

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
        let computed_hash = self.compute_hash(&content);
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

    /// Compute hash of content using this store's algorithm
    pub fn compute_hash(&self, content: &[u8]) -> String {
        hash::hash_bytes(self.algorithm, content).value
    }

    /// Compute hash of content using a specific algorithm (static method)
    ///
    /// This is useful when you need to compute a hash without a CasStore instance,
    /// such as when verifying package signatures.
    pub fn compute_hash_with(algorithm: HashAlgorithm, content: &[u8]) -> String {
        hash::hash_bytes(algorithm, content).value
    }

    /// Compute SHA-256 hash (convenience method for backward compatibility)
    pub fn compute_sha256(content: &[u8]) -> String {
        hash::sha256(content)
    }

    /// Get the objects directory path
    pub fn objects_dir(&self) -> &Path {
        &self.objects_dir
    }

    /// Store a symlink target in CAS
    ///
    /// Symlinks are stored as their target path (the content is the target string).
    /// The hash is computed from the target path, prefixed with "symlink:" to
    /// distinguish from regular file content.
    pub fn store_symlink(&self, target: &str) -> Result<String> {
        // Prefix to distinguish symlink content from file content
        let content = format!("symlink:{}", target);
        self.store(content.as_bytes())
    }

    /// Retrieve a symlink target from CAS
    ///
    /// Returns the symlink target path if the hash represents a symlink.
    pub fn retrieve_symlink(&self, hash: &str) -> Result<Option<String>> {
        let content = self.retrieve(hash)?;
        let content_str = String::from_utf8_lossy(&content);

        if let Some(target) = content_str.strip_prefix("symlink:") {
            Ok(Some(target.to_string()))
        } else {
            Ok(None) // Not a symlink
        }
    }

    /// Check if a hash represents a symlink
    pub fn is_symlink_hash(&self, hash: &str) -> bool {
        if let Ok(content) = self.retrieve(hash) {
            let content_str = String::from_utf8_lossy(&content);
            content_str.starts_with("symlink:")
        } else {
            false
        }
    }

    /// Hardlink an existing file into CAS (zero-copy adoption)
    ///
    /// Instead of reading the file and copying it to CAS, this creates a hardlink
    /// from the CAS location to the existing file. Benefits:
    /// - Zero additional disk space (same inode)
    /// - Instant operation (no I/O for content)
    /// - File survives if original is deleted (nlink > 1)
    ///
    /// Falls back to copying if hardlink fails (cross-device, etc).
    ///
    /// Returns the SHA-256 hash of the file content.
    pub fn hardlink_from_existing<P: AsRef<Path>>(&self, existing_path: P) -> Result<String> {
        let existing_path = existing_path.as_ref();

        // Read file to compute hash (we need the hash for the CAS path)
        // Note: We still need to read for hashing, but we avoid the write
        let content = fs::read(existing_path)?;
        let hash = self.compute_hash(&content);

        // Get CAS storage path
        let cas_path = self.hash_to_path(&hash);

        // If already exists in CAS, we're done (deduplication)
        if cas_path.exists() {
            debug!(
                "Content already in CAS (hardlink adoption): {} -> {}",
                existing_path.display(),
                hash
            );
            return Ok(hash);
        }

        // Create parent directory
        if let Some(parent) = cas_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Try hardlink first (existing_path -> cas_path)
        // Note: hardlink order is (original, link) so we link FROM existing TO cas
        match fs::hard_link(existing_path, &cas_path) {
            Ok(()) => {
                debug!(
                    "Hardlinked into CAS: {} -> {} (hash: {})",
                    existing_path.display(),
                    cas_path.display(),
                    hash
                );
                Ok(hash)
            }
            Err(e) => {
                // Hardlink failed (probably cross-device), fall back to copy
                debug!(
                    "Hardlink failed for {}, falling back to copy: {}",
                    existing_path.display(),
                    e
                );
                self.store(&content)
            }
        }
    }

    /// Hardlink an existing file into CAS using a pre-computed hash
    ///
    /// This is more efficient when you already have the hash (e.g., from RPM metadata)
    /// because it can skip reading the file entirely if the hash already exists in CAS.
    ///
    /// If verify_hash is true, reads the file to verify the hash matches.
    pub fn hardlink_from_existing_with_hash<P: AsRef<Path>>(
        &self,
        existing_path: P,
        expected_hash: &str,
        verify_hash: bool,
    ) -> Result<String> {
        let existing_path = existing_path.as_ref();
        let cas_path = self.hash_to_path(expected_hash);

        // If already exists in CAS, we're done
        if cas_path.exists() {
            debug!(
                "Content already in CAS (skipped hardlink): {} (hash: {})",
                existing_path.display(),
                expected_hash
            );
            return Ok(expected_hash.to_string());
        }

        // Optionally verify hash
        if verify_hash {
            let content = fs::read(existing_path)?;
            let actual_hash = self.compute_hash(&content);
            if actual_hash != expected_hash {
                return Err(crate::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Hash mismatch for {}: expected {}, got {}",
                        existing_path.display(),
                        expected_hash,
                        actual_hash
                    ),
                )));
            }
        }

        // Create parent directory
        if let Some(parent) = cas_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Try hardlink
        match fs::hard_link(existing_path, &cas_path) {
            Ok(()) => {
                debug!(
                    "Hardlinked into CAS (with known hash): {} -> {}",
                    existing_path.display(),
                    expected_hash
                );
                Ok(expected_hash.to_string())
            }
            Err(e) => {
                // Fall back to copy
                debug!(
                    "Hardlink failed for {}, falling back to copy: {}",
                    existing_path.display(),
                    e
                );
                let content = fs::read(existing_path)?;
                self.store(&content)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_compute_hash() {
        let content = b"Hello, World!";
        let hash = CasStore::compute_sha256(content);
        assert_eq!(
            hash,
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
        );
    }

    #[test]
    fn test_compute_hash_with_algorithm() {
        let content = b"Hello, World!";

        // SHA-256
        let sha_hash = CasStore::compute_hash_with(HashAlgorithm::Sha256, content);
        assert_eq!(sha_hash.len(), 64);

        // XXH128
        let xxh_hash = CasStore::compute_hash_with(HashAlgorithm::Xxh128, content);
        assert_eq!(xxh_hash.len(), 32);
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
    fn test_store_and_retrieve_xxh128() {
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::with_algorithm(temp_dir.path(), HashAlgorithm::Xxh128).unwrap();

        assert_eq!(cas.algorithm(), HashAlgorithm::Xxh128);

        let content = b"Test content for fast CAS";
        let hash = cas.store(content).unwrap();

        // XXH128 produces 32-char hex (128 bits)
        assert_eq!(hash.len(), 32);

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

    #[test]
    fn test_hardlink_from_existing() {
        let temp_dir = TempDir::new().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let cas = CasStore::new(&cas_dir).unwrap();

        // Create a file to "adopt"
        let existing_file = temp_dir.path().join("existing_file.txt");
        let content = b"Content to be hardlinked into CAS";
        fs::write(&existing_file, content).unwrap();

        // Hardlink into CAS
        let hash = cas.hardlink_from_existing(&existing_file).unwrap();

        // Verify content is in CAS
        assert!(cas.exists(&hash));
        let retrieved = cas.retrieve(&hash).unwrap();
        assert_eq!(content, retrieved.as_slice());
    }

    #[test]
    #[cfg(unix)]
    fn test_hardlink_shares_inode() {
        use std::os::unix::fs::MetadataExt;

        let temp_dir = TempDir::new().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let cas = CasStore::new(&cas_dir).unwrap();

        // Create a file to "adopt"
        let existing_file = temp_dir.path().join("shared_inode.txt");
        let content = b"This file will share an inode with CAS";
        fs::write(&existing_file, content).unwrap();

        // Get original inode
        let original_inode = fs::metadata(&existing_file).unwrap().ino();

        // Hardlink into CAS
        let hash = cas.hardlink_from_existing(&existing_file).unwrap();

        // Get CAS file inode
        let cas_path = cas.hash_to_path(&hash);
        let cas_inode = fs::metadata(&cas_path).unwrap().ino();

        // Should be the same inode (hardlink)
        assert_eq!(
            original_inode, cas_inode,
            "Hardlinked file should share inode with original"
        );

        // nlink should be 2
        let nlink = fs::metadata(&existing_file).unwrap().nlink();
        assert_eq!(nlink, 2, "Hardlinked file should have nlink=2");
    }

    #[test]
    #[cfg(unix)]
    fn test_hardlink_survives_original_deletion() {
        let temp_dir = TempDir::new().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let cas = CasStore::new(&cas_dir).unwrap();

        // Create a file to "adopt"
        let existing_file = temp_dir.path().join("will_be_deleted.txt");
        let content = b"This file will be deleted but CAS keeps it";
        fs::write(&existing_file, content).unwrap();

        // Hardlink into CAS
        let hash = cas.hardlink_from_existing(&existing_file).unwrap();

        // Delete the original file (simulating RPM removal)
        fs::remove_file(&existing_file).unwrap();
        assert!(!existing_file.exists());

        // CAS should still have the content
        assert!(cas.exists(&hash));
        let retrieved = cas.retrieve(&hash).unwrap();
        assert_eq!(content, retrieved.as_slice());
    }

    #[test]
    fn test_hardlink_with_known_hash() {
        let temp_dir = TempDir::new().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let cas = CasStore::new(&cas_dir).unwrap();

        // Create a file
        let existing_file = temp_dir.path().join("known_hash.txt");
        let content = b"Content with pre-computed hash";
        fs::write(&existing_file, content).unwrap();

        // Pre-compute hash
        let expected_hash = CasStore::compute_sha256(content);

        // Hardlink with known hash (no verification)
        let hash = cas
            .hardlink_from_existing_with_hash(&existing_file, &expected_hash, false)
            .unwrap();

        assert_eq!(hash, expected_hash);
        assert!(cas.exists(&hash));
    }

    #[test]
    fn test_hardlink_deduplication() {
        let temp_dir = TempDir::new().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let cas = CasStore::new(&cas_dir).unwrap();

        // Create two files with same content
        let file1 = temp_dir.path().join("file1.txt");
        let file2 = temp_dir.path().join("file2.txt");
        let content = b"Identical content in two files";
        fs::write(&file1, content).unwrap();
        fs::write(&file2, content).unwrap();

        // Hardlink first file
        let hash1 = cas.hardlink_from_existing(&file1).unwrap();

        // Hardlink second file - should detect duplicate
        let hash2 = cas.hardlink_from_existing(&file2).unwrap();

        // Same hash
        assert_eq!(hash1, hash2);

        // Content retrievable
        let retrieved = cas.retrieve(&hash1).unwrap();
        assert_eq!(content, retrieved.as_slice());
    }
}
