// apps/conary/src/commands/adopt/unadopt.rs

//! Stop tracking adopted native packages without deleting native files.

use std::path::Path;

use anyhow::{Result, bail};
use conary_core::db;
use conary_core::db::backup::CheckpointReason;
use conary_core::db::models::{Changeset, ChangesetStatus, Trove};
use conary_core::generation::mount::current_generation;
use conary_core::runtime_root::ConaryRuntimeRoot;

use super::checkpoint::write_db_checkpoint;
use super::hooks::remove_detected_sync_hooks;
use crate::commands::create_state_snapshot;

#[derive(Debug, Clone)]
pub struct UnadoptOptions {
    pub packages: Vec<String>,
    pub all: bool,
    pub dry_run: bool,
    pub keep_hooks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnadoptSummary {
    pub unadopted: Vec<String>,
    pub skipped_conary_owned: Vec<String>,
    pub changeset_id: Option<i64>,
    pub hooks_removed: bool,
}

#[derive(Debug)]
struct UnadoptPlan {
    adopted: Vec<Trove>,
    skipped_conary_owned: Vec<String>,
}

pub async fn cmd_unadopt(options: UnadoptOptions, db_path: &str) -> Result<UnadoptSummary> {
    cmd_unadopt_with_hook_remover(options, db_path, remove_detected_sync_hooks)
}

fn cmd_unadopt_with_hook_remover<F>(
    options: UnadoptOptions,
    db_path: &str,
    hook_remover: F,
) -> Result<UnadoptSummary>
where
    F: FnOnce() -> Result<bool>,
{
    validate_scope(&options)?;
    ensure_no_selected_generation_before_apply(db_path, options.dry_run)?;

    let mut conn = db::open(db_path)?;
    let plan = build_unadopt_plan(&conn, &options)?;
    let unadopted = sorted_trove_names(&plan.adopted);
    let mut skipped_conary_owned = plan.skipped_conary_owned;
    skipped_conary_owned.sort();

    if options.dry_run {
        print_dry_run_summary(&unadopted, &skipped_conary_owned);
        return Ok(UnadoptSummary {
            unadopted,
            skipped_conary_owned,
            changeset_id: None,
            hooks_removed: false,
        });
    }

    let target_ids = trove_ids(&plan.adopted)?;
    let changeset_id = if target_ids.is_empty() {
        None
    } else {
        write_db_checkpoint(db_path, CheckpointReason::PreMutation)?;
        let changeset_id = db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new(changeset_description(&unadopted));
            let changeset_id = changeset.insert(tx)?;
            for trove_id in &target_ids {
                Trove::delete(tx, *trove_id)?;
            }
            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(changeset_id)
        })?;

        create_state_snapshot(&conn, changeset_id, "Unadopt native packages")?;
        write_db_checkpoint(db_path, CheckpointReason::PostSuccess)?;
        Some(changeset_id)
    };

    let hooks_removed = if options.all && !options.keep_hooks {
        hook_remover()?
    } else {
        false
    };

    print_apply_summary(&unadopted, &skipped_conary_owned, hooks_removed);
    Ok(UnadoptSummary {
        unadopted,
        skipped_conary_owned,
        changeset_id,
        hooks_removed,
    })
}

fn validate_scope(options: &UnadoptOptions) -> Result<()> {
    if options.all == options.packages.is_empty() {
        return Ok(());
    }

    bail!("conary system unadopt requires either --all or one or more package names")
}

fn ensure_no_selected_generation_before_apply(db_path: &str, dry_run: bool) -> Result<()> {
    if dry_run {
        return Ok(());
    }

    let runtime_root = ConaryRuntimeRoot::from_db_path(Path::new(db_path));
    if let Some(generation) = current_generation(runtime_root.root())? {
        bail!(
            "Refusing to unadopt while Conary generation {generation} is selected. \
             Switch back to the native system first, then rerun conary system unadopt."
        );
    }

    Ok(())
}

fn build_unadopt_plan(
    conn: &rusqlite::Connection,
    options: &UnadoptOptions,
) -> Result<UnadoptPlan> {
    if options.all {
        let mut adopted = Vec::new();
        let mut skipped_conary_owned = Vec::new();

        for trove in Trove::list_all(conn)? {
            if trove.install_source.is_adopted() {
                adopted.push(trove);
            } else if trove.install_source.is_conary_owned() {
                skipped_conary_owned.push(trove.name);
            }
        }

        return Ok(UnadoptPlan {
            adopted,
            skipped_conary_owned,
        });
    }

    let mut adopted = Vec::new();
    let mut conary_owned = Vec::new();
    let mut missing = Vec::new();

    for package in &options.packages {
        let matches = Trove::find_by_name(conn, package)?;
        if matches.is_empty() {
            missing.push(package.clone());
            continue;
        }

        let mut package_adopted = Vec::new();
        for trove in matches {
            if trove.install_source.is_adopted() {
                package_adopted.push(trove);
            } else if trove.install_source.is_conary_owned() {
                conary_owned.push(trove.name);
            }
        }

        if package_adopted.is_empty() && !conary_owned.iter().any(|name| name == package) {
            missing.push(package.clone());
        }
        adopted.extend(package_adopted);
    }

    if !conary_owned.is_empty() {
        conary_owned.sort();
        conary_owned.dedup();
        bail!(
            "{} is Conary-owned, not adopted. Use conary remove or an explicit takeover handoff instead.",
            conary_owned.join(", ")
        );
    }

    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        bail!("{} is not currently adopted by Conary.", missing.join(", "));
    }

    Ok(UnadoptPlan {
        adopted,
        skipped_conary_owned: Vec::new(),
    })
}

fn trove_ids(troves: &[Trove]) -> Result<Vec<i64>> {
    troves
        .iter()
        .map(|trove| {
            trove
                .id
                .ok_or_else(|| anyhow::anyhow!("Trove {} is missing a database id", trove.name))
        })
        .collect()
}

fn sorted_trove_names(troves: &[Trove]) -> Vec<String> {
    let mut names: Vec<String> = troves.iter().map(|trove| trove.name.clone()).collect();
    names.sort();
    names
}

fn changeset_description(names: &[String]) -> String {
    if names.len() == 1 {
        return format!("Unadopt {}", names[0]);
    }
    format!("Unadopt {} native packages", names.len())
}

fn print_dry_run_summary(unadopted: &[String], skipped_conary_owned: &[String]) {
    if unadopted.is_empty() {
        println!("No adopted packages would be unadopted.");
    } else {
        println!("Would unadopt {} package(s):", unadopted.len());
        for name in unadopted {
            println!("  {name}");
        }
    }

    if !skipped_conary_owned.is_empty() {
        println!(
            "Skipping {} Conary-owned package(s): {}",
            skipped_conary_owned.len(),
            skipped_conary_owned.join(", ")
        );
    }
    println!("Native package files will not be deleted.");
}

fn print_apply_summary(unadopted: &[String], skipped_conary_owned: &[String], hooks_removed: bool) {
    if unadopted.is_empty() {
        println!("No adopted packages were unadopted.");
    } else {
        println!("Unadopted {} package(s):", unadopted.len());
        for name in unadopted {
            println!("  {name}");
        }
    }

    if !skipped_conary_owned.is_empty() {
        println!(
            "Left {} Conary-owned package(s) in place: {}",
            skipped_conary_owned.len(),
            skipped_conary_owned.join(", ")
        );
    }

    if hooks_removed {
        println!("Removed native package manager sync hooks.");
    }
    println!("Native package files were not deleted.");
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use conary_core::db::models::{Changeset, ChangesetStatus, InstallSource, TroveType};

    use super::*;
    use crate::commands::test_helpers::create_test_db;

    fn options_for_package(name: &str) -> UnadoptOptions {
        UnadoptOptions {
            packages: vec![name.to_string()],
            all: false,
            dry_run: false,
            keep_hooks: false,
        }
    }

    fn options_for_all() -> UnadoptOptions {
        UnadoptOptions {
            packages: Vec::new(),
            all: true,
            dry_run: false,
            keep_hooks: false,
        }
    }

    fn seed_package(db_path: &str, name: &str, source: InstallSource) -> i64 {
        let mut conn = db::open(db_path).unwrap();
        db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new(format!("Seed {name}"));
            let changeset_id = changeset.insert(tx)?;
            let mut trove = Trove::new_with_source(
                name.to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                source,
            );
            trove.installed_by_changeset_id = Some(changeset_id);
            let trove_id = trove.insert(tx)?;
            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(trove_id)
        })
        .unwrap()
    }

    fn package_names(db_path: &str, name: &str) -> Vec<String> {
        let conn = db::open(db_path).unwrap();
        Trove::find_by_name(&conn, name)
            .unwrap()
            .into_iter()
            .map(|trove| trove.name)
            .collect()
    }

    #[test]
    fn unadopt_named_package_removes_adopted_tracking_only() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedTrack);

        let summary =
            cmd_unadopt_with_hook_remover(options_for_package("curl"), &db_path, || Ok(false))
                .unwrap();

        assert_eq!(summary.unadopted, vec!["curl"]);
        assert!(summary.changeset_id.is_some());
        assert!(package_names(&db_path, "curl").is_empty());
    }

    #[test]
    fn unadopt_all_leaves_conary_owned_packages() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedTrack);
        seed_package(&db_path, "bash", InstallSource::File);

        let summary =
            cmd_unadopt_with_hook_remover(options_for_all(), &db_path, || Ok(false)).unwrap();

        assert_eq!(summary.unadopted, vec!["curl"]);
        assert_eq!(summary.skipped_conary_owned, vec!["bash"]);
        assert!(package_names(&db_path, "curl").is_empty());
        assert_eq!(package_names(&db_path, "bash"), vec!["bash"]);
    }

    #[test]
    fn unadopt_dry_run_does_not_mutate_database() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedFull);
        let mut options = options_for_package("curl");
        options.dry_run = true;

        let summary = cmd_unadopt_with_hook_remover(options, &db_path, || {
            panic!("dry-run must not remove hooks")
        })
        .unwrap();

        assert_eq!(summary.unadopted, vec!["curl"]);
        assert_eq!(summary.changeset_id, None);
        assert_eq!(package_names(&db_path, "curl"), vec!["curl"]);
    }

    #[cfg(unix)]
    #[test]
    fn unadopt_refuses_selected_generation_before_db_mutation() {
        let (temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedTrack);
        let generation_path = temp_dir.path().join("generations/7");
        std::fs::create_dir_all(&generation_path).unwrap();
        std::os::unix::fs::symlink(&generation_path, temp_dir.path().join("current")).unwrap();

        let err =
            cmd_unadopt_with_hook_remover(options_for_package("curl"), &db_path, || Ok(false))
                .unwrap_err();

        assert!(err.to_string().contains("generation 7"));
        assert_eq!(package_names(&db_path, "curl"), vec!["curl"]);
    }

    #[test]
    fn unadopt_named_conary_owned_package_is_rejected() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "bash", InstallSource::Repository);

        let err =
            cmd_unadopt_with_hook_remover(options_for_package("bash"), &db_path, || Ok(false))
                .unwrap_err();

        assert!(err.to_string().contains("Conary-owned"));
        assert_eq!(package_names(&db_path, "bash"), vec!["bash"]);
    }

    #[test]
    fn unadopt_all_removes_hooks_by_default() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedTrack);

        let summary =
            cmd_unadopt_with_hook_remover(options_for_all(), &db_path, || Ok(true)).unwrap();

        assert!(summary.hooks_removed);
    }

    #[test]
    fn unadopt_all_keep_hooks_skips_hook_removal() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedTrack);
        let mut options = options_for_all();
        options.keep_hooks = true;
        let called = Cell::new(false);

        let summary = cmd_unadopt_with_hook_remover(options, &db_path, || {
            called.set(true);
            Ok(true)
        })
        .unwrap();

        assert!(!summary.hooks_removed);
        assert!(!called.get());
    }
}
