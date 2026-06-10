// apps/conary/src/commands/remove/command.rs

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use conary_core::db::models::Changeset;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use tracing::info;

use super::execution_path::{RemoveExecutionPath, remove_execution_path};
use super::scriptlets::run_post_remove_scriptlet;
use super::transaction::{commit_remove_db, prepare_remove, remove_inner};
use super::types::{RemoveInnerResult, RemoveScriptletOptions};
use crate::commands::progress::{RemovePhase, RemoveProgress};
use crate::commands::{
    InstalledPackageSelector, LegacyReplayOptions, SandboxMode, open_db, resolve_installed_package,
};

/// Remove an installed package
#[allow(clippy::too_many_arguments)]
pub async fn cmd_remove(
    package_name: &str,
    db_path: &str,
    root: &str,
    version: Option<String>,
    architecture: Option<String>,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    purge_files: bool,
    legacy_replay: LegacyReplayOptions,
) -> Result<()> {
    info!("Removing package: {}", package_name);
    println!("Removing package: {}", package_name);
    std::io::stdout().flush()?;
    if let Ok(delay_ms) = std::env::var("CONARY_TEST_HOLD_DURING_REMOVE_MS")
        && let Ok(delay_ms) = delay_ms.parse::<u64>()
        && delay_ms > 0
    {
        std::thread::sleep(Duration::from_millis(delay_ms));
    }

    // Create progress tracker for removal
    let progress = RemoveProgress::new(package_name);

    let conn = open_db(db_path)?;
    let selector =
        InstalledPackageSelector::new(package_name.to_string(), version.clone(), architecture);
    let resolved = resolve_installed_package(&conn, &selector)
        .with_context(|| format!("Failed to select package '{}'", package_name))?;
    let trove = resolved.trove;

    // Check if package is pinned
    if trove.pinned {
        return Err(anyhow::anyhow!(
            "Package '{}' is pinned and cannot be removed. Use 'conary unpin {}' first.",
            package_name,
            package_name
        ));
    }

    if crate::commands::install::is_package_blocked(&trove.name) {
        anyhow::bail!(
            "Refusing to remove critical package '{}'. Use the native package manager for this system package.",
            trove.name
        );
    }

    if trove.install_source.is_adopted() && !purge_files {
        let pkg_mgr = conary_core::packages::SystemPackageManager::detect();
        anyhow::bail!(
            "Refusing to remove adopted package '{}': native package manager authority is preserved. \
             Use '{}' to uninstall it, 'conary system unadopt {}' to remove Conary tracking only, \
             or rerun with --purge-files only if deleting native-owned files is intentional.",
            package_name,
            pkg_mgr.remove_command(package_name),
            package_name
        );
    }

    // Check dependency breakage BEFORE any removal (including adopted packages)
    let breaking = conary_core::resolver::solve_removal(&conn, &[package_name.to_string()])?;

    if !breaking.is_empty() {
        println!(
            "WARNING: Removing '{}' would break the following packages:",
            package_name
        );
        for pkg in &breaking {
            println!("  {}", pkg);
        }
        println!("\nRefusing to remove package with dependencies.");
        println!(
            "Use 'conary query whatbreaks {}' for more information.",
            package_name
        );
        return Err(anyhow::anyhow!(
            "Cannot remove '{}': {} packages depend on it",
            package_name,
            breaking.len()
        ));
    }

    let mut engine = TransactionEngine::new(TransactionConfig::from_paths(
        PathBuf::from(root),
        db_path.into(),
    ))?;
    engine.begin()?;
    let scriptlet_options = RemoveScriptletOptions::new(no_scripts, sandbox_mode, legacy_replay);

    if trove.install_source.is_adopted() && purge_files {
        println!(
            "WARNING: --purge-files specified for adopted package '{}'. \
             Files will be deleted from disk.",
            package_name
        );
    }

    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    if remove_execution_path(db_path)? == RemoveExecutionPath::MutableLiveRoot {
        let result = (|| -> Result<(RemoveInnerResult, crate::commands::LiveRootStats)> {
            crate::commands::live_root::recover_pending_journals_with_changesets(
                runtime_root.root(),
                Path::new(root),
                &conn,
            )?;

            let tx_uuid = uuid::Uuid::new_v4().to_string();
            let tx_description = format!("Remove {}-{}", trove.name, trove.version);
            let prepared = prepare_remove(&conn, &trove, root, scriptlet_options, &progress)?;
            let remove_paths = prepared
                .snapshot
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>();
            let mut live_tx = crate::commands::LiveRootTransaction::begin(
                runtime_root.root(),
                Path::new(root),
                tx_uuid.clone(),
                format!("Remove {}", package_name),
            )?;
            progress.set_phase(RemovePhase::RemovingFiles);
            let stats = live_tx.apply_remove_paths(&remove_paths)?;

            progress.set_phase(RemovePhase::UpdatingDb);
            let tx = conn.unchecked_transaction()?;
            let mut changeset = Changeset::with_tx_uuid(tx_description, tx_uuid.clone());
            let remove_changeset_id = changeset.insert(&tx)?;
            let remove_result = match commit_remove_db(&tx, remove_changeset_id, prepared) {
                Ok(result) => result,
                Err(error) => {
                    live_tx.rollback()?;
                    return Err(error);
                }
            };
            let snapshot_json = crate::commands::metadata_with_removed_troves(vec![
                remove_result.snapshot.clone(),
            ])?;
            tx.execute(
                "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
                rusqlite::params![snapshot_json, remove_changeset_id],
            )?;
            changeset.update_status(&tx, conary_core::db::models::ChangesetStatus::Applied)?;
            if let Err(error) = tx.commit() {
                if let Err(rollback_error) = live_tx.rollback() {
                    return Err(error)
                        .context(format!("Failed to rollback live root: {rollback_error}"));
                }
                return Err(error.into());
            }
            live_tx.commit()?;
            Ok((remove_result, stats))
        })();
        engine.release_lock();
        let (remove_result, stats) = result?;

        run_post_remove_scriptlet(
            &conn,
            &remove_result,
            root,
            no_scripts,
            sandbox_mode,
            &progress,
        )?;
        progress.finish(&format!(
            "Removed {} {}",
            remove_result.trove.name, remove_result.trove.version
        ));
        print_remove_summary(&remove_result, &stats);
        return Ok(());
    }

    // DB-first approach: commit the DB transaction before removing files from disk.
    // If a crash occurs after the DB commit but before file removal completes, the
    // package is already correctly marked as removed. Leftover files on disk are
    // harmless orphans rather than a broken state where files are gone but the
    // package is still recorded as installed.
    // Capture /etc snapshot BEFORE the DB transaction so the three-way merge
    // can distinguish pre- from post-removal state.
    let prev_etc = crate::commands::composefs_ops::collect_etc_files(&conn)?;

    progress.set_phase(RemovePhase::UpdatingDb);
    let mut changeset = Changeset::new(format!("Remove {}-{}", trove.name, trove.version));
    let tx = conn.unchecked_transaction()?;
    let remove_changeset_id = changeset.insert(&tx)?;

    let remove_result = match remove_inner(
        &tx,
        remove_changeset_id,
        &trove,
        root,
        scriptlet_options,
        &progress,
    ) {
        Ok(result) => result,
        Err(e) => {
            engine.release_lock();
            return Err(e);
        }
    };
    let snapshot_json =
        crate::commands::metadata_with_removed_troves(vec![remove_result.snapshot.clone()])?;
    tx.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![snapshot_json, remove_changeset_id],
    )?;
    changeset.update_status(&tx, conary_core::db::models::ChangesetStatus::Applied)?;
    tx.commit()?;

    // Composefs-native: rebuild EROFS image and remount to reflect removal
    progress.set_phase(RemovePhase::RemovingFiles);
    let post_commit_result = (|| -> Result<()> {
        let summary = format!("Remove {}", package_name);
        let outcome = crate::commands::generation::publication::publish_current_db_state(
            &conn,
            crate::commands::generation::publication::PublicationRequest {
                db_path,
                summary: &summary,
                trigger_changeset_id: Some(remove_changeset_id),
                tx_uuid: changeset.tx_uuid.as_deref(),
                prev_etc_snapshot: Some(prev_etc),
            },
        )?;
        if outcome.needs_publication {
            crate::commands::append_deferred_follow_up_metadata(
                &conn,
                remove_changeset_id,
                crate::commands::publication_deferred_follow_up(
                    "generation publication is pending".to_string(),
                ),
            )?;
            crate::commands::generation::publication::warn_if_publication_pending(
                remove_changeset_id,
                &outcome,
            );
        }
        Ok(())
    })();
    engine.release_lock();
    post_commit_result?;

    run_post_remove_scriptlet(
        &conn,
        &remove_result,
        root,
        no_scripts,
        sandbox_mode,
        &progress,
    )?;

    progress.finish(&format!(
        "Removed {} {}",
        remove_result.trove.name, remove_result.trove.version
    ));

    let stats = crate::commands::LiveRootStats {
        files_removed: remove_result.removed_count,
        dirs_removed: remove_result.dirs_removed,
        ..Default::default()
    };
    print_remove_summary(&remove_result, &stats);
    // Note: composefs-native removal rebuilds the entire EROFS image,
    // so individual file failure tracking is not applicable.

    Ok(())
}

fn print_remove_summary(remove_result: &RemoveInnerResult, stats: &crate::commands::LiveRootStats) {
    println!(
        "Removed package: {} version {}",
        remove_result.trove.name, remove_result.trove.version
    );
    println!(
        "  Architecture: {}",
        remove_result
            .trove
            .architecture
            .as_deref()
            .unwrap_or("none")
    );
    println!("  Files removed: {}", stats.files_removed);
    if stats.dirs_removed > 0 {
        println!("  Directories removed: {}", stats.dirs_removed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn no_generation_remove_deletes_files_and_db_rows() {
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_clear_guard();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();

        let payload = root.join("usr/bin/fixture");
        std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
        std::fs::write(&payload, "fixture").unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "fixture".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = conary_core::db::models::FileEntry::new(
            "/usr/bin/fixture".to_string(),
            "0".repeat(64),
            "fixture".len() as i64,
            0o100755,
            trove_id,
        );
        file.insert(&conn).unwrap();
        drop(conn);

        cmd_remove(
            "fixture",
            db_path.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
            None,
            None,
            true,
            SandboxMode::None,
            false,
            LegacyReplayOptions::default(),
        )
        .await
        .unwrap();

        assert!(!payload.exists());
        let conn = conary_core::db::open(&db_path).unwrap();
        assert!(
            conary_core::db::models::Trove::find_by_name(&conn, "fixture")
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn no_generation_remove_fails_closed_on_dangling_current_without_mutation() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        std::os::unix::fs::symlink("generations/7", root.join("current")).unwrap();

        let payload = root.join("usr/bin/fixture");
        std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
        std::fs::write(&payload, "fixture").unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "fixture".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = conary_core::db::models::FileEntry::new(
            "/usr/bin/fixture".to_string(),
            "0".repeat(64),
            "fixture".len() as i64,
            0o100755,
            trove_id,
        );
        file.insert(&conn).unwrap();
        drop(conn);

        let err = cmd_remove(
            "fixture",
            db_path.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
            None,
            None,
            true,
            SandboxMode::None,
            false,
            LegacyReplayOptions::default(),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("dangling"), "{err}");
        assert_eq!(std::fs::read_to_string(&payload).unwrap(), "fixture");
        let conn = conary_core::db::open(&db_path).unwrap();
        assert_eq!(
            conary_core::db::models::Trove::find_by_name(&conn, "fixture")
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn no_generation_remove_live_root_failure_leaves_no_pending_changeset() {
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_clear_guard();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "fixture".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = conary_core::db::models::FileEntry::new(
            "../escape".to_string(),
            "0".repeat(64),
            7,
            0o100755,
            trove_id,
        );
        file.insert(&conn).unwrap();
        drop(conn);

        let err = cmd_remove(
            "fixture",
            db_path.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
            None,
            None,
            true,
            SandboxMode::None,
            false,
            LegacyReplayOptions::default(),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("escapes the target root"), "{err}");
        let conn = conary_core::db::open(&db_path).unwrap();
        let changesets: i64 = conn
            .query_row("SELECT COUNT(*) FROM changesets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(changesets, 0);
        assert_eq!(
            conary_core::db::models::Trove::find_by_name(&conn, "fixture")
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn remove_refuses_critical_package_before_file_mutation() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();

        let payload = root.join("usr/bin/bash");
        std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
        std::fs::write(&payload, "bash").unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "bash".to_string(),
            "5.2".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = conary_core::db::models::FileEntry::new(
            "/usr/bin/bash".to_string(),
            "0".repeat(64),
            "bash".len() as i64,
            0o100755,
            trove_id,
        );
        file.insert(&conn).unwrap();
        drop(conn);

        let err = cmd_remove(
            "bash",
            db_path.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
            None,
            None,
            true,
            SandboxMode::None,
            false,
            LegacyReplayOptions::default(),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("critical package"));
        assert_eq!(std::fs::read_to_string(&payload).unwrap(), "bash");
    }
}
