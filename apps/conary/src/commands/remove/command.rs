// apps/conary/src/commands/remove/command.rs

use std::io::Write;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::info;

use super::types::{RemoveInnerResult, RemoveLifecycleOptions};
use crate::commands::progress::RemoveProgress;
use crate::commands::{InstalledPackageSelector, SandboxMode, open_db, resolve_installed_package};

/// Remove an installed package
#[allow(clippy::too_many_arguments)]
pub async fn cmd_remove(
    package_name: &str,
    db_path: &str,
    version: Option<String>,
    architecture: Option<String>,
    sandbox_mode: SandboxMode,
    purge: bool,
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

    if trove.install_source.is_adopted() && !purge {
        anyhow::bail!(
            "Refusing to remove adopted package '{}': its files are not Conary-owned and \
             remain under native package manager authority. Use 'conary system unadopt {}' \
             to remove Conary tracking only, \
             or rerun with --purge only if deleting externally owned files is intentional.",
            package_name,
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

    let lifecycle_options =
        RemoveLifecycleOptions::new(sandbox_mode).with_purge_config_files(purge);
    if trove.install_source.is_adopted() && purge {
        println!(
            "WARNING: --purge specified for adopted package '{}'. \
             Files will be deleted from disk.",
            package_name
        );
    }

    let graph_result = super::native_graph::execute_installed_trove_remove_graph(
        &conn,
        &trove,
        db_path,
        package_name,
        lifecycle_options,
        &progress,
    )?;
    progress.finish(&format!(
        "Removed {} {}",
        graph_result.removal.trove.name, graph_result.removal.trove.version
    ));
    print_remove_summary(&graph_result.removal, &graph_result.stats);
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
    async fn no_current_generation_remove_publishes_without_mutating_ambient_root() {
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

        let payload = root.join("usr/bin/fixture");
        std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
        std::fs::write(&payload, "fixture").unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "fixture".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();
        crate::commands::test_helpers::insert_test_regular_file_with_parents(
            &conn,
            &db_path,
            "/usr/bin/fixture",
            b"fixture",
            0o755,
            trove_id,
            None,
        );
        drop(conn);

        cmd_remove(
            "fixture",
            db_path.to_string_lossy().as_ref(),
            None,
            None,
            SandboxMode::Always,
            false,
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read_to_string(&payload).unwrap(), "fixture");
        let conn = conary_core::db::open(&db_path).unwrap();
        assert!(
            conary_core::db::models::Trove::find_by_name(&conn, "fixture")
                .unwrap()
                .is_empty()
        );
        let runtime_root =
            conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.clone());
        assert!(
            conary_core::generation::mount::current_generation(runtime_root.root())
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn no_generation_remove_fails_closed_on_dangling_current_without_mutation() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);
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
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();
        crate::commands::test_helpers::insert_test_regular_file_with_parents(
            &conn,
            &db_path,
            "/usr/bin/fixture",
            b"fixture",
            0o755,
            trove_id,
            None,
        );
        drop(conn);

        let err = cmd_remove(
            "fixture",
            db_path.to_string_lossy().as_ref(),
            None,
            None,
            SandboxMode::Always,
            false,
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
    async fn selected_root_materialization_failure_leaves_no_pending_changeset() {
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "fixture".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let file = crate::commands::test_helpers::insert_test_regular_file_with_parents(
            &conn,
            &db_path,
            "/usr/bin/fixture",
            b"fixture",
            0o755,
            trove_id,
            None,
        );
        let runtime_root =
            conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.clone());
        let cas = conary_core::filesystem::CasStore::new(runtime_root.objects_dir()).unwrap();
        let content_hash = &file.content.as_ref().unwrap().sha256;
        std::fs::remove_file(cas.hash_to_path(content_hash).unwrap()).unwrap();
        drop(conn);

        let err = cmd_remove(
            "fixture",
            db_path.to_string_lossy().as_ref(),
            None,
            None,
            SandboxMode::Always,
            false,
        )
        .await
        .unwrap_err();
        let error_chain = format!("{err:#}");

        assert!(
            error_chain.contains("failed to retrieve CAS object")
                && error_chain.contains("/usr/bin/fixture"),
            "{error_chain}"
        );
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
    async fn remove_has_no_package_name_blocklist() {
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

        let payload = root.join("usr/bin/bash");
        std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
        std::fs::write(&payload, "bash").unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "bash".to_string(),
            "5.2.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();
        crate::commands::test_helpers::insert_test_regular_file_with_parents(
            &conn,
            &db_path,
            "/usr/bin/bash",
            b"bash",
            0o755,
            trove_id,
            None,
        );
        drop(conn);

        cmd_remove(
            "bash",
            db_path.to_string_lossy().as_ref(),
            None,
            None,
            SandboxMode::Always,
            false,
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read_to_string(&payload).unwrap(), "bash");
        let conn = conary_core::db::open(&db_path).unwrap();
        assert!(
            conary_core::db::models::Trove::find_by_name(&conn, "bash")
                .unwrap()
                .is_empty()
        );
    }
}
