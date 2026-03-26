// src/commands/composefs_ops.rs

//! Shared composefs-native operations for CLI commands.
//!
//! Every package mutation (install, remove, restore, rollback) ends with
//! the same four-step apply sequence:
//!
//! 1. `build_generation_from_db` -- build EROFS image from current DB state
//! 2. Three-way `/etc` merge -- compare prev generation, new generation, and
//!    user overlay; resolve non-conflicts and warn on real conflicts
//! 3. `mount_generation` -- mount it via composefs with `/etc` overlay
//! 4. `update_current_symlink` -- point `/conary/current` at the new generation

use std::collections::HashMap;
use std::path::Path;

use rusqlite::Connection;
use tracing::{info, warn};

use conary_core::db::models::FileEntry;
use conary_core::generation::etc_merge::{self, MergeAction};

/// Collect a `HashMap<relative_path, sha256_hash>` for all /etc files in the DB.
///
/// Paths are stored as absolute in the database (`/etc/foo`); we strip the
/// leading `/` to produce relative keys (`etc/foo`) matching the overlay
/// upper directory layout.
fn collect_etc_files(conn: &Connection) -> anyhow::Result<HashMap<String, String>> {
    let files = FileEntry::find_by_path_pattern(conn, "/etc/%")
        .map_err(|e| anyhow::anyhow!("Failed to query /etc files: {e}"))?;

    let mut map = HashMap::with_capacity(files.len());
    for f in files {
        let rel = f.path.strip_prefix('/').unwrap_or(&f.path).to_string();
        map.insert(rel, f.sha256_hash);
    }
    Ok(map)
}

/// Rebuild the EROFS generation from current DB state and mount it.
///
/// This is the composefs-native "apply" step that follows every DB mutation
/// (install, remove, restore, rollback).  It:
///
/// 1. Snapshots the previous generation's /etc file hashes from the DB
/// 2. Builds a new EROFS image from all installed packages in the DB
/// 3. Runs a three-way /etc merge (prev base vs new package vs user overlay)
/// 4. For `AcceptPackage` actions, removes the upper layer copy so the new
///    EROFS lower shows through
/// 5. Warns on conflicts (user must resolve manually)
/// 6. Mounts it via composefs with `/etc` overlay
/// 7. Updates the `/conary/current` symlink
///
/// Returns the new generation number on success.
pub fn rebuild_and_mount(conn: &Connection, summary: &str) -> anyhow::Result<i64> {
    let conary_root = Path::new("/conary");
    let upper_dir = conary_root.join("etc-state/upper");

    // Step 1: Snapshot the previous generation's /etc files from DB *before*
    // the build mutates state.
    let prev_etc = collect_etc_files(conn)?;

    // Step 2: Build the new generation (creates state snapshot + EROFS image).
    let generations_dir = conary_core::generation::metadata::generations_dir();
    let (gen_num, build_result) =
        conary_core::generation::builder::build_generation_from_db(conn, &generations_dir, summary)
            .map_err(|e| anyhow::anyhow!("Failed to build EROFS generation: {e}"))?;

    info!(
        "Built generation {gen_num} ({} bytes, {} CAS objects)",
        build_result.image_size, build_result.cas_objects_referenced
    );

    // Step 3: Collect new generation's /etc files (DB was just updated by
    // the build/snapshot process, so this reflects the new package set).
    let new_etc = collect_etc_files(conn)?;

    // Step 4: Run three-way /etc merge.
    let merge_plan = etc_merge::plan_etc_merge(&prev_etc, &new_etc, &upper_dir)
        .map_err(|e| anyhow::anyhow!("Failed to plan /etc merge: {e}"))?;

    // Step 5: Apply non-conflict actions.
    for (rel_path, action) in &merge_plan.actions {
        match action {
            MergeAction::AcceptPackage => {
                // Remove the upper layer copy so the new EROFS lower version
                // shows through the overlay.
                let upper_file = upper_dir.join(rel_path);
                if upper_file.exists() {
                    if let Err(e) = std::fs::remove_file(&upper_file) {
                        warn!(
                            path = %rel_path.display(),
                            error = %e,
                            "Failed to remove upper layer file for package update"
                        );
                    } else {
                        info!(
                            path = %rel_path.display(),
                            "Accepted package update (removed upper layer copy)"
                        );
                    }
                }
            }
            MergeAction::Conflict {
                base_hash,
                package_hash,
                user_hash,
            } => {
                warn!(
                    path = %rel_path.display(),
                    base = %base_hash,
                    package = %package_hash,
                    user = %user_hash,
                    "Merge conflict: both package and user modified this /etc file"
                );
            }
            MergeAction::OrphanedUserFile => {
                warn!(
                    path = %rel_path.display(),
                    "Package removed this /etc file but user had modifications; \
                     user copy preserved in overlay"
                );
            }
            MergeAction::KeepUser => {
                info!(
                    path = %rel_path.display(),
                    "Keeping user-modified /etc file"
                );
            }
            MergeAction::NewFromPackage => {
                info!(
                    path = %rel_path.display(),
                    "New /etc file from package"
                );
            }
            MergeAction::Unchanged => {}
        }
    }

    if merge_plan.has_conflicts() {
        let conflict_count = merge_plan.conflicts().len();
        warn!(
            count = conflict_count,
            "Generation {gen_num} has /etc merge conflicts that need manual resolution"
        );
    }

    // Step 6: Mount the new generation at the staging point.
    let staging_mount = conary_root.join("mnt");
    conary_core::generation::mount::mount_generation(
        &conary_core::generation::mount::MountOptions {
            image_path: build_result.image_path,
            basedir: conary_root.join("objects"),
            mount_point: staging_mount.clone(),
            verity: false,
            digest: None,
            upperdir: None,
            workdir: None,
        },
    )
    .map_err(|e| anyhow::anyhow!("Failed to mount generation {gen_num}: {e}"))?;

    // Step 7: Set up /etc overlay -- lower from staging, target at live /etc.
    let etc_work = conary_root.join("etc-state/work");
    if let Err(e) = conary_core::generation::mount::mount_etc_overlay(
        &staging_mount.join("etc"),
        Path::new("/etc"),
        &upper_dir,
        &etc_work,
    ) {
        warn!("Failed to mount /etc overlay: {e}; /etc may be stale");
    }

    conary_core::generation::mount::update_current_symlink(conary_root, gen_num)
        .map_err(|e| anyhow::anyhow!("Failed to update current symlink: {e}"))?;

    info!("Generation {gen_num} mounted and active");
    Ok(gen_num)
}
