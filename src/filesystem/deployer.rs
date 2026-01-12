// src/filesystem/deployer.rs

//! File deployment manager
//!
//! Deploys files from CAS to the filesystem with atomic writes
//! and permission management.

use crate::error::Result;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::CasStore;

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
