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
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use tracing::{debug, warn};

mod ephemeral;
mod liveness;
mod stream;
mod verified_batch;
mod write_batch;

pub(crate) use ephemeral::EphemeralObjectStore;
pub use liveness::CasObjectCollectionSession;
pub(crate) use liveness::CasObjectLivenessLease;
pub use verified_batch::{
    VerifiedObjectBatch, VerifiedObjectBatchMetrics, VerifiedObjectDisposition, VerifiedObjectSet,
};
pub use write_batch::{PrivateCasWriter, PrivateCopyBatch};

/// Return whether a filename belongs to the CAS private staging namespace.
///
/// Atomic writers use `{hash}.tmp.{pid}.{counter}`, while streaming and batch
/// writers may use leading-dot private names. A plain `.tmp` suffix is retained
/// for the older single-writer path. Scanners must never interpret any of these
/// private names as content identities.
pub fn is_temporary_object_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with('.') || name.contains(".tmp.") || name.ends_with(".tmp")
}

/// Compute the CAS object path for a hex hash under a root directory.
///
/// Uses a two-level layout: `root/<hash[..2]>/<hash[2..]>`.
/// This is the canonical path construction shared by CAS storage,
/// CCS builder, chunking, archive reader, and derivation install.
///
/// # Security
///
/// Validates that `hash` contains only ASCII hex digits to prevent path
/// traversal via crafted hash strings containing `/`, `..`, or null bytes.
///
/// # Panics
///
/// Does not panic. Returns an error for invalid hashes instead of constructing
/// a fallback path.
pub fn object_path(root: &Path, hash: &str) -> Result<PathBuf> {
    validate_hash_path(hash)?;
    let (prefix, suffix) = hash.split_at(2);
    Ok(root.join(prefix).join(suffix))
}

fn validate_hash_path(hash: &str) -> Result<()> {
    if hash.len() < 4 {
        return Err(crate::Error::InvalidPath(format!(
            "hash too short for CAS path (need >= 4 hex chars, got {}): '{}'",
            hash.len(),
            hash
        )));
    }

    if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(crate::Error::InvalidPath(format!(
            "hash contains non-hex characters: '{}'",
            &hash[..hash.len().min(20)]
        )));
    }

    Ok(())
}

#[cfg(unix)]
fn cas_object_appears_shared(path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    Ok(fs::metadata(path)?.nlink() > 1)
}

#[cfg(not(unix))]
fn cas_object_appears_shared(_path: &Path) -> Result<bool> {
    Ok(false)
}

fn sync_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        let dir = fs::File::open(parent)?;
        dir.sync_all()?;
    }
    Ok(())
}

/// Content-addressable storage manager
#[derive(Clone, Debug)]
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
        sync_parent_dir(&path)?;

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
            } else if file_type.is_file()
                && is_temporary_object_name(&entry.file_name())
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
        object_path(&self.objects_dir, hash)
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

    /// Compute a SHA-256 hash as unprefixed hexadecimal bytes.
    pub fn compute_sha256(content: &[u8]) -> String {
        hash::sha256(content)
    }

    /// Compute the hash for a symlink target (static method)
    ///
    /// This provides a single source of truth for symlink hashing used by:
    /// - `store_symlink()` for storing symlinks in CAS
    /// - `CcsPackage` parser for matching symlink hashes
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

    /// Store an existing mutable file into CAS by copying bytes into a private inode.
    ///
    /// Use this for live adoption and any path whose source can be modified
    /// outside Conary after capture.
    pub fn store_file_copy_from_existing<P: AsRef<Path>>(
        &self,
        existing_path: P,
    ) -> Result<String> {
        let content = fs::read(existing_path)?;
        self.store_private_copy(&content)
    }

    /// Store caller-captured bytes in a private CAS inode.
    ///
    /// This is the byte-oriented counterpart to
    /// [`Self::store_file_copy_from_existing`]. Callers that must hold an open
    /// source descriptor while validating its node type can read from that
    /// descriptor and then use this method without reopening the mutable path.
    pub fn store_private_copy(&self, content: &[u8]) -> Result<String> {
        let hash = self.compute_hash(content);
        if self.atomic_store_private_copy(&hash, content)? {
            debug!("Stored private file copy in CAS: {}", hash);
        } else {
            debug!("Private CAS object already exists: {}", hash);
        }
        Ok(hash)
    }

    /// Begin a transaction-scoped batch of private live-file captures.
    ///
    /// New objects remain under ignored temporary names until
    /// [`PrivateCopyBatch::commit`] makes the complete batch durable and
    /// publishes it. Callers must commit before persisting references.
    pub fn private_copy_batch(&self) -> PrivateCopyBatch<'_> {
        PrivateCopyBatch::new(self)
    }

    /// Begin a transaction-scoped batch backed by signed SHA-256 object authority.
    ///
    /// Existing canonical objects with the exact signed size are trusted local
    /// hits. Missing objects remain under ignored temporary names until the
    /// complete batch has been verified, made durable, and committed.
    pub fn verified_object_batch<I, S>(&self, expected: I) -> Result<VerifiedObjectBatch<'_>>
    where
        I: IntoIterator<Item = (S, u64)>,
        S: Into<String>,
    {
        VerifiedObjectBatch::new(self, expected)
    }

    /// Atomically store content into a private CAS inode.
    ///
    /// Unlike `store`, this helper also repairs a touched shared-hardlink
    /// object. If the hash already exists and appears private, it is left alone
    /// for deduplication. If it is shared on Unix, the CAS directory entry is
    /// replaced with a fresh inode containing the same content.
    fn atomic_store_private_copy(&self, hash: &str, content: &[u8]) -> Result<bool> {
        let path = self.hash_to_path(hash)?;
        if path.exists() && !cas_object_appears_shared(&path)? {
            return Ok(false);
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let temp_ext = format!(
            "tmp.{}.{}.private",
            std::process::id(),
            Self::next_temp_id()
        );
        let temp_path = path.with_extension(temp_ext);
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&temp_path, &path)?;
        sync_parent_dir(&path)?;
        Ok(true)
    }

    /// Hardlink an existing file into CAS when the caller proves the source root is sealed.
    ///
    /// Do not use this for live native package-manager files. A hardlink shares
    /// the inode with the source and therefore shares future in-place mutations.
    pub fn hardlink_from_immutable_root<P: AsRef<Path>>(&self, existing_path: P) -> Result<String> {
        self.hardlink_from_existing_inner(existing_path)
    }

    fn hardlink_from_existing_inner<P: AsRef<Path>>(&self, existing_path: P) -> Result<String> {
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

    /// Hardlink an existing file into CAS using a pre-computed hash when the
    /// caller proves the source root is sealed.
    ///
    /// This is more efficient when you already have the hash (e.g., from RPM metadata)
    /// because it can skip reading the file entirely if the hash already exists in CAS.
    ///
    /// If verify_hash is true, reads the file to verify the hash matches.
    pub fn hardlink_from_immutable_root_with_hash<P: AsRef<Path>>(
        &self,
        existing_path: P,
        expected_hash: &str,
        verify_hash: bool,
    ) -> Result<String> {
        self.hardlink_from_existing_with_hash_inner(existing_path, expected_hash, verify_hash)
    }

    fn hardlink_from_existing_with_hash_inner<P: AsRef<Path>>(
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

                    if is_temporary_object_name(&name) {
                        continue;
                    }

                    let name_str = name.to_string_lossy();
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
mod tests;
