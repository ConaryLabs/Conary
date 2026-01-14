// src/filesystem/deployer.rs

//! File deployment manager
//!
//! Deploys files from CAS to the filesystem using hardlinks for efficiency.
//! Falls back to copying when hardlinks aren't possible (cross-device, etc).
//!
//! Hardlink benefits:
//! - Zero additional disk space for deployed files
//! - Instant deployment (no I/O for content)
//! - Automatic deduplication across all packages

use crate::error::Result;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use tracing::{debug, info, warn};

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

    /// Validate and compute a safe target path within the install root
    ///
    /// This function prevents path traversal attacks by:
    /// 1. Normalizing the path to remove `.` and `..` components
    /// 2. Verifying the result stays within install_root
    ///
    /// Returns an error if the path would escape the install root.
    fn safe_target_path(&self, path: &str) -> Result<PathBuf> {
        // Strip leading slashes and normalize the path
        let relative_path = path.trim_start_matches('/');

        // Build normalized path by processing each component
        let mut normalized = PathBuf::new();
        for component in Path::new(relative_path).components() {
            match component {
                Component::Normal(c) => normalized.push(c),
                Component::CurDir => {} // Skip "."
                Component::ParentDir => {
                    // Reject ".." - don't try to resolve, just fail
                    warn!("Path traversal attempt detected: {}", path);
                    return Err(crate::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("Path traversal detected: {}", path),
                    )));
                }
                Component::Prefix(_) | Component::RootDir => {
                    // Skip absolute path markers (we already stripped leading /)
                }
            }
        }

        // Reject empty paths
        if normalized.as_os_str().is_empty() {
            return Err(crate::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Empty path after normalization",
            )));
        }

        let target_path = self.install_root.join(&normalized);

        // Final safety check: ensure the path starts with install_root
        // This catches edge cases and provides defense in depth
        if !target_path.starts_with(&self.install_root) {
            warn!(
                "Path escaped install root: {} -> {:?}",
                path, target_path
            );
            return Err(crate::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Path escapes install root: {}", path),
            )));
        }

        Ok(target_path)
    }

    /// Deploy a file from CAS to the filesystem
    ///
    /// Uses hardlinks for efficiency (zero disk space, instant deployment).
    /// Falls back to copying if hardlinks aren't possible (cross-device, etc).
    ///
    /// - Creates hardlink from CAS to install_root + path
    /// - Sets permissions (ownership requires root)
    pub fn deploy_file(
        &self,
        path: &str,
        hash: &str,
        permissions: u32,
    ) -> Result<()> {
        // Get CAS path for this content
        let cas_path = self.cas.hash_to_path(hash);

        if !cas_path.exists() {
            return Err(crate::Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Content not found in CAS: {}", hash),
            )));
        }

        // Compute target path with traversal validation
        let target_path = self.safe_target_path(path)?;

        // Create parent directories
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Remove existing file if present (hardlink requires target not to exist)
        if target_path.exists() {
            fs::remove_file(&target_path)?;
        }

        // Try hardlink first, fall back to copy
        let method = if self.try_hardlink(&cas_path, &target_path) {
            "hardlink"
        } else {
            // Hardlink failed, fall back to copy
            debug!(
                "Hardlink failed for {}, falling back to copy",
                path
            );
            self.copy_from_cas(hash, &target_path)?;
            "copy"
        };

        // Set permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(permissions);
            fs::set_permissions(&target_path, perms)?;
        }

        info!(
            "Deployed file: {} (hash: {}, mode: {:o}, method: {})",
            path, hash, permissions, method
        );
        Ok(())
    }

    /// Try to create a hardlink, returns true if successful
    fn try_hardlink(&self, source: &Path, target: &Path) -> bool {
        fs::hard_link(source, target).is_ok()
    }

    /// Copy file content from CAS to target (fallback when hardlink fails)
    fn copy_from_cas(&self, hash: &str, target_path: &Path) -> Result<()> {
        let content = self.cas.retrieve(hash)?;

        // Write atomically
        let temp_path = target_path.with_extension("conary-tmp");
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(&content)?;
        file.sync_all()?;

        // Atomic rename
        fs::rename(&temp_path, target_path)?;

        Ok(())
    }

    /// Check if a file exists at the target path
    pub fn file_exists(&self, path: &str) -> bool {
        match self.safe_target_path(path) {
            Ok(target_path) => target_path.exists(),
            Err(_) => false, // Path traversal attempts return false
        }
    }

    /// Deploy a symlink to the filesystem
    ///
    /// Creates a symbolic link at the target path pointing to the given target.
    pub fn deploy_symlink(&self, path: &str, target: &str) -> Result<()> {
        let link_path = self.safe_target_path(path)?;

        // Create parent directories
        if let Some(parent) = link_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Remove existing file/symlink if present
        if link_path.exists() || link_path.symlink_metadata().is_ok() {
            if link_path.is_dir() {
                // Don't remove directories
                debug!("Skipping symlink deployment over directory: {}", path);
                return Ok(());
            }
            fs::remove_file(&link_path)?;
        }

        // Create symlink
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, &link_path)?;
        }

        #[cfg(not(unix))]
        {
            return Err(crate::Error::Io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Symlinks not supported on this platform",
            )));
        }

        info!("Deployed symlink: {} -> {}", path, target);
        Ok(())
    }

    /// Deploy a file or symlink from CAS based on stored content
    ///
    /// This method checks if the hash represents a symlink (prefixed content)
    /// and deploys accordingly.
    pub fn deploy_auto(&self, path: &str, hash: &str, permissions: u32) -> Result<()> {
        // Check if this is a symlink
        if let Ok(Some(target)) = self.cas.retrieve_symlink(hash) {
            return self.deploy_symlink(path, &target);
        }

        // Otherwise deploy as regular file
        self.deploy_file(path, hash, permissions)
    }

    /// Remove a file from the filesystem
    pub fn remove_file(&self, path: &str) -> Result<()> {
        let target_path = self.safe_target_path(path)?;

        if target_path.exists() {
            if target_path.is_dir() {
                // Skip directories - they should be removed with remove_directory
                debug!("Skipping directory in remove_file: {}", path);
                return Ok(());
            }
            fs::remove_file(&target_path)?;
            info!("Removed file: {}", path);
        } else {
            debug!("File already removed: {}", path);
        }

        Ok(())
    }

    /// Remove an empty directory from the filesystem
    ///
    /// Only removes the directory if it's empty. Returns Ok(true) if removed,
    /// Ok(false) if not empty or doesn't exist.
    pub fn remove_directory(&self, path: &str) -> Result<bool> {
        let target_path = self.safe_target_path(path)?;

        if !target_path.exists() {
            debug!("Directory already removed: {}", path);
            return Ok(false);
        }

        if !target_path.is_dir() {
            debug!("Path is not a directory: {}", path);
            return Ok(false);
        }

        // Try to remove - will fail if not empty
        match fs::remove_dir(&target_path) {
            Ok(()) => {
                info!("Removed directory: {}", path);
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                debug!("Directory not empty, skipping: {}", path);
                Ok(false)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Verify a file's hash matches expected
    pub fn verify_file(&self, path: &str, expected_hash: &str) -> Result<bool> {
        let target_path = self.safe_target_path(path)?;

        if !target_path.exists() {
            return Ok(false);
        }

        let mut file = fs::File::open(&target_path)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;

        let actual_hash = self.cas.compute_hash(&content);
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

    #[test]
    #[cfg(unix)]
    fn test_file_deployer_uses_hardlinks() {
        use std::os::unix::fs::MetadataExt;

        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Store content in CAS
        let content = b"hardlink test content";
        let hash = deployer.cas().store(content).unwrap();

        // Deploy file
        deployer
            .deploy_file("/hardlink_test.txt", &hash, 0o644)
            .unwrap();

        // Get inodes for both files
        let cas_path = deployer.cas().hash_to_path(&hash);
        let target_path = install_root.join("hardlink_test.txt");

        let cas_inode = fs::metadata(&cas_path).unwrap().ino();
        let target_inode = fs::metadata(&target_path).unwrap().ino();

        // Should be the same inode (hardlink)
        assert_eq!(
            cas_inode, target_inode,
            "Deployed file should be hardlinked to CAS (same inode)"
        );

        // Verify nlink count is 2 (CAS + deployed)
        let nlink = fs::metadata(&cas_path).unwrap().nlink();
        assert_eq!(nlink, 2, "Hardlinked file should have nlink=2");
    }

    #[test]
    #[cfg(unix)]
    fn test_file_deployer_hardlink_deduplication() {
        use std::os::unix::fs::MetadataExt;

        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Store content once
        let content = b"shared content across packages";
        let hash = deployer.cas().store(content).unwrap();

        // Deploy same content to multiple locations (simulating multiple packages)
        deployer.deploy_file("/pkg1/shared.txt", &hash, 0o644).unwrap();
        deployer.deploy_file("/pkg2/shared.txt", &hash, 0o644).unwrap();
        deployer.deploy_file("/pkg3/shared.txt", &hash, 0o644).unwrap();

        // All should share the same inode
        let cas_path = deployer.cas().hash_to_path(&hash);
        let cas_inode = fs::metadata(&cas_path).unwrap().ino();

        for path in &["/pkg1/shared.txt", "/pkg2/shared.txt", "/pkg3/shared.txt"] {
            let target_path = install_root.join(path.trim_start_matches('/'));
            let target_inode = fs::metadata(&target_path).unwrap().ino();
            assert_eq!(
                cas_inode, target_inode,
                "All deployed files should share the same inode: {}",
                path
            );
        }

        // nlink should be 4 (CAS + 3 deployed)
        let nlink = fs::metadata(&cas_path).unwrap().nlink();
        assert_eq!(nlink, 4, "Should have 4 hardlinks total");
    }

    #[test]
    fn test_path_traversal_dotdot_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Store content
        let content = b"malicious content";
        let hash = deployer.cas().store(content).unwrap();

        // Various path traversal attempts should fail
        let traversal_paths = [
            "../etc/passwd",
            "../../etc/shadow",
            "/foo/../../../etc/passwd",
            "usr/bin/../../../etc/passwd",
            "foo/bar/../../../baz",
        ];

        for path in &traversal_paths {
            let result = deployer.deploy_file(path, &hash, 0o644);
            assert!(
                result.is_err(),
                "Path traversal should be rejected: {}",
                path
            );
        }
    }

    #[test]
    fn test_path_traversal_file_exists_returns_false() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Path traversal attempts should return false, not panic or return true
        assert!(!deployer.file_exists("../etc/passwd"));
        assert!(!deployer.file_exists("../../etc/shadow"));
    }

    #[test]
    fn test_path_traversal_remove_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Remove with traversal should fail
        let result = deployer.remove_file("../../../etc/passwd");
        assert!(result.is_err());

        let result = deployer.remove_directory("../../../etc");
        assert!(result.is_err());
    }

    #[test]
    fn test_path_traversal_symlink_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Symlink with traversal path should fail
        let result = deployer.deploy_symlink("../../../etc/passwd", "/some/target");
        assert!(result.is_err());
    }

    #[test]
    fn test_path_traversal_verify_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Verify with traversal should fail
        let result = deployer.verify_file("../../../etc/passwd", "somehash");
        assert!(result.is_err());
    }

    #[test]
    fn test_valid_paths_still_work() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Store content
        let content = b"valid content";
        let hash = deployer.cas().store(content).unwrap();

        // These should all work
        deployer.deploy_file("/usr/bin/test", &hash, 0o755).unwrap();
        deployer.deploy_file("usr/local/bin/test2", &hash, 0o755).unwrap();
        deployer.deploy_file("/var/lib/app/data.txt", &hash, 0o644).unwrap();

        // Verify they exist
        assert!(deployer.file_exists("/usr/bin/test"));
        assert!(deployer.file_exists("usr/local/bin/test2"));
        assert!(deployer.file_exists("/var/lib/app/data.txt"));
    }

    #[test]
    fn test_empty_path_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        let content = b"content";
        let hash = deployer.cas().store(content).unwrap();

        // Empty paths should fail
        let result = deployer.deploy_file("", &hash, 0o644);
        assert!(result.is_err());

        let result = deployer.deploy_file("/", &hash, 0o644);
        assert!(result.is_err());
    }
}
