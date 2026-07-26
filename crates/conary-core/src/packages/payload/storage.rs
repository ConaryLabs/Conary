// conary-core/src/packages/payload/storage.rs

//! Temporary payload storage and exact u64 disk-capacity preflight.

use super::ReopenablePayload;
use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct PayloadSpool {
    temp: Arc<tempfile::TempDir>,
}

impl PayloadSpool {
    pub fn new(required_bytes: u64) -> Result<Self> {
        let temp = Arc::new(tempfile::tempdir()?);
        ensure_available_space(temp.path(), required_bytes)?;
        Ok(Self { temp })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        self.temp.path()
    }

    /// Return a deterministic spool path for a parser-owned numeric identity.
    #[must_use]
    pub fn indexed_path(&self, index: usize) -> PathBuf {
        self.temp.path().join(format!("{index:016x}.payload"))
    }

    #[must_use]
    pub fn source(&self, path: impl Into<PathBuf>) -> ReopenablePayload {
        ReopenablePayload::from_spooled_path(path, Arc::clone(&self.temp))
    }
}

pub fn ensure_available_space(path: &Path, required_bytes: u64) -> Result<()> {
    let available = available_space(path)?;
    ensure_capacity(required_bytes, available)
}

fn ensure_capacity(required_bytes: u64, available_bytes: u64) -> Result<()> {
    if required_bytes > available_bytes {
        return Err(Error::IoError(format!(
            "payload spool requires {required_bytes} bytes, but only {available_bytes} bytes are available"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn available_space(path: &Path) -> Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::InvalidPath("payload spool path contains NUL".to_string()))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `stats` points to writable storage.
    let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: successful statvfs initialized the structure.
    let stats = unsafe { stats.assume_init() };
    stats
        .f_bavail
        .checked_mul(stats.f_frsize)
        .ok_or_else(|| Error::IoError("available-space arithmetic overflow".to_string()))
}

#[cfg(not(unix))]
fn available_space(_path: &Path) -> Result<u64> {
    Ok(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_file_larger_than_u32_reaches_u64_preflight() {
        let temp = tempfile::tempdir().unwrap();
        let sparse = temp.path().join("sparse");
        let declared = u64::from(u32::MAX) + 64 * 1024;
        std::fs::File::create(&sparse)
            .unwrap()
            .set_len(declared)
            .unwrap();
        let measured = std::fs::metadata(sparse).unwrap().len();

        assert_eq!(measured, declared);
        ensure_capacity(measured, declared).unwrap();
        assert!(ensure_capacity(measured, declared - 1).is_err());
    }
}
