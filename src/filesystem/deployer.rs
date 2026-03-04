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
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, info, warn};

/// Monotonic counter for unique temp file names (prevents races during parallel deployments)
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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

    /// Create a new file deployer with an existing CAS store
    pub fn with_cas<P: AsRef<Path>>(cas: CasStore, install_root: P) -> Result<Self> {
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
            warn!("Path escaped install root: {} -> {:?}", path, target_path);
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
    pub fn deploy_file(&self, path: &str, hash: &str, permissions: u32) -> Result<()> {
        // Get CAS path for this content
        let cas_path = self.cas.hash_to_path(hash);

        // Open the CAS file first — serves as existence check AND holds a reference
        // to prevent inode reclaim between check and hardlink (TOCTOU fix)
        let cas_file = fs::File::open(&cas_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                crate::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Content not found in CAS: {}", hash),
                ))
            } else {
                crate::Error::Io(e)
            }
        })?;

        // Compute target path with traversal validation
        let target_path = self.safe_target_path(path)?;

        // Create parent directories
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Remove existing file if present (hardlink requires target not to exist).
        // Ignore NotFound to avoid a TOCTOU race on the target path.
        match fs::remove_file(&target_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }

        // Try hardlink first, fall back to copy from the already-open file handle
        let method = if self.try_hardlink(&cas_path, &target_path) {
            "hardlink"
        } else {
            // Hardlink failed, fall back to copy from the open fd
            debug!("Hardlink failed for {}, falling back to copy", path);
            self.copy_from_cas_fd(&cas_file, &target_path)?;
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

    /// Copy file content from an open CAS file handle to target
    ///
    /// Used as fallback when hardlink fails. Reads from the already-open fd
    /// rather than re-opening by hash, avoiding a second TOCTOU window.
    fn copy_from_cas_fd(&self, source: &fs::File, target_path: &Path) -> Result<()> {
        let temp_name = format!(
            ".conary-tmp.{}.{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let temp_path = target_path.with_file_name(temp_name);
        let mut target = fs::File::create(&temp_path)?;
        let mut reader: &fs::File = source;
        std::io::copy(&mut reader, &mut target)?;
        target.sync_all()?;

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
    /// Validates that the symlink target cannot escape the install root via
    /// path traversal.
    pub fn deploy_symlink(&self, path: &str, target: &str) -> Result<()> {
        let link_path = self.safe_target_path(path)?;

        // Validate symlink target
        self.validate_symlink_target(&link_path, target)?;

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

    /// Validate that a symlink target does not escape the install root
    ///
    /// For relative targets: resolves the target relative to the link's parent
    /// directory and verifies the result stays within the install root.
    /// For absolute targets: verifies the path is within the install root.
    ///
    /// Simple relative targets without `..` (e.g., `libfoo.so.1`) are always
    /// allowed since they cannot escape their parent directory.
    fn validate_symlink_target(&self, link_path: &Path, target: &str) -> Result<()> {
        let target_path = Path::new(target);

        if target_path.is_absolute() {
            // Absolute symlink target: the OS resolves this literally, so it
            // must start with the install root path. Normalize away any ".."
            // components first to prevent traversal tricks.
            let mut normalized = PathBuf::from("/");
            for component in target_path.components() {
                match component {
                    Component::Normal(c) => normalized.push(c),
                    Component::CurDir => {}
                    Component::ParentDir => {
                        normalized.pop();
                    }
                    Component::RootDir | Component::Prefix(_) => {
                        normalized = PathBuf::from("/");
                    }
                }
            }
            if !normalized.starts_with(&self.install_root) {
                warn!(
                    "Symlink target escapes install root: {} -> {}",
                    link_path.display(),
                    target
                );
                return Err(crate::Error::PathTraversal(format!(
                    "Symlink target escapes install root: {}",
                    target
                )));
            }
        } else {
            // Relative symlink target: resolve relative to the link's parent
            // directory and verify it stays within the install root.
            //
            // Quick path: if the target contains no ".." components, it cannot
            // escape upward and is always safe.
            let has_parent_traversal = target_path
                .components()
                .any(|c| matches!(c, Component::ParentDir));

            if has_parent_traversal {
                // Resolve the target relative to the link's parent directory
                let link_parent = link_path.parent().unwrap_or(&self.install_root);

                let mut resolved = link_parent.to_path_buf();
                for component in target_path.components() {
                    match component {
                        Component::Normal(c) => resolved.push(c),
                        Component::CurDir => {}
                        Component::ParentDir => {
                            resolved.pop();
                        }
                        Component::Prefix(_) | Component::RootDir => {}
                    }
                }

                if !resolved.starts_with(&self.install_root) {
                    warn!(
                        "Symlink target escapes install root: {} -> {}",
                        link_path.display(),
                        target
                    );
                    return Err(crate::Error::PathTraversal(format!(
                        "Symlink target escapes install root: {}",
                        target
                    )));
                }
            }
            // No ".." components means the symlink stays in or below the
            // link's parent directory, which is already within the install root.
        }

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
        deployer
            .deploy_file("/usr/bin/test.sh", &hash, 0o755)
            .unwrap();

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
        deployer
            .deploy_file("/remove_me.txt", &hash, 0o644)
            .unwrap();

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
        deployer
            .deploy_file("/pkg1/shared.txt", &hash, 0o644)
            .unwrap();
        deployer
            .deploy_file("/pkg2/shared.txt", &hash, 0o644)
            .unwrap();
        deployer
            .deploy_file("/pkg3/shared.txt", &hash, 0o644)
            .unwrap();

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
        deployer
            .deploy_file("usr/local/bin/test2", &hash, 0o755)
            .unwrap();
        deployer
            .deploy_file("/var/lib/app/data.txt", &hash, 0o644)
            .unwrap();

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

    #[test]
    #[cfg(unix)]
    fn test_symlink_target_relative_same_dir_allowed() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Common case: libfoo.so -> libfoo.so.1 (same directory)
        deployer
            .deploy_symlink("/usr/lib/libfoo.so", "libfoo.so.1")
            .unwrap();

        let link_path = install_root.join("usr/lib/libfoo.so");
        assert!(link_path.symlink_metadata().is_ok());
        assert_eq!(
            fs::read_link(&link_path).unwrap().to_str().unwrap(),
            "libfoo.so.1"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_symlink_target_relative_subdir_allowed() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Relative target pointing to a subdirectory (no "..")
        deployer
            .deploy_symlink("/usr/share/app/config", "defaults/config.yaml")
            .unwrap();

        let link_path = install_root.join("usr/share/app/config");
        assert!(link_path.symlink_metadata().is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn test_symlink_target_relative_parent_within_root_allowed() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Relative target with ".." that stays within the install root
        // /usr/lib/pkgconfig/../lib/libfoo.so resolves to /usr/lib/libfoo.so
        deployer
            .deploy_symlink("/usr/lib/pkgconfig/link", "../libfoo.so")
            .unwrap();

        let link_path = install_root.join("usr/lib/pkgconfig/link");
        assert!(link_path.symlink_metadata().is_ok());
    }

    #[test]
    fn test_symlink_target_relative_escapes_root_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Malicious: symlink at valid path but target escapes via ".."
        let result = deployer.deploy_symlink("/usr/lib/libevil.so", "../../../../etc/shadow");
        assert!(
            result.is_err(),
            "Symlink target escaping install root should be rejected"
        );
    }

    #[test]
    fn test_symlink_target_relative_escapes_from_root_level_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Symlink at root-level directory: even one ".." escapes
        let result = deployer.deploy_symlink("/link", "../etc/shadow");
        assert!(
            result.is_err(),
            "Symlink target escaping from root-level path should be rejected"
        );
    }

    #[test]
    fn test_symlink_target_absolute_outside_root_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Absolute target pointing outside install root
        let result = deployer.deploy_symlink("/usr/lib/libevil.so", "/etc/shadow");
        assert!(
            result.is_err(),
            "Absolute symlink target outside install root should be rejected"
        );
    }

    #[test]
    fn test_symlink_target_absolute_with_traversal_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Absolute target with ".." traversal
        let result = deployer.deploy_symlink("/usr/lib/libevil.so", "/usr/lib/../../../etc/shadow");
        assert!(
            result.is_err(),
            "Absolute symlink target with traversal should be rejected"
        );
    }

    #[test]
    fn test_symlink_target_many_dotdots_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let install_root = temp_dir.path().join("root");
        let objects_dir = temp_dir.path().join("objects");

        let deployer = FileDeployer::new(&objects_dir, &install_root).unwrap();

        // Many levels of ".." traversal
        let result = deployer.deploy_symlink(
            "/usr/lib/deep/nested/dir/link",
            "../../../../../../../../../../etc/shadow",
        );
        assert!(
            result.is_err(),
            "Symlink target with excessive traversal should be rejected"
        );
    }
}
