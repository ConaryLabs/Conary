// src/commands/composefs_ops.rs

//! Shared composefs-native operations for CLI commands.
//!
//! Every package mutation (install, remove, restore, rollback) ends with
//! the same atomic generation publication sequence:
//!
//! 1. `build_generation_from_db` -- build EROFS image from current DB state
//! 2. Three-way `/etc` merge -- compare prev generation, new generation, and
//!    generation-local user overlay; resolve non-conflicts and warn on real conflicts
//! 3. `enable_generation_rootfs_verity` -- make runtime metadata truthful when
//!    the backing filesystem supports fs-verity
//! 4. `update_current_symlink` -- point `/conary/current` at the next-boot generation

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::Connection;
use tracing::{debug, info, warn};

use crate::commands::generation::builder::enable_generation_rootfs_verity;
use conary_core::db::models::FileEntry;
use conary_core::generation::etc_merge::{self, MergeAction};
use conary_core::runtime_root::ConaryRuntimeRoot;

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
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT f.path, f.sha256_hash FROM files f \
         JOIN troves t ON f.trove_id = t.id \
         JOIN state_members sm ON sm.trove_name = t.name \
             AND sm.trove_version = t.version \
             AND (sm.architecture IS NULL OR t.architecture IS NULL \
                  OR sm.architecture = t.architecture) \
         JOIN system_states ss ON sm.state_id = ss.id \
         WHERE ss.state_number = ?1 AND f.path LIKE '/etc/%'",
        )
        .map_err(|e| anyhow::anyhow!("Failed to prepare state /etc query: {e}"))?;

    let rows = stmt
        .query_map([state_number], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| anyhow::anyhow!("Failed to query state /etc files: {e}"))?;

    let mut map = HashMap::new();
    for row in rows {
        let (path, hash) = row.map_err(|e| anyhow::anyhow!("Row error: {e}"))?;
        let rel = path.strip_prefix('/').unwrap_or(&path).to_string();
        map.insert(rel, hash);
    }
    Ok(map)
}

fn current_base_generation_for_merge(
    conn: &Connection,
    current_gen: i64,
) -> anyhow::Result<Option<i64>> {
    if current_gen <= 0 {
        return Ok(None);
    }

    let state = conary_core::db::models::SystemState::find_by_number(conn, current_gen)
        .map_err(|e| anyhow::anyhow!("Failed to load system state {current_gen}: {e}"))?;
    Ok(state.and_then(|state| state.base_generation))
}

/// Rebuild the EROFS generation from current DB state and publish it.
///
/// This is the composefs-native publication step that follows every DB
/// mutation (install, remove, restore, rollback).  It:
///
/// 1. Snapshots the previous generation's /etc file hashes from the DB
/// 2. Builds a new EROFS image from all installed packages in the DB
/// 3. Runs a three-way /etc merge (prev base vs new package vs user overlay)
/// 4. For `AcceptPackage` actions, removes the upper layer copy so the new
///    EROFS lower shows through
/// 5. Warns on conflicts (user must resolve manually)
/// 6. Enables fs-verity for the generation image when not explicitly skipped
/// 7. Updates the `/conary/current` symlink for next boot
///
/// `prev_etc_snapshot` must be captured **before** the mutating DB transaction
/// so the three-way merge can distinguish pre- from post-transaction state.
/// Pass `Some(map)` when the caller captured it ahead of the transaction (install,
/// remove).  Pass `None` for callers that do not perform a prior mutation (restore,
/// rollback, `system init`) -- the snapshot will be read from the current DB state.
///
/// The runtime root is derived through `ConaryRuntimeRoot`, so the default DB
/// at `/var/lib/conary/conary.db` still stores boot-visible generation state
/// under `/conary` while non-default test DB paths stay self-contained.
///
/// Returns the new generation number on success.
fn runtime_root_for_db_path(db_path: &str) -> ConaryRuntimeRoot {
    ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path))
}

fn boot_root_for_generation_build(runtime_root: &ConaryRuntimeRoot) -> PathBuf {
    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
        let test_boot = runtime_root.root().join("boot");
        if test_boot.is_dir() {
            return test_boot;
        }
    }

    PathBuf::from("/boot")
}

pub fn rebuild_and_mount(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    prev_etc_snapshot: Option<HashMap<String, String>>,
) -> anyhow::Result<i64> {
    let runtime_root = runtime_root_for_db_path(db_path);

    // Record the currently active generation before building the new one.
    // The state snapshot for the new generation stores this as its
    // database-backed /etc merge base.
    let current_gen = conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap_or(None)
        .unwrap_or(0);

    // Step 1: Determine the correct "previous" /etc state.
    // - If a pre-captured snapshot was provided (install/remove callers), use it.
    // - Otherwise, check the active generation's recorded base_generation
    //   in system_states. That tells us which generation was active when the
    //   current upper dir was created -- we should use THAT generation's DB
    //   state as the base.
    //   This handles the rollback case correctly.
    // - Final fallback: read from current DB state (for init/first-run).
    let prev_etc = match prev_etc_snapshot {
        Some(snapshot) => snapshot,
        None => {
            if let Some(base_num) = current_base_generation_for_merge(conn, current_gen)? {
                debug!(
                    "Using base generation {} from system_states for /etc merge",
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
                        debug!(
                            "Base generation {} troves deleted, falling back to current DB",
                            base_num
                        );
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
    let generations_dir = runtime_root.generations_dir();
    let boot_root = boot_root_for_generation_build(&runtime_root);
    let (gen_num, build_result) =
        conary_core::generation::builder::build_generation_from_db_with_boot_root(
            conn,
            &generations_dir,
            summary,
            &boot_root,
        )
        .map_err(|e| anyhow::anyhow!("Failed to build EROFS generation: {e}"))?;

    // Per-generation /etc overlay directories -- isolate user modifications so
    // they do not bleed across generation switches.
    let upper_dir = runtime_root.etc_state_dir().join(gen_num.to_string());
    std::fs::create_dir_all(&upper_dir)?;

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

    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
        conary_core::generation::mount::update_current_symlink(runtime_root.root(), gen_num)
            .map_err(|e| anyhow::anyhow!("Failed to update current symlink: {e}"))?;
        info!(
            "Skipping generation fs-verity enablement because CONARY_TEST_SKIP_GENERATION_MOUNT is set"
        );
        return Ok(gen_num);
    }

    let gen_dir = generations_dir.join(gen_num.to_string());
    enable_generation_rootfs_verity(&gen_dir, &build_result.image_path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to enable fs-verity on generation {gen_num} image {}: {e}",
            build_result.image_path.display()
        )
    })?;

    conary_core::generation::mount::update_current_symlink(runtime_root.root(), gen_num)
        .map_err(|e| anyhow::anyhow!("Failed to update current symlink: {e}"))?;

    info!("Generation {gen_num} built and selected for next boot");
    Ok(gen_num)
}

#[cfg(test)]
pub(crate) struct TestMountSkipGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
static TEST_MOUNT_SKIP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn test_mount_skip_guard() -> TestMountSkipGuard {
    let guard = TEST_MOUNT_SKIP_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("CONARY_TEST_SKIP_GENERATION_MOUNT", "1");
    }
    TestMountSkipGuard { _guard: guard }
}

#[cfg(test)]
impl Drop for TestMountSkipGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("CONARY_TEST_SKIP_GENERATION_MOUNT");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::SystemState;
    use std::path::Path;

    #[test]
    fn current_base_generation_for_merge_reads_db_column() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();

        let mut state = SystemState::new(3, "gen3".to_string());
        state.base_generation = Some(2);
        state.insert(&conn).unwrap();

        assert_eq!(
            current_base_generation_for_merge(&conn, 3).unwrap(),
            Some(2)
        );
        assert_eq!(current_base_generation_for_merge(&conn, 99).unwrap(), None);
    }

    #[test]
    fn verity_warning_text_is_backed_by_mount_helper() {
        use conary_core::generation::mount::{GenerationMountOutcome, verity_downgrade_warning};

        let warning = verity_downgrade_warning(
            true,
            GenerationMountOutcome::ComposefsPlain,
            Path::new("/conary/generations/7/root.erofs"),
        )
        .unwrap();
        assert!(warning.contains("downgraded"));
    }

    #[test]
    fn runtime_root_for_default_db_path_uses_boot_visible_runtime_root() {
        let runtime_root = runtime_root_for_db_path("/var/lib/conary/conary.db");

        assert_eq!(runtime_root.root(), Path::new("/conary"));
        assert_eq!(
            runtime_root.db_path(),
            Path::new("/var/lib/conary/conary.db")
        );
        assert_eq!(runtime_root.objects_dir(), PathBuf::from("/conary/objects"));
        assert_eq!(
            runtime_root.generations_dir(),
            PathBuf::from("/conary/generations")
        );
    }

    #[test]
    fn runtime_root_for_test_db_path_stays_self_contained() {
        let runtime_root = runtime_root_for_db_path("/tmp/test-conary/conary.db");

        assert_eq!(runtime_root.root(), Path::new("/tmp/test-conary"));
        assert_eq!(
            runtime_root.generations_dir(),
            PathBuf::from("/tmp/test-conary/generations")
        );
    }

    #[test]
    fn boot_root_for_generation_build_prefers_self_contained_test_boot_when_requested() {
        let _guard = test_mount_skip_guard();
        let temp = tempfile::TempDir::new().unwrap();
        let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::for_test_root(temp.path());
        std::fs::create_dir_all(temp.path().join("boot")).unwrap();

        assert_eq!(
            boot_root_for_generation_build(&runtime_root),
            temp.path().join("boot")
        );
    }
}
