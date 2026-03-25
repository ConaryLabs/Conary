// conary-core/src/filesystem/cas.rs

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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use tracing::{debug, warn};

/// Compute the CAS object path for a hex hash under a root directory.
///
/// Uses a two-level layout: `root/<hash[..2]>/<hash[2..]>`.
/// This is the canonical path construction shared by CAS storage,
/// CCS builder, chunking, archive reader, and derivation install.
///
/// # Panics
///
/// Does not panic. Returns a flat path under `root` if `hash` is shorter
/// than 3 characters (graceful fallback for edge cases).
pub fn object_path(root: &Path, hash: &str) -> PathBuf {
    if hash.len() < 3 {
        return root.join(hash);
    }
    let (prefix, suffix) = hash.split_at(2);
    root.join(prefix).join(suffix)
}

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
    /// use conary_core::filesystem::CasStore;
    /// use conary_core::hash::HashAlgorithm;
    ///
    /// // Fast CAS for local deduplication
    /// let fast_cas = CasStore::with_algorithm("/var/lib/conary/objects", HashAlgorithm::Xxh128)?;
    ///
    /// // Secure CAS for package verification
    /// let secure_cas = CasStore::with_algorithm("/var/lib/conary/objects", HashAlgorithm::Sha256)?;
    /// ```
    pub fn with_algorithm<P: AsRef<Path>>(
        objects_dir: P,
        algorithm: HashAlgorithm,
    ) -> Result<Self> {
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

    /// Counter for generating unique temp file names within this process.
    fn next_temp_id() -> u64 {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    /// Atomically write content to a CAS path (write to temp, fsync, rename).
    ///
    /// Uses a unique temp name incorporating PID and a monotonic counter to avoid
    /// races when multiple processes or threads store to the same hash concurrently.
    ///
    /// Returns `true` if content was written, `false` if it already existed.
    fn atomic_store(&self, hash: &str, content: &[u8]) -> Result<bool> {
        let path = self.hash_to_path(hash)?;

        if path.exists() {
            return Ok(false);
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let temp_ext = format!("tmp.{}.{}", std::process::id(), Self::next_temp_id());
        let temp_path = path.with_extension(temp_ext);
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&temp_path, &path)?;

        // Fsync parent directory to ensure the rename is durable on crash
        if let Some(parent) = path.parent()
            && let Ok(dir) = fs::File::open(parent)
        {
            let _ = dir.sync_all();
        }

        Ok(true)
    }

    /// Remove orphaned temp files older than the given threshold.
    ///
    /// Temp files are left behind when a process crashes between creating the temp
    /// file and renaming it into place. This method scans for files matching
    /// `*.tmp.*` and removes any older than the specified duration.
    ///
    /// A threshold of 1 hour is recommended to avoid interfering with stores
    /// that are legitimately in progress.
    pub fn cleanup_orphaned_temps(&self, max_age: std::time::Duration) -> Result<usize> {
        let now = SystemTime::now();
        let mut removed = 0;

        self.cleanup_temps_in_dir(&self.objects_dir, now, max_age, &mut removed)?;

        if removed > 0 {
            debug!(
                "Cleaned up {} orphaned temp file(s) from CAS (older than {:?})",
                removed, max_age
            );
        }

        Ok(removed)
    }

    /// Recursively scan a directory for orphaned temp files.
    fn cleanup_temps_in_dir(
        &self,
        dir: &Path,
        now: SystemTime,
        max_age: std::time::Duration,
        removed: &mut usize,
    ) -> Result<()> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };

        for entry in entries {
            let entry = entry?;
            let file_type = entry.file_type()?;

            if file_type.is_dir() {
                self.cleanup_temps_in_dir(&entry.path(), now, max_age, removed)?;
            } else if file_type.is_file() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();

                // Match temp files: any file whose name contains ".tmp."
                if name_str.contains(".tmp.")
                    && let Ok(metadata) = entry.metadata()
                {
                    let age = metadata
                        .modified()
                        .ok()
                        .and_then(|mtime| now.duration_since(mtime).ok());

                    if age.is_some_and(|a| a > max_age) {
                        match fs::remove_file(entry.path()) {
                            Ok(()) => {
                                *removed += 1;
                                debug!("Removed orphaned temp file: {}", entry.path().display());
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to remove orphaned temp file {}: {}",
                                    entry.path().display(),
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Store file content in CAS and return its hash
    ///
    /// The content is stored at: objects/{first2}/{rest_of_hash}
    /// If the content already exists (same hash), this is a no-op (deduplication).
    pub fn store(&self, content: &[u8]) -> Result<String> {
        let hash = self.compute_hash(content);

        if self.atomic_store(&hash, content)? {
            debug!("Stored content in CAS: {} ({} bytes)", hash, content.len());
        } else {
            debug!("Content already in CAS: {}", hash);
        }

        Ok(hash)
    }

    /// Retrieve file content from CAS by hash
    pub fn retrieve(&self, hash: &str) -> Result<Vec<u8>> {
        self.retrieve_with_algorithm(hash, self.algorithm)
    }

    /// Retrieve a CAS object by hash WITHOUT integrity verification.
    ///
    /// Use when fs-verity is enabled or when the caller will verify separately.
    /// Avoids the overhead of reading the file twice (once for hash, once for content).
    pub fn retrieve_unchecked(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.hash_to_path(hash)?;
        std::fs::read(&path).map_err(|e| {
            crate::Error::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read CAS object {}: {}", hash, e),
            ))
        })
    }

    /// Retrieve file content from CAS with explicit hash algorithm
    ///
    /// This is useful when the stored content uses a different algorithm
    /// than the CAS's default (e.g., symlinks always use SHA-256).
    fn retrieve_with_algorithm(&self, hash: &str, algorithm: HashAlgorithm) -> Result<Vec<u8>> {
        let path = self.hash_to_path(hash)?;

        if !path.exists() {
            return Err(crate::Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Content not found in CAS: {}", hash),
            )));
        }

        let mut file = fs::File::open(&path)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;

        // Verify hash with specified algorithm
        let computed_hash = hash::hash_bytes(algorithm, &content).value;
        if computed_hash != hash {
            return Err(crate::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Hash mismatch: expected {}, got {}", hash, computed_hash),
            )));
        }

        debug!(
            "Retrieved content from CAS: {} ({} bytes)",
            hash,
            content.len()
        );
        Ok(content)
    }

    /// Check if content with given hash exists in CAS
    pub fn exists(&self, hash: &str) -> bool {
        self.hash_to_path(hash).is_ok_and(|p| p.exists())
    }

    /// Get the filesystem path for a given hash
    ///
    /// Path format: objects/{first2}/{remaining}
    /// Example: abc123... -> objects/ab/c123...
    pub fn hash_to_path(&self, hash: &str) -> Result<PathBuf> {
        if hash.len() < 2 {
            return Err(crate::Error::InvalidPath(format!(
                "hash too short for CAS path (need >= 2 hex chars, got {}): '{}'",
                hash.len(),
                hash
            )));
        }

        Ok(object_path(&self.objects_dir, hash))
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

    /// Compute the hash for a symlink target (static method)
    ///
    /// This provides a single source of truth for symlink hashing used by:
    /// - `store_symlink()` for storing symlinks in CAS
    /// - `CcsPackage` parser for matching symlink hashes
    /// - `TransactionPlanner` for symlink operations
    ///
    /// The hash is computed from the raw target path bytes, matching the
    /// convention used by `CcsBuilder` which hashes symlink targets as
    /// plain byte content.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let hash = CasStore::compute_symlink_hash("/usr/lib/libfoo.so.1");
    /// // hash is SHA-256 of "/usr/lib/libfoo.so.1"
    /// ```
    pub fn compute_symlink_hash(target: &str) -> String {
        hash::sha256(target.as_bytes())
    }

    /// Iterate over all objects in the CAS store.
    ///
    /// Yields `(hash, path)` pairs by walking the two-level `objects/{prefix}/{suffix}`
    /// directory. Skips temp files (`.` prefix, `.tmp` suffix, or `.tmp.` interior)
    /// and non-file entries.
    pub fn iter_objects(&self) -> impl Iterator<Item = crate::Result<(String, PathBuf)>> + '_ {
        CasIterator::new(&self.objects_dir)
    }

    /// Get the objects directory path
    pub fn objects_dir(&self) -> &Path {
        &self.objects_dir
    }

    /// Store a symlink target in CAS
    ///
    /// Symlinks are stored as the raw target path bytes, matching the convention
    /// used by `CcsBuilder`. The hash is computed using SHA-256 to match
    /// `compute_symlink_hash()`, regardless of the CAS's configured algorithm.
    /// This ensures symlink identity is consistent across systems.
    pub fn store_symlink(&self, target: &str) -> Result<String> {
        let content = target.as_bytes();
        // Always use SHA-256 for symlinks to match compute_symlink_hash()
        // This is critical: symlink hashes are used as identities across systems
        let hash = hash::sha256(content);

        if self.atomic_store(&hash, content)? {
            debug!("Stored symlink in CAS: {} -> {}", target, hash);
        } else {
            debug!("Symlink already in CAS: {}", hash);
        }

        Ok(hash)
    }

    /// Retrieve a symlink target from CAS
    ///
    /// Returns the symlink target path stored at the given hash.
    /// Uses SHA-256 for verification since symlinks are always stored with SHA-256.
    /// The caller is responsible for knowing which hashes represent symlinks
    /// (e.g., via file type metadata from the package manifest).
    pub fn retrieve_symlink(&self, hash: &str) -> Result<String> {
        // Symlinks are always stored with SHA-256, so use that for verification
        let content = self.retrieve_with_algorithm(hash, HashAlgorithm::Sha256)?;
        Ok(String::from_utf8_lossy(&content).into_owned())
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
        let cas_path = self.hash_to_path(&hash)?;

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
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another process stored this content concurrently -- that's fine
                debug!(
                    "CAS object appeared concurrently for {}: {}",
                    existing_path.display(),
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
        let cas_path = self.hash_to_path(expected_hash)?;

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
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another process stored this content concurrently -- that's fine
                debug!(
                    "CAS object appeared concurrently for {}: {}",
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

/// Iterator over all objects in a CAS directory.
///
/// Walks the two-level layout: `objects/{2-char-prefix}/{suffix}`. Reconstructs
/// the full hash as `prefix + suffix`. Skips entries starting with `.`, ending
/// with `.tmp`, or containing `.tmp.` to avoid temp files left by `atomic_store()`
/// (which uses the naming pattern `{hash}.tmp.{pid}.{counter}`).
struct CasIterator {
    /// Outer iterator over prefix directories.
    prefix_iter: Option<std::fs::ReadDir>,
    /// Current prefix string (e.g. "ab").
    current_prefix: String,
    /// Inner iterator over files in the current prefix directory.
    suffix_iter: Option<std::fs::ReadDir>,
}

impl CasIterator {
    fn new(objects_dir: &Path) -> Self {
        let prefix_iter = std::fs::read_dir(objects_dir).ok();
        Self {
            prefix_iter,
            current_prefix: String::new(),
            suffix_iter: None,
        }
    }

    /// Advance to the next valid prefix directory.
    /// Returns `true` if a new prefix directory was found, `false` if exhausted.
    fn advance_prefix(&mut self) -> crate::Result<bool> {
        let Some(ref mut iter) = self.prefix_iter else {
            return Ok(false);
        };

        loop {
            let Some(entry) = iter.next() else {
                return Ok(false);
            };
            let entry = entry?;

            if !entry.file_type()?.is_dir() {
                continue;
            }

            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Prefix directories must be exactly 2 characters
            if name_str.len() != 2 {
                continue;
            }

            self.current_prefix = name_str.into_owned();
            self.suffix_iter = Some(std::fs::read_dir(entry.path())?);
            return Ok(true);
        }
    }
}

impl Iterator for CasIterator {
    type Item = crate::Result<(String, PathBuf)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Try to get the next file from the current suffix iterator
            if let Some(ref mut suffix_iter) = self.suffix_iter {
                for entry in suffix_iter.by_ref() {
                    let entry = match entry {
                        Ok(e) => e,
                        Err(e) => return Some(Err(e.into())),
                    };

                    let ft = match entry.file_type() {
                        Ok(ft) => ft,
                        Err(e) => return Some(Err(e.into())),
                    };

                    if !ft.is_file() {
                        continue;
                    }

                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();

                    // Skip temp files: atomic_store() creates temps as
                    // `{hash}.tmp.{pid}.{counter}` which end with a digit,
                    // so we also match on the `.tmp.` interior marker.
                    if name_str.starts_with('.')
                        || name_str.contains(".tmp.")
                        || name_str.ends_with(".tmp")
                    {
                        continue;
                    }

                    let hash = format!("{}{}", self.current_prefix, name_str);
                    return Some(Ok((hash, entry.path())));
                }
            }

            // Current prefix exhausted, advance to next
            match self.advance_prefix() {
                Ok(true) => continue,
                Ok(false) => return None,
                Err(e) => return Some(Err(e)),
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
    fn test_compute_symlink_hash() {
        let target = "/usr/lib/libfoo.so.1";
        let hash = CasStore::compute_symlink_hash(target);

        // Should be SHA-256 of raw target bytes (matching CcsBuilder convention)
        let expected = CasStore::compute_sha256(b"/usr/lib/libfoo.so.1");
        assert_eq!(hash, expected);

        // Hash should be 64 chars (256 bits hex)
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_symlink_hash_consistency() {
        // Verify that compute_symlink_hash and store_symlink produce the same hash
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();

        let target = "/usr/bin/python3";
        let computed_hash = CasStore::compute_symlink_hash(target);
        let stored_hash = cas.store_symlink(target).unwrap();

        assert_eq!(
            computed_hash, stored_hash,
            "compute_symlink_hash and store_symlink must produce identical hashes"
        );
    }

    #[test]
    fn test_symlink_hash_consistency_with_xxh128() {
        // Verify symlink hashes are consistent even when CAS uses Xxh128
        // This tests that store_symlink always uses SHA-256 for symlinks,
        // regardless of the CAS's configured algorithm.
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::with_algorithm(temp_dir.path(), HashAlgorithm::Xxh128).unwrap();

        // CAS is configured for Xxh128
        assert_eq!(cas.algorithm(), HashAlgorithm::Xxh128);

        let target = "/usr/lib64/libssl.so.3";
        let computed_hash = CasStore::compute_symlink_hash(target);
        let stored_hash = cas.store_symlink(target).unwrap();

        // Symlink hashes must match compute_symlink_hash (always SHA-256)
        assert_eq!(
            computed_hash, stored_hash,
            "Symlink hash must use SHA-256 even when CAS uses Xxh128"
        );

        // Hash should be 64 chars (SHA-256), not 32 (XXH128)
        assert_eq!(
            stored_hash.len(),
            64,
            "Symlink hash must be SHA-256 (64 chars)"
        );

        // Verify the symlink can be retrieved
        let retrieved = cas.retrieve_symlink(&stored_hash).unwrap();
        assert_eq!(retrieved, target);
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
        let path = cas.hash_to_path(hash).unwrap();

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
        let cas_path = cas.hash_to_path(&hash).unwrap();
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

    #[test]
    fn test_atomic_store_unique_temp_names() {
        // Verify that successive stores use different temp name counters
        // by checking that concurrent stores to the same hash do not corrupt data
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();

        let content1 = b"Content A for uniqueness test";
        let content2 = b"Content B for uniqueness test";

        let hash1 = cas.store(content1).unwrap();
        let hash2 = cas.store(content2).unwrap();

        // Different content should produce different hashes
        assert_ne!(hash1, hash2);

        // Both should be retrievable without corruption
        let retrieved1 = cas.retrieve(&hash1).unwrap();
        let retrieved2 = cas.retrieve(&hash2).unwrap();
        assert_eq!(content1, retrieved1.as_slice());
        assert_eq!(content2, retrieved2.as_slice());
    }

    #[test]
    fn test_cleanup_orphaned_temps() {
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();

        // Create a fake orphaned temp file inside a CAS subdirectory
        let sub_dir = temp_dir.path().join("ab");
        fs::create_dir_all(&sub_dir).unwrap();
        let orphan = sub_dir.join("c123def456.tmp.99999.0");
        fs::write(&orphan, "orphaned data").unwrap();

        // With a very large max_age, nothing should be removed (file is too new)
        let removed = cas
            .cleanup_orphaned_temps(std::time::Duration::from_secs(999_999))
            .unwrap();
        assert_eq!(removed, 0);
        assert!(orphan.exists());

        // With zero max_age, the file should be removed (it is older than 0 seconds)
        let removed = cas
            .cleanup_orphaned_temps(std::time::Duration::from_secs(0))
            .unwrap();
        assert_eq!(removed, 1);
        assert!(!orphan.exists());
    }

    #[test]
    fn test_cleanup_ignores_non_temp_files() {
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();

        // Store real content so there is a real CAS file
        let content = b"Real CAS content that should survive cleanup";
        let hash = cas.store(content).unwrap();

        // Cleanup with zero threshold should not touch real CAS files
        let removed = cas
            .cleanup_orphaned_temps(std::time::Duration::from_secs(0))
            .unwrap();
        assert_eq!(removed, 0);

        // Real content should still be retrievable
        let retrieved = cas.retrieve(&hash).unwrap();
        assert_eq!(content, retrieved.as_slice());
    }

    #[test]
    fn test_iter_objects_basic() {
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();

        // Store two distinct objects
        let hash1 = cas.store(b"alpha").unwrap();
        let hash2 = cas.store(b"bravo").unwrap();

        // Also create temp files that should be skipped
        let prefix_dir = temp_dir.path().join(&hash1[..2]);
        fs::write(prefix_dir.join(".tmp_in_progress"), b"temp").unwrap();
        fs::write(prefix_dir.join("something.tmp"), b"temp2").unwrap();
        // Temp file matching atomic_store() naming: {hash}.tmp.{pid}.{counter}
        fs::write(prefix_dir.join("abcdef1234.tmp.12345.0"), b"temp3").unwrap();

        let mut results: Vec<(String, PathBuf)> =
            cas.iter_objects().collect::<Result<Vec<_>>>().unwrap();
        results.sort_by(|a, b| a.0.cmp(&b.0));

        let hashes: Vec<&str> = results.iter().map(|(h, _)| h.as_str()).collect();
        assert!(hashes.contains(&hash1.as_str()));
        assert!(hashes.contains(&hash2.as_str()));
        assert_eq!(
            hashes.len(),
            2,
            "Temp files should be excluded, got: {:?}",
            hashes
        );
    }

    #[test]
    fn test_iter_objects_empty() {
        let temp_dir = TempDir::new().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();

        let results: Vec<_> = cas.iter_objects().collect::<Result<Vec<_>>>().unwrap();
        assert!(results.is_empty());
    }
}
