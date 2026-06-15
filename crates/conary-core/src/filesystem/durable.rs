// conary-core/src/filesystem/durable.rs

use crate::{Error, Result};
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

fn temp_path_for(path: &Path) -> PathBuf {
    match path.extension() {
        Some(extension) => path.with_extension(format!("{}.tmp", extension.to_string_lossy())),
        None => path.with_extension("tmp"),
    }
}

pub fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            Error::IoError(format!("Path has no parent directory: {}", path.display()))
        })?;
    let dir = OpenOptions::new()
        .read(true)
        .open(parent)
        .map_err(|error| {
            Error::IoError(format!(
                "Failed to open parent directory {} for sync: {error}",
                parent.display()
            ))
        })?;
    dir.sync_all().map_err(|error| {
        Error::IoError(format!(
            "Failed to sync parent directory {}: {error}",
            parent.display()
        ))
    })
}

pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path_for(path);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    sync_parent_directory(path)
}

pub fn write_file_atomic_with_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path_for(path);
    {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(mode)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, path)?;
    sync_parent_directory(path)
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| Error::InternalError(format!("failed to serialize JSON: {error}")))?;
    write_file_atomic(path, &bytes)
}

pub fn write_json_atomic_with_mode<T: Serialize>(
    path: &Path,
    value: &T,
    mode: u32,
) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| Error::InternalError(format!("failed to serialize JSON: {error}")))?;
    write_file_atomic_with_mode(path, &bytes, mode)
}

pub fn remove_file_and_sync_parent(path: &Path) -> Result<()> {
    std::fs::remove_file(path)?;
    sync_parent_directory(path)
}

#[cfg(test)]
mod tests {
    use super::{sync_parent_directory, write_json_atomic, write_json_atomic_with_mode};
    use tempfile::TempDir;

    #[test]
    fn sync_parent_directory_succeeds_for_existing_parent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("current");
        std::fs::write(&path, b"fixture").unwrap();
        sync_parent_directory(&path).unwrap();
    }

    #[test]
    fn sync_parent_directory_rejects_path_without_parent() {
        let error = sync_parent_directory(std::path::Path::new("current"))
            .expect_err("relative path without parent should fail");
        assert!(error.to_string().contains("no parent directory"));
    }

    #[test]
    fn write_json_atomic_writes_pretty_json_and_syncs_parent() {
        #[derive(serde::Serialize)]
        struct Fixture {
            name: &'static str,
        }

        let temp = TempDir::new().unwrap();
        let path = temp.path().join("record.json");
        write_json_atomic(&path, &Fixture { name: "fixture" }).unwrap();

        let raw = std::fs::read_to_string(path).unwrap();
        assert!(raw.contains("\"fixture\""));
        assert!(!temp.path().join("record.json.tmp").exists());
    }

    #[test]
    fn write_json_atomic_with_mode_uses_requested_mode() {
        use std::os::unix::fs::PermissionsExt;

        #[derive(serde::Serialize)]
        struct Fixture {
            name: &'static str,
        }

        let temp = TempDir::new().unwrap();
        let path = temp.path().join("private.json");
        write_json_atomic_with_mode(&path, &Fixture { name: "private" }, 0o600).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!temp.path().join("private.json.tmp").exists());
    }
}
