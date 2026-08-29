// conary-core/src/filesystem/cas/verified_batch/durability.rs

//! Durability ordering for permanent verified-object transactions.

use super::super::CasStore;
use super::{StagedObject, VerifiedObjectBatchMetrics};
use crate::error::Result;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(not(target_os = "linux"))]
use std::fs;
use std::path::PathBuf;

pub(super) trait VerifiedObjectDurability {
    fn sync_staged_data(
        &mut self,
        cas: &CasStore,
        staged: &BTreeMap<String, StagedObject>,
        metrics: &mut VerifiedObjectBatchMetrics,
    ) -> Result<()>;

    fn sync_canonical_names(
        &mut self,
        cas: &CasStore,
        staged: &BTreeMap<String, StagedObject>,
        touched_shards: &BTreeSet<PathBuf>,
        new_shards: &BTreeSet<PathBuf>,
        metrics: &mut VerifiedObjectBatchMetrics,
    ) -> Result<()>;
}

pub(super) struct FilesystemVerifiedObjectDurability;

impl VerifiedObjectDurability for FilesystemVerifiedObjectDurability {
    fn sync_staged_data(
        &mut self,
        cas: &CasStore,
        staged: &BTreeMap<String, StagedObject>,
        metrics: &mut VerifiedObjectBatchMetrics,
    ) -> Result<()> {
        if staged.is_empty() {
            return Ok(());
        }

        #[cfg(target_os = "linux")]
        super::super::durability::sync_filesystem(cas.objects_dir())?;

        #[cfg(not(target_os = "linux"))]
        for object in staged.values() {
            fs::File::open(&object.temp_path)?.sync_all()?;
            metrics.fallback_object_syncs += 1;
        }

        metrics.staged_data_barriers += 1;
        Ok(())
    }

    fn sync_canonical_names(
        &mut self,
        cas: &CasStore,
        staged: &BTreeMap<String, StagedObject>,
        touched_shards: &BTreeSet<PathBuf>,
        new_shards: &BTreeSet<PathBuf>,
        metrics: &mut VerifiedObjectBatchMetrics,
    ) -> Result<()> {
        if staged.is_empty() {
            return Ok(());
        }

        #[cfg(target_os = "linux")]
        {
            let _ = (touched_shards, new_shards);
            super::super::durability::sync_filesystem(cas.objects_dir())?;
        }

        #[cfg(not(target_os = "linux"))]
        {
            for shard in touched_shards {
                fs::File::open(shard)?.sync_all()?;
                metrics.fallback_directory_syncs += 1;
            }
            if !new_shards.is_empty() {
                fs::File::open(cas.objects_dir())?.sync_all()?;
                metrics.fallback_directory_syncs += 1;
            }
        }

        metrics.canonical_name_barriers += 1;
        Ok(())
    }
}
