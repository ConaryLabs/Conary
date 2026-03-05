// src/filesystem/reflink.rs

//! Reflink (CoW clone) support for efficient file deployment
//!
//! On filesystems that support it (Btrfs, XFS, bcachefs), reflinks create
//! copy-on-write clones that share data blocks until modified. This is faster
//! than copying and more flexible than hardlinks (independent metadata,
//! independent modification).
//!
//! Falls back gracefully when the filesystem doesn't support FICLONE.

use std::fs;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use anyhow::{Context, Result};
use tracing::debug;

/// Linux FICLONE ioctl number — creates a CoW clone of an entire file.
const FICLONE: libc::c_ulong = 0x4004_9409;

/// Attempt to reflink (CoW clone) a file from `src` to `dst`.
///
/// Uses the FICLONE ioctl to create a copy-on-write clone. On success the
/// destination shares physical blocks with the source until either file is
/// modified.
///
/// On failure the destination file is cleaned up and an error is returned.
/// Permissions from the source file are preserved on the destination.
pub fn reflink_file(src: &Path, dst: &Path) -> Result<()> {
    let src_file = fs::File::open(src)
        .with_context(|| format!("Failed to open reflink source: {}", src.display()))?;

    let dst_file = fs::File::create(dst)
        .with_context(|| format!("Failed to create reflink destination: {}", dst.display()))?;

    // SAFETY: FICLONE is a well-defined Linux ioctl that operates on two open
    // file descriptors. Both fds are valid because we just opened them above.
    let ret =
        unsafe { libc::ioctl(dst_file.as_raw_fd(), FICLONE, src_file.as_raw_fd()) };

    if ret != 0 {
        let err = std::io::Error::last_os_error();
        // Drop the destination handle before removing
        drop(dst_file);
        // Best-effort cleanup — ignore removal errors
        let _ = fs::remove_file(dst);
        return Err(err).with_context(|| {
            format!(
                "FICLONE ioctl failed: {} -> {}",
                src.display(),
                dst.display()
            )
        });
    }

    // Preserve source permissions on the clone
    let src_meta = src_file.metadata().with_context(|| {
        format!("Failed to read source metadata: {}", src.display())
    })?;
    dst_file
        .set_permissions(src_meta.permissions())
        .with_context(|| {
            format!(
                "Failed to set permissions on reflink destination: {}",
                dst.display()
            )
        })?;

    debug!(
        "Reflinked {} -> {}",
        src.display(),
        dst.display()
    );

    Ok(())
}

/// Check whether the directory's filesystem supports reflinks.
///
/// Creates a temporary file pair, attempts FICLONE, then cleans up.
/// Returns `true` if reflinks are supported, `false` otherwise.
pub fn supports_reflinks(dir: &Path) -> bool {
    let src_path = dir.join(".conary-reflink-test-src");
    let dst_path = dir.join(".conary-reflink-test-dst");

    // Best-effort: create a small test file
    let ok = (|| -> Result<bool> {
        fs::write(&src_path, b"reflink probe")?;
        let result = reflink_file(&src_path, &dst_path).is_ok();
        Ok(result)
    })();

    // Always clean up, ignore errors
    let _ = fs::remove_file(&dst_path);
    let _ = fs::remove_file(&src_path);

    match ok {
        Ok(supported) => {
            debug!("Reflink support for {}: {}", dir.display(), supported);
            supported
        }
        Err(e) => {
            debug!(
                "Reflink probe failed for {}: {}",
                dir.display(),
                e
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_supports_reflinks_returns_bool() {
        // Just verifies the function runs without panicking on any filesystem.
        let tmp = TempDir::new().unwrap();
        let _result = supports_reflinks(tmp.path());
        // No assertion on the value — tmpfs typically returns false, Btrfs returns true.
    }

    #[test]
    fn test_reflink_file_fails_on_tmpfs() {
        // tmpfs does not support FICLONE, so this should fail and clean up dst.
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src.txt");
        let dst = tmp.path().join("dst.txt");

        fs::write(&src, b"test content").unwrap();

        let result = reflink_file(&src, &dst);

        // On tmpfs this will fail (EOPNOTSUPP / ENOTTY)
        if result.is_err() {
            // Destination must have been cleaned up
            assert!(
                !dst.exists(),
                "Destination file should be cleaned up on reflink failure"
            );
        }
        // If we happen to run on Btrfs/XFS, success is also fine.
    }
}
