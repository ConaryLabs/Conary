// apps/conary/src/commands/try_session/util.rs
//! Private shared filesystem helpers for try-session modules.

#[cfg(test)]
use anyhow::bail;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub(super) fn remove_dir_if_exists(path: PathBuf) -> Result<()> {
    #[cfg(test)]
    if let Some(fail_path) = std::env::var_os("CONARY_TEST_TRY_REMOVE_DIR_FAIL")
        && Path::new(&fail_path) == path
    {
        bail!(
            "forced try directory removal failure for {}",
            path.display()
        );
    }

    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

pub(super) fn remove_path_if_exists(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .with_context(|| format!("failed to remove {}", path.display()))
}
