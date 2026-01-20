// src/daemon/lock.rs

//! System-wide exclusive lock for daemon operations
//!
//! The daemon holds this lock for its entire lifetime, ensuring only one
//! daemon instance can run at a time. The CLI checks for this lock to
//! determine if it should forward commands to the daemon or operate directly.
//!
//! # Lock Strategy
//!
//! - **Lifetime Lock**: `/var/lib/conary/daemon.lock` - held while daemon runs
//! - **Commit Lock**: Transaction-level locks in TransactionEngine (separate)
//!
//! # Example
//!
//! ```ignore
//! use conary::daemon::lock::SystemLock;
//!
//! // Daemon startup
//! let lock = SystemLock::acquire("/var/lib/conary/daemon.lock")?;
//!
//! // ... daemon runs ...
//!
//! // Lock automatically released on drop
//! ```

use crate::Result;
use fs2::FileExt;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

/// System-wide exclusive lock
///
/// This lock ensures only one daemon instance can run at a time.
/// The lock is held using `flock(LOCK_EX)` for the daemon's entire lifetime.
pub struct SystemLock {
    /// The lock file handle (kept open to maintain lock)
    #[allow(dead_code)]
    file: File,
    /// Path to the lock file
    path: PathBuf,
}

impl SystemLock {
    /// Default lock path for the daemon
    pub const DEFAULT_PATH: &'static str = "/var/lib/conary/daemon.lock";

    /// Acquire an exclusive lock, blocking until available
    ///
    /// This will block if another process holds the lock.
    /// Use `try_acquire` for non-blocking behavior.
    pub fn acquire<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = File::create(&path)?;

        // Block until lock is acquired
        file.lock_exclusive()
            .map_err(|e| crate::Error::IoError(format!("Failed to acquire system lock: {}", e)))?;

        log::info!("Acquired system lock at {:?}", path);

        Ok(Self { file, path })
    }

    /// Try to acquire an exclusive lock without blocking
    ///
    /// Returns:
    /// - `Ok(Some(lock))` if lock was acquired
    /// - `Ok(None)` if lock is held by another process
    /// - `Err` on I/O errors
    pub fn try_acquire<P: AsRef<Path>>(path: P) -> Result<Option<Self>> {
        let path = path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = File::create(&path)?;

        match file.try_lock_exclusive() {
            Ok(()) => {
                log::info!("Acquired system lock at {:?}", path);
                Ok(Some(Self { file, path }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                log::debug!("System lock already held at {:?}", path);
                Ok(None)
            }
            Err(e) => Err(crate::Error::IoError(format!(
                "Failed to try-acquire system lock: {}",
                e
            ))),
        }
    }

    /// Check if a lock is currently held (by any process)
    ///
    /// This is a non-destructive check that doesn't acquire the lock.
    /// Useful for CLI to check if daemon is running.
    pub fn is_held<P: AsRef<Path>>(path: P) -> bool {
        let path = path.as_ref();

        if !path.exists() {
            return false;
        }

        // Try to open and lock - if we can't, someone else has it
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return false,
        };

        match file.try_lock_exclusive() {
            Ok(()) => {
                // We got the lock, so no one else had it
                let _ = file.unlock();
                false
            }
            Err(_) => {
                // Lock is held by someone else
                true
            }
        }
    }

    /// Get the path to the lock file
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the PID of the process holding the lock (if available)
    ///
    /// Note: This reads from a .pid file that the daemon should write.
    /// The lock itself doesn't track the PID.
    pub fn holder_pid<P: AsRef<Path>>(lock_path: P) -> Option<u32> {
        let pid_path = lock_path.as_ref().with_extension("pid");
        if pid_path.exists() {
            fs::read_to_string(&pid_path)
                .ok()
                .and_then(|s| s.trim().parse().ok())
        } else {
            None
        }
    }

    /// Write our PID to the .pid file
    ///
    /// Call this after acquiring the lock to help identify the holder.
    pub fn write_pid(&self) -> Result<()> {
        let pid_path = self.path.with_extension("pid");
        let pid = std::process::id();
        fs::write(&pid_path, pid.to_string())?;
        Ok(())
    }

    /// Remove the .pid file
    fn remove_pid(&self) {
        let pid_path = self.path.with_extension("pid");
        let _ = fs::remove_file(pid_path);
    }
}

impl Drop for SystemLock {
    fn drop(&mut self) {
        // Remove PID file first
        self.remove_pid();

        // Lock is automatically released when file is closed
        log::info!("Released system lock at {:?}", self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_acquire_lock() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = temp_dir.path().join("test.lock");

        let lock = SystemLock::acquire(&lock_path).unwrap();
        assert!(lock_path.exists());
        assert!(SystemLock::is_held(&lock_path));

        drop(lock);
        assert!(!SystemLock::is_held(&lock_path));
    }

    #[test]
    fn test_try_acquire_success() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = temp_dir.path().join("test.lock");

        let lock = SystemLock::try_acquire(&lock_path).unwrap();
        assert!(lock.is_some());

        let lock = lock.unwrap();
        assert!(lock_path.exists());
    }

    #[test]
    fn test_try_acquire_fails_when_held() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = temp_dir.path().join("test.lock");

        // Acquire lock
        let _lock1 = SystemLock::acquire(&lock_path).unwrap();

        // Try to acquire again should fail
        let lock2 = SystemLock::try_acquire(&lock_path).unwrap();
        assert!(lock2.is_none());
    }

    #[test]
    fn test_is_held_when_no_file() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = temp_dir.path().join("nonexistent.lock");

        assert!(!SystemLock::is_held(&lock_path));
    }

    #[test]
    fn test_pid_file() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = temp_dir.path().join("test.lock");

        let lock = SystemLock::acquire(&lock_path).unwrap();
        lock.write_pid().unwrap();

        let pid = SystemLock::holder_pid(&lock_path).unwrap();
        assert_eq!(pid, std::process::id());

        drop(lock);

        // PID file should be removed on drop
        assert!(SystemLock::holder_pid(&lock_path).is_none());
    }

    #[test]
    fn test_creates_parent_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = temp_dir.path().join("subdir/deep/test.lock");

        let lock = SystemLock::acquire(&lock_path).unwrap();
        assert!(lock_path.exists());
        assert!(lock_path.parent().unwrap().exists());
    }
}
