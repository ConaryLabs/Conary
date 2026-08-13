// apps/conary/src/commands/live_root/durability.rs

//! Filesystem mutation helpers that make parent-directory durability explicit.

use anyhow::{Context, Result};
use conary_core::filesystem::durable::sync_parent_directory;
use std::fs::{self, File};
use std::io;
use std::path::Path;

pub(super) fn rename_and_sync(source: &Path, target: &Path) -> io::Result<()> {
    fs::rename(source, target)?;
    sync_parent_directory_io(target)?;
    if let (Some(source_parent), Some(target_parent)) = (source.parent(), target.parent())
        && source_parent != target_parent
    {
        sync_directory_io(source_parent)?;
    }
    Ok(())
}

pub(super) fn remove_file_and_sync(path: &Path) -> io::Result<()> {
    fs::remove_file(path)?;
    sync_parent_directory_io(path)
}

pub(super) fn remove_dir_and_sync(path: &Path) -> io::Result<()> {
    fs::remove_dir(path)?;
    sync_parent_directory_io(path)
}

pub(super) fn remove_dir_all_and_sync(path: &Path) -> io::Result<()> {
    fs::remove_dir_all(path)?;
    sync_parent_directory_io(path)
}

pub(super) fn create_dir_and_sync(path: &Path) -> io::Result<()> {
    fs::create_dir(path)?;
    sync_parent_directory_io(path)
}

pub(super) fn create_dir_all_and_sync(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    sync_parent_directory_io(path)
}

fn sync_parent_directory_io(path: &Path) -> io::Result<()> {
    sync_parent_directory(path).map_err(|error| io::Error::other(error.to_string()))
}

fn sync_directory_io(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

pub(super) fn sync_directory(path: &Path) -> Result<()> {
    sync_directory_io(path).with_context(|| format!("Failed to sync {}", path.display()))?;
    Ok(())
}
