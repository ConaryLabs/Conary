// src/filesystem/mod.rs

//! Filesystem operations for Conary
//!
//! This module provides content-addressable storage (CAS) for files,
//! similar to git's object storage. Files are stored by their SHA-256
//! hash, enabling deduplication and efficient rollback support.

use crate::error::Result;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

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
    fn hash_to_path(&self, hash: &str) -> PathBuf {
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

/// File deployment manager
pub struct FileDeployer {
    /// CAS store for file contents
    cas: CasStore,
    /// Install root directory (e.g., "/" or "/tmp/conary-root")
    install_root: PathBuf,
}

impl FileDeployer {
    /// Create a new file deployer
    pub fn new<P: AsRef<Path>>(objects_dir: P, install_root: P) -> Result<Self> {
        let cas = CasStore::new(objects_dir)?;
        let install_root = install_root.as_ref().to_path_buf();

        // Create install root if it doesn't exist
        if !install_root.exists() {
            fs::create_dir_all(&install_root)?;
            debug!("Created install root: {:?}", install_root);
        }

        Ok(Self { cas, install_root })
    }

    /// Deploy a file from CAS to the filesystem
    ///
    /// - Retrieves content from CAS by hash
    /// - Writes to install_root + path
    /// - Sets permissions (ownership requires root)
    pub fn deploy_file(
        &self,
        path: &str,
        hash: &str,
        permissions: u32,
    ) -> Result<()> {
        // Retrieve content from CAS
        let content = self.cas.retrieve(hash)?;

        // Compute target path
        let target_path = self.install_root.join(path.trim_start_matches('/'));

        // Create parent directories
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write atomically
        let temp_path = target_path.with_extension("conary-tmp");
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(&content)?;
        file.sync_all()?;

        // Set permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(permissions);
            fs::set_permissions(&temp_path, perms)?;
        }

        // Atomic rename
        fs::rename(&temp_path, &target_path)?;

        info!("Deployed file: {} (hash: {}, mode: {:o})", path, hash, permissions);
        Ok(())
    }

    /// Check if a file exists at the target path
    pub fn file_exists(&self, path: &str) -> bool {
        let target_path = self.install_root.join(path.trim_start_matches('/'));
        target_path.exists()
    }

    /// Remove a file from the filesystem
    pub fn remove_file(&self, path: &str) -> Result<()> {
        let target_path = self.install_root.join(path.trim_start_matches('/'));

        if target_path.exists() {
            fs::remove_file(&target_path)?;
            info!("Removed file: {}", path);
        } else {
            debug!("File already removed: {}", path);
        }

        Ok(())
    }

    /// Verify a file's hash matches expected
    pub fn verify_file(&self, path: &str, expected_hash: &str) -> Result<bool> {
        let target_path = self.install_root.join(path.trim_start_matches('/'));

        if !target_path.exists() {
            return Ok(false);
        }

        let mut file = fs::File::open(&target_path)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;

        let actual_hash = CasStore::compute_hash(&content);
        Ok(actual_hash == expected_hash)
    }

    /// Get CAS store
    pub fn cas(&self) -> &CasStore {
        &self.cas
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

    #[test]
    fn test_file_deployer_deploy() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Store content in CAS
        let content = b"#!/bin/bash\necho 'test'\n";
        let hash = deployer.cas().store(content).unwrap();

        // Deploy file
        deployer.deploy_file("/usr/bin/test.sh", &hash, 0o755).unwrap();

        // Verify file exists
        assert!(deployer.file_exists("/usr/bin/test.sh"));

        // Verify content
        let target_path = install_root.join("usr/bin/test.sh");
        let deployed_content = fs::read(&target_path).unwrap();
        assert_eq!(content, deployed_content.as_slice());

        // Verify permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(&target_path).unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o755);
        }
    }

    #[test]
    fn test_file_deployer_verify() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Store and deploy
        let content = b"test content";
        let hash = deployer.cas().store(content).unwrap();
        deployer.deploy_file("/test.txt", &hash, 0o644).unwrap();

        // Verify matches
        assert!(deployer.verify_file("/test.txt", &hash).unwrap());

        // Modify file
        let target_path = install_root.join("test.txt");
        fs::write(&target_path, b"modified content").unwrap();

        // Verify should fail
        assert!(!deployer.verify_file("/test.txt", &hash).unwrap());
    }

    #[test]
    fn test_file_deployer_remove() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Store and deploy
        let content = b"to be removed";
        let hash = deployer.cas().store(content).unwrap();
        deployer.deploy_file("/remove_me.txt", &hash, 0o644).unwrap();

        assert!(deployer.file_exists("/remove_me.txt"));

        // Remove
        deployer.remove_file("/remove_me.txt").unwrap();
        assert!(!deployer.file_exists("/remove_me.txt"));
    }
}
