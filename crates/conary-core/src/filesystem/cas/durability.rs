// conary-core/src/filesystem/cas/durability.rs

//! Filesystem-wide durability barriers shared by transaction-owned CAS batches.

use crate::error::Result;
use std::fs;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::path::Path;

#[cfg(target_os = "linux")]
pub(super) fn sync_filesystem(path: &Path) -> Result<()> {
    let directory = fs::File::open(path)?;
    let result = unsafe { libc::syncfs(directory.as_raw_fd()) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn sync_filesystem(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}
