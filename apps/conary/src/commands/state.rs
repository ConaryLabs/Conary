// src/commands/state.rs
//! System state snapshot management commands

use super::install::{
    InstallOptions, add_prepared_install_to_target_state, build_target_state_view,
    finalize_prepared_install_without_snapshot, install_prepared_inner,
    prepare_install_for_restore, run_pre_install_for_prepared,
    validate_prepared_install_dependencies,
};
use super::progress::RemoveProgress;
use super::remove::remove_inner;
use super::{RevertMetadata, SandboxMode, open_db};
use anyhow::Result;
use conary_core::db::models::{Changeset, StateDiff, StateEngine, StateMember, SystemState, Trove};
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::path::PathBuf;
use tracing::info;

/// List all system states
pub async fn cmd_state_list(db_path: &str, limit: Option<i64>) -> Result<()> {
    info!("Listing system states...");

    let conn = open_db(db_path)?;

    let states = if let Some(n) = limit {
        SystemState::list_recent(&conn, n)?
    } else {
        SystemState::list_all(&conn)?
    };

    if states.is_empty() {
        println!("No system states recorded.");
        println!("\nStates are created automatically after install/remove operations.");
        return Ok(());
    }

    println!("System States:");
    println!(
        "{:>6}  {:>8}  {:20}  SUMMARY",
        "STATE", "PACKAGES", "CREATED"
    );
    println!("{}", "-".repeat(70));

    for state in &states {
        let active_marker = if state.is_active { "*" } else { " " };
        let created = state.created_at.as_deref().unwrap_or("unknown");
        // Truncate to date/time portion
        let created_short = if created.len() > 19 {
            &created[..19]
        } else {
            created
        };

        println!(
            "{:>5}{} {:>8}  {:20}  {}",
            state.state_number, active_marker, state.package_count, created_short, state.summary
        );
    }

    println!();
    println!("* = active state");
    println!("Total: {} state(s)", states.len());

    Ok(())
}

/// Show details of a specific state
pub async fn cmd_state_show(db_path: &str, state_number: i64) -> Result<()> {
    info!("Showing state {}...", state_number);

    let conn = open_db(db_path)?;

    let state = SystemState::find_by_number(&conn, state_number)?
        .ok_or_else(|| anyhow::anyhow!("State {} not found", state_number))?;

    println!("State {}", state.state_number);
    println!("{}", "=".repeat(40));
    println!("Summary:     {}", state.summary);
    if let Some(desc) = &state.description {
        println!("Description: {}", desc);
    }
    println!(
        "Created:     {}",
        state.created_at.as_deref().unwrap_or("unknown")
    );
    println!("Packages:    {}", state.package_count);
    println!(
        "Active:      {}",
        if state.is_active { "Yes" } else { "No" }
    );
    if let Some(cs_id) = state.changeset_id {
        println!("Changeset:   {}", cs_id);
    }

    // Show packages in this state
    let members = state.get_members(&conn)?;
    if !members.is_empty() {
        println!("\nPackages ({}):", members.len());
        for member in &members {
            let arch = member.architecture.as_deref().unwrap_or("");
            let reason = member.install_reason.as_str();
            let marker = if reason == "dependency" { " (dep)" } else { "" };
            println!(
                "  {} {} [{}]{}",
                member.trove_name, member.trove_version, arch, marker
            );
        }
    }

    Ok(())
}

/// Show diff between two states
pub async fn cmd_state_diff(db_path: &str, from_state: i64, to_state: i64) -> Result<()> {
    info!("Comparing states {} -> {}...", from_state, to_state);

    let conn = open_db(db_path)?;

    let from = SystemState::find_by_number(&conn, from_state)?
        .ok_or_else(|| anyhow::anyhow!("State {} not found", from_state))?;
    let to = SystemState::find_by_number(&conn, to_state)?
        .ok_or_else(|| anyhow::anyhow!("State {} not found", to_state))?;

    let from_id = from.id.ok_or_else(|| anyhow::anyhow!("State has no ID"))?;
    let to_id = to.id.ok_or_else(|| anyhow::anyhow!("State has no ID"))?;

    let diff = StateDiff::compare(&conn, from_id, to_id)?;

    println!("State Diff: {} -> {}", from_state, to_state);
    println!("{}", "=".repeat(50));

    if diff.is_empty() {
        println!("No differences between states.");
        return Ok(());
    }

    if !diff.added.is_empty() {
        println!("\nAdded ({}):", diff.added.len());
        for member in &diff.added {
            println!("  + {} {}", member.trove_name, member.trove_version);
        }
    }

    if !diff.removed.is_empty() {
        println!("\nRemoved ({}):", diff.removed.len());
        for member in &diff.removed {
            println!("  - {} {}", member.trove_name, member.trove_version);
        }
    }

    if !diff.upgraded.is_empty() {
        println!("\nChanged ({}):", diff.upgraded.len());
        for (old, new) in &diff.upgraded {
            println!(
                "  ~ {} {} -> {}",
                old.trove_name, old.trove_version, new.trove_version
            );
        }
    }

    println!("\nTotal changes: {}", diff.change_count());

    Ok(())
}

/// Restore to a previous state
pub async fn cmd_state_restore(db_path: &str, state_number: i64, dry_run: bool) -> Result<()> {
    execute_restore_plan_with_root(db_path, "/", state_number, dry_run).await
}

async fn execute_restore_plan_with_root(
    db_path: &str,
    root: &str,
    state_number: i64,
    dry_run: bool,
) -> Result<()> {
    info!("Restoring to state {}...", state_number);

    let conn = open_db(db_path)?;

    let target = SystemState::find_by_number(&conn, state_number)?
        .ok_or_else(|| anyhow::anyhow!("State {} not found", state_number))?;

    let target_id = target
        .id
        .ok_or_else(|| anyhow::anyhow!("State has no ID"))?;

    let engine = StateEngine::new(&conn);
    let plan = engine.plan_restore(target_id)?;

    if plan.is_empty() {
        println!("System is already at state {}.", state_number);
        return Ok(());
    }

    println!(
        "Restore Plan: State {} -> State {}",
        plan.from_state.state_number, plan.to_state.state_number
    );
    println!("{}", "=".repeat(50));

    if !plan.to_remove.is_empty() {
        println!("\nPackages to remove ({}):", plan.to_remove.len());
        for member in &plan.to_remove {
            println!("  - {} {}", member.trove_name, member.trove_version);
        }
    }

    if !plan.to_install.is_empty() {
        println!("\nPackages to install ({}):", plan.to_install.len());
        for member in &plan.to_install {
            println!("  + {} {}", member.trove_name, member.trove_version);
        }
    }

    if !plan.to_upgrade.is_empty() {
        println!("\nPackages to change ({}):", plan.to_upgrade.len());
        for (old, new) in &plan.to_upgrade {
            println!(
                "  ~ {} {} -> {}",
                old.trove_name, old.trove_version, new.trove_version
            );
        }
    }

    println!("\nTotal operations: {}", plan.operation_count());

    if dry_run {
        println!("\nDry run - no changes made.");
        println!("Run without --dry-run to apply these changes.");
        return Ok(());
    }

    let target_members = target.get_members(&conn)?;
    let mut target_state = build_target_state_view(&conn, &target_members)?;
    let mut prepared_installs = Vec::with_capacity(plan.to_install.len() + plan.to_upgrade.len());
    for member in &plan.to_install {
        let prepared = prepare_install_for_restore(
            &conn,
            &member.trove_name,
            InstallOptions {
                db_path,
                root,
                version: Some(member.trove_version.clone()),
                architecture: member.architecture.clone(),
                no_scripts: false,
                sandbox_mode: SandboxMode::Always,
                allow_downgrade: true,
                selection_reason: Some("Restored by state revert"),
                yes: true,
                ..InstallOptions::default()
            },
        )
        .await?;
        add_prepared_install_to_target_state(&mut target_state, &prepared);
        prepared_installs.push(prepared);
    }
    for (_, member) in &plan.to_upgrade {
        let prepared = prepare_install_for_restore(
            &conn,
            &member.trove_name,
            InstallOptions {
                db_path,
                root,
                version: Some(member.trove_version.clone()),
                architecture: member.architecture.clone(),
                no_scripts: false,
                sandbox_mode: SandboxMode::Always,
                allow_downgrade: true,
                selection_reason: Some("Restored by state revert"),
                yes: true,
                ..InstallOptions::default()
            },
        )
        .await?;
        add_prepared_install_to_target_state(&mut target_state, &prepared);
        prepared_installs.push(prepared);
    }
    for prepared in &prepared_installs {
        validate_prepared_install_dependencies(prepared, &target_state)?;
    }

    let prev_etc = crate::commands::composefs_ops::collect_etc_files(&conn)?;
    drop(conn);

    let conn = open_db(db_path)?;
    let tx_config = TransactionConfig::from_paths(PathBuf::from(root), PathBuf::from(db_path));
    let mut engine = TransactionEngine::new(tx_config)
        .map_err(|e| anyhow::anyhow!("Failed to create transaction engine: {e}"))?;
    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_none() {
        engine
            .recover(&conn)
            .map_err(|e| anyhow::anyhow!("Failed to recover incomplete transactions: {e}"))?;
    } else {
        info!(
            "Skipping transaction recovery mount because CONARY_TEST_SKIP_GENERATION_MOUNT is set"
        );
    }
    engine
        .begin()
        .map_err(|e| anyhow::anyhow!("Failed to begin transaction: {e}"))?;

    let mut prepared_executions = Vec::with_capacity(prepared_installs.len());
    for prepared in prepared_installs {
        let execution =
            match run_pre_install_for_prepared(&conn, root, false, SandboxMode::Always, prepared) {
                Ok(execution) => execution,
                Err(err) => {
                    engine.release_lock();
                    return Err(err);
                }
            };
        prepared_executions.push(execution);
    }

    let mut changeset = Changeset::new(format!(
        "Restore state {} -> {}",
        plan.from_state.state_number, plan.to_state.state_number
    ));

    let restore_tx_result = (|| -> Result<i64> {
        let tx = conn.unchecked_transaction()?;
        let changeset_id = changeset.insert(&tx)?;

        let mut removed_troves = Vec::with_capacity(plan.to_remove.len());
        for member in &plan.to_remove {
            let trove = find_installed_trove_for_member(&tx, member)?;
            let progress = RemoveProgress::new(&trove.name);
            let remove_result = remove_inner(
                &tx,
                changeset_id,
                &trove,
                root,
                false,
                SandboxMode::Always,
                &progress,
            )?;
            removed_troves.push(remove_result.snapshot);
        }
        for execution in &prepared_executions {
            install_prepared_inner(&tx, &mut engine, changeset_id, db_path, execution)?;
        }

        let metadata_json = serde_json::to_string(&RevertMetadata { removed_troves })?;
        tx.execute(
            "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
            rusqlite::params![metadata_json, changeset_id],
        )?;
        tx.commit()?;
        Ok(changeset_id)
    })();

    let changeset_id = match restore_tx_result {
        Ok(changeset_id) => changeset_id,
        Err(err) => {
            engine.release_lock();
            return Err(err);
        }
    };

    let post_commit_result = (|| -> Result<()> {
        crate::commands::composefs_ops::rebuild_and_mount(
            &conn,
            db_path,
            &format!("Restore state {}", state_number),
            Some(prev_etc),
        )?;
        changeset.update_status(&conn, conary_core::db::models::ChangesetStatus::Applied)?;
        for execution in &prepared_executions {
            finalize_prepared_install_without_snapshot(&conn, changeset_id, execution)?;
        }
        info!("Restore changeset {} applied", changeset_id);
        Ok(())
    })();

    engine.release_lock();
    post_commit_result
}

fn find_installed_trove_for_member(
    conn: &rusqlite::Connection,
    member: &StateMember,
) -> Result<Trove> {
    Trove::find_by_name(conn, &member.trove_name)?
        .into_iter()
        .find(|trove| state_member_matches_trove(member, trove))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Restore plan expected installed package '{}' version '{}'{}",
                member.trove_name,
                member.trove_version,
                format_member_arch_suffix(member.architecture.as_deref()),
            )
        })
}

fn state_member_matches_trove(member: &StateMember, trove: &Trove) -> bool {
    trove.version == member.trove_version
        && architectures_match(
            member.architecture.as_deref(),
            trove.architecture.as_deref(),
        )
}

fn architectures_match(target: Option<&str>, actual: Option<&str>) -> bool {
    target == actual || target.is_none() || actual.is_none()
}

fn format_member_arch_suffix(architecture: Option<&str>) -> String {
    architecture
        .map(|arch| format!(" [{arch}]"))
        .unwrap_or_default()
}

/// Prune old states, keeping only the most recent N
pub async fn cmd_state_prune(db_path: &str, keep_count: i64, dry_run: bool) -> Result<()> {
    info!("Pruning states, keeping {} most recent...", keep_count);

    if keep_count < 1 {
        return Err(anyhow::anyhow!("Must keep at least 1 state"));
    }

    let conn = open_db(db_path)?;

    let all_states = SystemState::list_all(&conn)?;
    let total_count = all_states.len() as i64;

    if total_count <= keep_count {
        println!("Only {} state(s) exist, nothing to prune.", total_count);
        return Ok(());
    }

    let to_prune = total_count - keep_count;

    // Show states that would be pruned
    let prune_candidates: Vec<_> = all_states
        .iter()
        .rev() // Oldest first
        .take(to_prune as usize)
        .filter(|s| !s.is_active) // Never prune active state
        .collect();

    if prune_candidates.is_empty() {
        println!("No states to prune (active state is protected).");
        return Ok(());
    }

    println!("States to prune ({}):", prune_candidates.len());
    for state in &prune_candidates {
        println!(
            "  State {}: {} ({})",
            state.state_number,
            state.summary,
            state.created_at.as_deref().unwrap_or("unknown")
        );
    }

    if dry_run {
        println!("\nDry run - no states will be deleted.");
        return Ok(());
    }

    let engine = StateEngine::new(&conn);
    let deleted = engine.prune(keep_count)?;

    println!(
        "\nPruned {} state(s). Keeping {} most recent.",
        deleted, keep_count
    );

    Ok(())
}

/// Create a manual state snapshot
pub async fn cmd_state_create(
    db_path: &str,
    summary: &str,
    description: Option<&str>,
) -> Result<()> {
    info!("Creating manual state snapshot...");

    let conn = open_db(db_path)?;

    let engine = StateEngine::new(&conn);
    let state = engine.create_snapshot(summary, description, None)?;

    println!("Created state {}", state.state_number);
    println!("  Summary:  {}", state.summary);
    println!("  Packages: {}", state.package_count);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::execute_restore_plan_with_root;
    use conary_core::db::models::{
        Changeset, ChangesetStatus, PackageResolution, PrimaryStrategy, Repository,
        RepositoryPackage, ResolutionStrategy, Trove, TroveType,
    };
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};

    fn build_test_ccs_package(dir: &Path, name: &str, version: &str) -> PathBuf {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let binary_content = format!("#!/bin/sh\necho {name} {version}\n").into_bytes();
        let binary_hash = hash::sha256(&binary_content);
        let files = vec![FileEntry {
            path: format!("/usr/bin/{name}"),
            hash: binary_hash.clone(),
            size: binary_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        }];
        let package_path = dir.join(format!("{name}-{version}.ccs"));
        let result = BuildResult {
            manifest: CcsManifest::new_minimal(name, version),
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: format!("{name}-runtime"),
                    size: binary_content.len() as u64,
                },
            )]),
            files,
            blobs: HashMap::from([(binary_hash, binary_content)]),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();
        package_path
    }

    fn serve_test_file(file_path: PathBuf) -> (String, std::thread::JoinHandle<()>) {
        let filename = file_path.file_name().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(&file_path).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            stream.write_all(headers.as_bytes()).unwrap();
            stream.write_all(&bytes).unwrap();
        });
        (format!("http://{addr}/{filename}"), handle)
    }

    #[tokio::test]
    async fn test_state_restore_remove_only_executes_and_creates_one_changeset_and_snapshot() {
        let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
        let root = tempfile::tempdir().unwrap();
        let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

        let mut conn = crate::commands::open_db(&db_path).unwrap();
        let engine = conary_core::db::models::StateEngine::new(&conn);
        let baseline = engine.create_snapshot("baseline", None, None).unwrap();

        conary_core::db::transaction(&mut conn, |tx| {
            let mut cs = conary_core::db::models::Changeset::new("Install vim-9.1.0".to_string());
            let cs_id = cs.insert(tx)?;
            let mut vim = conary_core::db::models::Trove::new(
                "vim".to_string(),
                "9.1.0".to_string(),
                conary_core::db::models::TroveType::Package,
            );
            vim.architecture = Some("x86_64".to_string());
            vim.installed_by_changeset_id = Some(cs_id);
            vim.insert(tx)?;
            cs.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
            Ok::<_, conary_core::Error>(())
        })
        .unwrap();

        let _drifted = conary_core::db::models::StateEngine::new(&conn)
            .create_snapshot("drifted", None, None)
            .unwrap();

        let before_changesets: i64 = conn
            .query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get(0))
            .unwrap();
        let before_states: i64 = conn
            .query_row("SELECT COUNT(*) FROM system_states", [], |r| r.get(0))
            .unwrap();
        drop(conn);

        let result = execute_restore_plan_with_root(
            &db_path,
            root.path().to_str().unwrap(),
            baseline.state_number,
            false,
        )
        .await;

        assert!(
            result.is_ok(),
            "remove-only restore should succeed: {:?}",
            result
        );

        let conn = crate::commands::open_db(&db_path).unwrap();
        assert!(
            conary_core::db::models::Trove::find_one_by_name(&conn, "vim")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM changesets", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            before_changesets + 1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM system_states", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            before_states + 1
        );
    }

    #[tokio::test]
    async fn test_state_restore_missing_repo_version_rolls_back_without_snapshot() {
        let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
        let root = tempfile::tempdir().unwrap();
        let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

        let mut conn = crate::commands::open_db(&db_path).unwrap();
        let engine = conary_core::db::models::StateEngine::new(&conn);
        let baseline = engine.create_snapshot("baseline", None, None).unwrap();

        conary_core::db::transaction(&mut conn, |tx| {
            let mut cs = conary_core::db::models::Changeset::new("Install vim-9.1.0".to_string());
            let cs_id = cs.insert(tx)?;
            let mut vim = conary_core::db::models::Trove::new(
                "vim".to_string(),
                "9.1.0".to_string(),
                conary_core::db::models::TroveType::Package,
            );
            vim.architecture = Some("x86_64".to_string());
            vim.installed_by_changeset_id = Some(cs_id);
            vim.insert(tx)?;
            cs.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
            Ok::<_, conary_core::Error>(())
        })
        .unwrap();

        let drifted = conary_core::db::models::StateEngine::new(&conn)
            .create_snapshot("drifted", None, None)
            .unwrap();
        assert!(drifted.state_number > baseline.state_number);

        conn.execute(
            "UPDATE state_members SET trove_version = '9.9.9' WHERE state_id = ?1 AND trove_name = 'nginx'",
            [baseline.id.unwrap()],
        )
        .unwrap();

        let before_changesets: i64 = conn
            .query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get(0))
            .unwrap();
        let before_states: i64 = conn
            .query_row("SELECT COUNT(*) FROM system_states", [], |r| r.get(0))
            .unwrap();
        drop(conn);

        let result = execute_restore_plan_with_root(
            &db_path,
            root.path().to_str().unwrap(),
            baseline.state_number,
            false,
        )
        .await;

        let err = result.expect_err("missing repo version should fail");
        let message = format!("{err:#}");
        assert!(
            message.contains("9.9.9"),
            "missing-version restore should surface the unresolved target version, got: {message}"
        );
        assert!(
            !message.contains("not yet implemented"),
            "missing-version restore should fail in preflight, not via the placeholder bail: {message}"
        );

        let conn = crate::commands::open_db(&db_path).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM changesets", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            before_changesets
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM system_states", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            before_states
        );
    }

    #[tokio::test]
    async fn test_state_restore_changeset_rolls_back_via_revert_metadata_wrapper() {
        let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
        let root = tempfile::tempdir().unwrap();
        let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

        let mut conn = crate::commands::open_db(&db_path).unwrap();
        let engine = conary_core::db::models::StateEngine::new(&conn);
        let baseline = engine.create_snapshot("baseline", None, None).unwrap();

        conary_core::db::transaction(&mut conn, |tx| {
            let mut cs = conary_core::db::models::Changeset::new("Install vim-9.1.0".to_string());
            let cs_id = cs.insert(tx)?;
            let mut vim = conary_core::db::models::Trove::new(
                "vim".to_string(),
                "9.1.0".to_string(),
                conary_core::db::models::TroveType::Package,
            );
            vim.architecture = Some("x86_64".to_string());
            vim.installed_by_changeset_id = Some(cs_id);
            vim.insert(tx)?;
            cs.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
            Ok::<_, conary_core::Error>(())
        })
        .unwrap();
        conary_core::db::models::StateEngine::new(&conn)
            .create_snapshot("drifted", None, None)
            .unwrap();
        drop(conn);

        execute_restore_plan_with_root(
            &db_path,
            root.path().to_str().unwrap(),
            baseline.state_number,
            false,
        )
        .await
        .unwrap();

        let conn = crate::commands::open_db(&db_path).unwrap();
        assert!(
            conary_core::db::models::Trove::find_one_by_name(&conn, "vim")
                .unwrap()
                .is_none()
        );
        let restore_changeset_id: i64 = conn
            .query_row(
                "SELECT id FROM changesets WHERE description = ?1 ORDER BY id DESC LIMIT 1",
                [format!(
                    "Restore state {} -> {}",
                    baseline.state_number + 1,
                    baseline.state_number
                )],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);

        crate::commands::cmd_rollback(
            restore_changeset_id,
            &db_path,
            root.path().to_str().unwrap(),
        )
        .await
        .unwrap();

        let conn = crate::commands::open_db(&db_path).unwrap();
        assert!(
            conary_core::db::models::Trove::find_one_by_name(&conn, "vim")
                .unwrap()
                .is_some()
        );
        let restore_changeset =
            conary_core::db::models::Changeset::find_by_id(&conn, restore_changeset_id)
                .unwrap()
                .unwrap();
        assert_eq!(
            restore_changeset.status,
            conary_core::db::models::ChangesetStatus::RolledBack
        );
    }

    #[tokio::test]
    async fn test_state_restore_install_plan_executes_under_wrapping_changeset() {
        let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
        let root = tempfile::tempdir().unwrap();
        let package_dir = tempfile::tempdir().unwrap();
        let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

        let package_path = build_test_ccs_package(package_dir.path(), "vim", "9.1.0");
        let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
        let (package_url, _server_handle) = serve_test_file(package_path.clone());

        let mut conn = crate::commands::open_db(&db_path).unwrap();
        let mut repo = Repository::new("arch-test".to_string(), package_url.clone());
        let repo_id = repo.insert(&conn).unwrap();

        let mut repo_pkg = RepositoryPackage::new(
            repo_id,
            "vim".to_string(),
            "9.1.0".to_string(),
            package_checksum.clone(),
            std::fs::metadata(&package_path)
                .unwrap()
                .len()
                .try_into()
                .unwrap(),
            package_url.clone(),
        );
        repo_pkg.architecture = Some("x86_64".to_string());
        repo_pkg.insert(&conn).unwrap();

        let mut resolution = PackageResolution::new(
            repo_id,
            "vim".to_string(),
            vec![ResolutionStrategy::Binary {
                url: package_url,
                checksum: package_checksum,
                delta_base: None,
            }],
        );
        resolution.version = Some("9.1.0".to_string());
        resolution.primary_strategy = PrimaryStrategy::Binary;
        resolution.insert(&conn).unwrap();

        conary_core::db::transaction(&mut conn, |tx| {
            let mut cs = Changeset::new("Install vim-9.1.0".to_string());
            let cs_id = cs.insert(tx)?;
            let mut vim = Trove::new("vim".to_string(), "9.1.0".to_string(), TroveType::Package);
            vim.architecture = Some("x86_64".to_string());
            vim.installed_by_changeset_id = Some(cs_id);
            vim.insert(tx)?;
            cs.update_status(tx, ChangesetStatus::Applied)?;
            Ok::<_, conary_core::Error>(())
        })
        .unwrap();
        let baseline = conary_core::db::models::StateEngine::new(&conn)
            .create_snapshot("baseline", None, None)
            .unwrap();

        conn.execute("DELETE FROM troves WHERE name = 'vim'", [])
            .unwrap();
        let _drifted = conary_core::db::models::StateEngine::new(&conn)
            .create_snapshot("drifted", None, None)
            .unwrap();

        let before_changesets: i64 = conn
            .query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get(0))
            .unwrap();
        let before_states: i64 = conn
            .query_row("SELECT COUNT(*) FROM system_states", [], |r| r.get(0))
            .unwrap();
        drop(conn);

        let result = execute_restore_plan_with_root(
            &db_path,
            root.path().to_str().unwrap(),
            baseline.state_number,
            false,
        )
        .await;

        assert!(
            result.is_ok(),
            "install restore should succeed under one wrapping changeset: {result:?}"
        );

        let conn = crate::commands::open_db(&db_path).unwrap();
        let vim = conary_core::db::models::Trove::find_one_by_name(&conn, "vim")
            .unwrap()
            .expect("vim should be restored");
        assert_eq!(vim.version, "9.1.0");
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM changesets", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            before_changesets + 1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM system_states", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            before_states + 1
        );
    }
}
