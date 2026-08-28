// apps/conary/src/commands/live_root/durability.rs

//! Filesystem mutation helpers that make parent-directory durability explicit.

use anyhow::{Context, Result};
use conary_core::filesystem::durable::sync_parent_directory;
use std::fs::{self, File};
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LiveRootDurabilityMetrics {
    pub(crate) file_syncs: u64,
    pub(crate) directory_syncs: u64,
    pub(crate) deferred_file_syncs: u64,
    pub(crate) deferred_directory_syncs: u64,
}

/// Proof that a disposable upper completed all node mutations without
/// claiming immediate durability. The selected-root owner must consume this
/// token at its filesystem-wide freeze boundary before decoding the upper.
#[derive(Debug)]
#[must_use = "deferred selected-root durability must be consumed by filesystem freeze"]
pub(crate) struct DeferredOverlayDurability {
    metrics: LiveRootDurabilityMetrics,
}

impl DeferredOverlayDurability {
    pub(crate) const fn metrics(&self) -> LiveRootDurabilityMetrics {
        self.metrics
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DurabilityBoundary {
    Immediate,
    FilesystemFreeze,
}

/// Durability policy for one mutable-root lifetime.
///
/// Journaled roots recover individual mutations and therefore make every
/// file/directory transition durable immediately. A disposable selected-root
/// upper has no recoverable authority until its owner performs the mandatory
/// `syncfs` plus strict-unmount freeze, so repeating those flushes per node is
/// deferred to that single boundary.
#[derive(Debug)]
pub(super) struct MutationDurability {
    boundary: DurabilityBoundary,
    metrics: LiveRootDurabilityMetrics,
}

impl MutationDurability {
    pub(super) const fn immediate() -> Self {
        Self {
            boundary: DurabilityBoundary::Immediate,
            metrics: LiveRootDurabilityMetrics {
                file_syncs: 0,
                directory_syncs: 0,
                deferred_file_syncs: 0,
                deferred_directory_syncs: 0,
            },
        }
    }

    pub(super) const fn filesystem_freeze() -> Self {
        Self {
            boundary: DurabilityBoundary::FilesystemFreeze,
            metrics: LiveRootDurabilityMetrics {
                file_syncs: 0,
                directory_syncs: 0,
                deferred_file_syncs: 0,
                deferred_directory_syncs: 0,
            },
        }
    }

    pub(super) const fn metrics(&self) -> LiveRootDurabilityMetrics {
        self.metrics
    }

    pub(super) fn finish_for_filesystem_freeze(&self) -> Result<DeferredOverlayDurability> {
        if self.boundary != DurabilityBoundary::FilesystemFreeze {
            anyhow::bail!("immediate live-root durability cannot be deferred to filesystem freeze");
        }
        Ok(DeferredOverlayDurability {
            metrics: self.metrics,
        })
    }

    pub(super) fn sync_file(&mut self, file: &File) -> io::Result<()> {
        match self.boundary {
            DurabilityBoundary::Immediate => {
                file.sync_all()?;
                self.metrics.file_syncs += 1;
            }
            DurabilityBoundary::FilesystemFreeze => {
                self.metrics.deferred_file_syncs += 1;
            }
        }
        Ok(())
    }

    pub(super) fn sync_directory(&mut self, path: &Path) -> io::Result<()> {
        match self.boundary {
            DurabilityBoundary::Immediate => {
                sync_directory_io(path)?;
                self.metrics.directory_syncs += 1;
            }
            DurabilityBoundary::FilesystemFreeze => {
                self.metrics.deferred_directory_syncs += 1;
            }
        }
        Ok(())
    }

    pub(super) fn sync_parent(&mut self, path: &Path) -> io::Result<()> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("path has no parent directory: {}", path.display()),
            )
        })?;
        self.sync_directory(parent)
    }

    pub(super) fn rename(&mut self, source: &Path, target: &Path) -> io::Result<()> {
        fs::rename(source, target)?;
        self.sync_parent(target)?;
        if let (Some(source_parent), Some(target_parent)) = (source.parent(), target.parent())
            && source_parent != target_parent
        {
            self.sync_directory(source_parent)?;
        }
        Ok(())
    }

    pub(super) fn remove_file(&mut self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)?;
        self.sync_parent(path)
    }

    pub(super) fn remove_dir(&mut self, path: &Path) -> io::Result<()> {
        fs::remove_dir(path)?;
        self.sync_parent(path)
    }

    pub(super) fn create_dir(&mut self, path: &Path) -> io::Result<()> {
        fs::create_dir(path)?;
        self.sync_parent(path)
    }
}

pub(super) fn rename_and_sync(source: &Path, target: &Path) -> io::Result<()> {
    MutationDurability::immediate().rename(source, target)
}

pub(super) fn remove_file_and_sync(path: &Path) -> io::Result<()> {
    MutationDurability::immediate().remove_file(path)
}

pub(super) fn remove_dir_and_sync(path: &Path) -> io::Result<()> {
    MutationDurability::immediate().remove_dir(path)
}

pub(super) fn remove_dir_all_and_sync(path: &Path) -> io::Result<()> {
    fs::remove_dir_all(path)?;
    sync_parent_directory_io(path)
}

pub(super) fn create_dir_and_sync(path: &Path) -> io::Result<()> {
    MutationDurability::immediate().create_dir(path)
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
