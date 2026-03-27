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
use tracing::{debug, info, warn};

use conary_core::db::models::FileEntry;
use conary_core::generation::etc_merge::{self, MergeAction};

/// Collect a `HashMap<relative_path, sha256_hash>` for all /etc files in the DB.
///
/// Paths are stored as absolute in the database (`/etc/foo`); we strip the
/// leading `/` to produce relative keys (`etc/foo`) matching the overlay
/// upper directory layout.
pub(crate) fn collect_etc_files(conn: &Connection) -> anyhow::Result<HashMap<String, String>> {
    let files = FileEntry::find_by_path_pattern(conn, "/etc/%")
        .map_err(|e| anyhow::anyhow!("Failed to query /etc files: {e}"))?;

    let mut map = HashMap::with_capacity(files.len());
    for f in files {
        let rel = f.path.strip_prefix('/').unwrap_or(&f.path).to_string();
        map.insert(rel, f.sha256_hash);
    }
    Ok(map)
}

/// Collect /etc files from a specific generation's state snapshot.
///
/// Joins `state_members` -> `troves` -> `files` to find the /etc files
/// that were part of the given generation. Returns empty map if the
/// generation's troves have been deleted (upgrade cascade).
fn collect_etc_files_for_state(
    conn: &Connection,
    state_number: i64,
) -> anyhow::Result<HashMap<String, String>> {
    // Join on (name, version, architecture) to avoid cross-product in
    // multilib states where multiple troves share the same name+version
    // but differ by architecture.
    let mut stmt = conn.prepare(
        "SELECT DISTINCT f.path, f.sha256_hash FROM files f \
         JOIN troves t ON f.trove_id = t.id \
         JOIN state_members sm ON sm.trove_name = t.name \
             AND sm.trove_version = t.version \
             AND (sm.architecture IS NULL OR t.architecture IS NULL \
                  OR sm.architecture = t.architecture) \
         JOIN system_states ss ON sm.state_id = ss.id \
         WHERE ss.state_number = ?1 AND f.path LIKE '/etc/%'"
    ).map_err(|e| anyhow::anyhow!("Failed to prepare state /etc query: {e}"))?;

    let rows = stmt.query_map([state_number], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }).map_err(|e| anyhow::anyhow!("Failed to query state /etc files: {e}"))?;

    let mut map = HashMap::new();
    for row in rows {
        let (path, hash) = row.map_err(|e| anyhow::anyhow!("Row error: {e}"))?;
        let rel = path.strip_prefix('/').unwrap_or(&path).to_string();
        map.insert(rel, hash);
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
/// `prev_etc_snapshot` must be captured **before** the mutating DB transaction
/// so the three-way merge can distinguish pre- from post-transaction state.
/// Pass `Some(map)` when the caller captured it ahead of the transaction (install,
/// remove).  Pass `None` for callers that do not perform a prior mutation (restore,
/// rollback, `system init`) -- the snapshot will be read from the current DB state.
///
/// `conary_root` is the Conary data directory (typically `/conary`). Callers
/// must pass this explicitly rather than relying on a hardcoded default so
/// that alternate roots (tests, alternate layouts) work correctly.
///
/// Returns the new generation number on success.
pub fn rebuild_and_mount(
    conn: &Connection,
    summary: &str,
    prev_etc_snapshot: Option<HashMap<String, String>>,
    conary_root: &Path,
) -> anyhow::Result<i64> {

    // Record the currently active generation before building the new one.
    // This is written to etc-state/{N}/.base-gen so that the merge logic
    // can identify the correct reference point even after rollback.
    let current_gen = conary_core::generation::mount::current_generation(conary_root)
        .unwrap_or(None)
        .unwrap_or(0);

    // Step 1: Determine the correct "previous" /etc state.
    // - If a pre-captured snapshot was provided (install/remove callers), use it.
    // - Otherwise, check if the currently active generation has a .base-gen marker.
    //   If so, that tells us which generation was active when the current upper dir
    //   was created -- we should use THAT generation's DB state as the base.
    //   This handles the rollback case correctly.
    // - Final fallback: read from current DB state (for init/first-run).
    let prev_etc = match prev_etc_snapshot {
        Some(snapshot) => snapshot,
        None => {
            // Try to read .base-gen from the current generation's upper dir
            let active_upper = conary_root.join(format!("etc-state/{current_gen}"));
            let base_gen_file = active_upper.join(".base-gen");
            if let Ok(content) = std::fs::read_to_string(&base_gen_file)
                && let Ok(base_num) = content.trim().parse::<i64>()
                && base_num > 0
            {
                debug!(
                    "Using base generation {} from .base-gen for /etc merge",
                    base_num
                );
                // Query /etc files from the base generation's state snapshot.
                // This is the correct "previous" state for the three-way merge,
                // even after a rollback where the current DB may not match.
                let base_etc = collect_etc_files_for_state(conn, base_num)?;
                if base_etc.is_empty() {
                    // Distinguish "no /etc files" (legitimate) from "troves
                    // deleted" (cascade, need fallback). Check if the state's
                    // members can still be resolved to trove rows.
                    // Match architecture to avoid false positives from
                    // multilib: if base had foo.x86_64 but only foo.i686
                    // survives, the base troves are effectively deleted.
                    let has_resolvable_troves: bool = conn
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM state_members sm \
                             JOIN troves t ON t.name = sm.trove_name \
                                 AND t.version = sm.trove_version \
                                 AND (sm.architecture IS NULL \
                                      OR t.architecture IS NULL \
                                      OR sm.architecture = t.architecture) \
                             JOIN system_states ss ON sm.state_id = ss.id \
                             WHERE ss.state_number = ?1)",
                            [base_num],
                            |row| row.get(0),
                        )
                        .unwrap_or(false);

                    if has_resolvable_troves {
                        // State exists and has troves, they just have no /etc
                        // files. An empty map IS the correct base.
                        debug!("Base generation {} has no /etc files (correct)", base_num);
                        base_etc
                    } else {
                        // Troves were cascade-deleted. Fall back to current DB.
                        debug!("Base generation {} troves deleted, falling back to current DB", base_num);
                        collect_etc_files(conn)?
                    }
                } else {
                    base_etc
                }
            } else {
                collect_etc_files(conn)?
            }
        }
    };

    // Step 2: Build the new generation (creates state snapshot + EROFS image).
    let generations_dir = conary_core::generation::metadata::generations_dir();
    let (gen_num, build_result) =
        conary_core::generation::builder::build_generation_from_db(conn, &generations_dir, summary)
            .map_err(|e| anyhow::anyhow!("Failed to build EROFS generation: {e}"))?;

    // Per-generation /etc overlay directories -- isolate user modifications so
    // they do not bleed across generation switches.
    let upper_dir = conary_root.join(format!("etc-state/{gen_num}"));
    std::fs::create_dir_all(&upper_dir)?;

    // Record which generation was active when this upper directory was created.
    // The merge logic reads this to find the correct base after rollback,
    // instead of assuming the numerically previous generation is always correct.
    std::fs::write(upper_dir.join(".base-gen"), current_gen.to_string())?;

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
    let etc_work = conary_root.join(format!("etc-state/{gen_num}-work"));
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
