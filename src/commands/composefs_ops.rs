// src/commands/composefs_ops.rs

//! Shared composefs-native operations for CLI commands.
//!
//! Every package mutation (install, remove, restore, rollback) ends with
//! the same three-step apply sequence:
//!
//! 1. `build_generation_from_db` -- build EROFS image from current DB state
//! 2. `mount_generation` -- mount it via composefs with `/etc` overlay
//! 3. `update_current_symlink` -- point `/conary/current` at the new generation

use std::path::Path;

use rusqlite::Connection;
use tracing::info;

/// Rebuild the EROFS generation from current DB state and mount it.
///
/// This is the composefs-native "apply" step that follows every DB mutation
/// (install, remove, restore, rollback).  It:
///
/// 1. Builds a new EROFS image from all installed packages in the DB
/// 2. Mounts it via composefs with `/etc` overlay
/// 3. Updates the `/conary/current` symlink
///
/// Returns the new generation number on success.
pub fn rebuild_and_mount(conn: &Connection, summary: &str) -> anyhow::Result<i64> {
    let generations_dir = conary_core::generation::metadata::generations_dir();

    let (gen_num, build_result) =
        conary_core::generation::builder::build_generation_from_db(conn, &generations_dir, summary)
            .map_err(|e| anyhow::anyhow!("Failed to build EROFS generation: {e}"))?;

    info!(
        "Built generation {gen_num} ({} bytes, {} CAS objects)",
        build_result.image_size, build_result.cas_objects_referenced
    );

    let conary_root = Path::new("/conary");
    conary_core::generation::mount::mount_generation(
        &conary_core::generation::mount::MountOptions {
            image_path: build_result.image_path,
            basedir: conary_root.join("objects"),
            mount_point: conary_root.join("mnt"),
            verity: false,
            digest: None,
            upperdir: Some(conary_root.join("etc-state/upper")),
            workdir: Some(conary_root.join("etc-state/work")),
        },
    )
    .map_err(|e| anyhow::anyhow!("Failed to mount generation {gen_num}: {e}"))?;

    conary_core::generation::mount::update_current_symlink(conary_root, gen_num)
        .map_err(|e| anyhow::anyhow!("Failed to update current symlink: {e}"))?;

    info!("Generation {gen_num} mounted and active");
    Ok(gen_num)
}
