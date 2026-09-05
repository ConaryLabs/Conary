// crates/conary-core/src/transaction/recovery/runtime.rs

//! Host operations at the recovery boundary, injectable without bypassing
//! artifact validation, policy, metadata persistence, or selection logic.

use std::path::Path;

use crate::filesystem::fsverity::{FsVerityError, enable_fsverity};
use crate::generation::builder::{BuildResult, rebuild_generation_image};
use crate::generation::mount::{GenerationMountOutcome, MountOptions, mount_generation};

pub(super) trait RecoveryRuntime {
    fn rebuild(
        &self,
        conn: &rusqlite::Connection,
        generations_root: &Path,
        generation: i64,
        summary: &str,
    ) -> crate::Result<BuildResult> {
        rebuild_generation_image(conn, generations_root, generation, summary)
    }

    fn enable_verity(&self, image: &Path) -> Result<bool, FsVerityError> {
        enable_fsverity(image)
    }

    fn mount(&self, options: &MountOptions) -> crate::Result<GenerationMountOutcome> {
        mount_generation(options)
    }
}

pub(super) struct HostRecoveryRuntime;
impl RecoveryRuntime for HostRecoveryRuntime {}
