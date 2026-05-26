// conary-core/src/filesystem/durable.rs

use crate::{Error, Result};
use std::fs::OpenOptions;
use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::sync_parent_directory;
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
}
