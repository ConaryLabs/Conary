// apps/conary/src/commands/remove/autoremove.rs

use anyhow::Result;
use conary_core::db::models::{PackagePayloadOwnership, Trove};
use conary_core::scriptlet::ExecutionMode;
use std::collections::HashSet;
use tracing::info;

use super::types::RemoveLifecycleOptions;
use crate::commands::{SandboxMode, open_db};

#[derive(Debug, Clone, PartialEq, Eq)]
enum AutoremoveSkipReason {
    AdoptedNativeAuthority,
    Pinned,
}

#[derive(Debug, Clone)]
struct AutoremovePlan {
    removable: Vec<Trove>,
    skipped: Vec<(Trove, AutoremoveSkipReason)>,
}

/// Remove orphaned packages (installed as dependencies but no longer needed)
///
/// Finds packages that were installed as dependencies of other packages,
/// but are no longer required by any installed package.
pub fn cmd_autoremove(db_path: &str, dry_run: bool, sandbox_mode: SandboxMode) -> Result<()> {
    info!("Finding orphaned packages...");

    let conn = open_db(db_path)?;

    let orphans = conary_core::db::models::Trove::find_orphans(&conn)?;
    if orphans.is_empty() {
        println!("No orphaned packages found.");
        return Ok(());
    }

    let plan = plan_autoremove(orphans);
    if plan.removable.is_empty() {
        println!("No Conary-owned orphaned packages can be autoremoved.");
        print_autoremove_skips(&plan.skipped);
        return Ok(());
    }
    print_autoremove_candidates("Found", &plan.removable);
    print_autoremove_skips(&plan.skipped);

    if dry_run {
        println!("\nDry run - no packages will be removed.");
        println!("Run without --dry-run to remove these packages.");
        return Ok(());
    }

    // Fixed-point iteration: removing orphans may expose new orphans (transitive chains).
    // Re-query after each round until no more orphans are found.
    const MAX_ITERATIONS: usize = 100;
    let mut total_removed = 0;
    let mut total_failed = 0;
    let mut current_plan = plan;
    let mut failed_orphans = HashSet::new();

    for iteration in 0..MAX_ITERATIONS {
        if iteration > 0 {
            // Re-query orphans after previous round of removals
            let conn = open_db(db_path)?;
            let current_orphans = conary_core::db::models::Trove::find_orphans(&conn)?;
            if current_orphans.is_empty() {
                break;
            }
            current_plan = plan_autoremove(current_orphans);
            current_plan
                .removable
                .retain(|trove| !failed_orphans.contains(&autoremove_identity(trove)));
            if current_plan.removable.is_empty() {
                println!("\nNo additional Conary-owned orphaned packages can be autoremoved.");
                print_autoremove_skips(&current_plan.skipped);
                break;
            }
            print_autoremove_candidates("Found additional", &current_plan.removable);
            print_autoremove_skips(&current_plan.skipped);
        } else {
            println!(
                "\nRemoving {} orphaned package(s)...",
                current_plan.removable.len()
            );
        }

        let conn = open_db(db_path)?;
        preflight_autoremove_round(
            &conn,
            &current_plan.removable,
            db_path,
            RemoveLifecycleOptions::new(sandbox_mode),
        )?;

        let mut round_removed = 0;
        for trove in &current_plan.removable {
            println!("\nRemoving {} {}...", trove.name, trove.version);
            match super::cmd_remove(
                &trove.name,
                db_path,
                Some(trove.version.clone()),
                trove.architecture.clone(),
                sandbox_mode,
                false,
            ) {
                Ok(()) => {
                    round_removed += 1;
                }
                Err(e) => {
                    eprintln!("  Failed to remove {}: {}", trove.name, e);
                    failed_orphans.insert(autoremove_identity(trove));
                    total_failed += 1;
                }
            }
        }

        total_removed += round_removed;

        // If nothing was removed this round, no point continuing
        if round_removed == 0 {
            break;
        }
    }

    println!("\nAutoremove complete:");
    println!("  Removed: {} package(s)", total_removed);
    if total_failed > 0 {
        println!("  Failed: {} package(s)", total_failed);
        anyhow::bail!(
            "Autoremove failed for {} package(s); see summary above",
            total_failed
        );
    }

    Ok(())
}

fn preflight_autoremove_round(
    conn: &rusqlite::Connection,
    troves: &[Trove],
    db_path: &str,
    lifecycle_options: RemoveLifecycleOptions,
) -> Result<()> {
    for trove in troves {
        let Some(trove_id) = trove.id else {
            anyhow::bail!(
                "autoremove lifecycle execution preflight failed for {} {}: trove has no id",
                trove.name,
                trove.version
            );
        };
        let locked_root =
            crate::commands::generation::selected_root::LockedRuntimeRoot::acquire(db_path)?;
        let paths = PackagePayloadOwnership::load(conn, trove_id)?
            .lifecycle_paths()
            .to_vec();
        let native_transaction =
            crate::commands::install::native_events::PreparedNativeTransaction::prepare_remove(
                conn,
                trove_id,
                &trove.name,
                &trove.version,
                paths,
                false,
            );
        let native_transaction = match native_transaction {
            Ok(transaction) => transaction,
            Err(error) => {
                anyhow::bail!(
                    "autoremove lifecycle execution preflight failed for {} {}: {error}",
                    trove.name,
                    trove.version
                );
            }
        };
        let selected = locked_root.prepare(
            conn,
            format!("Autoremove preflight {}-{}", trove.name, trove.version),
        )?;
        if let Err(error) =
            native_transaction.preflight(selected.selected_root(), &ExecutionMode::Remove)
        {
            anyhow::bail!(
                "autoremove lifecycle execution preflight failed for {} {}: {error}",
                trove.name,
                trove.version
            );
        }
        let selected_root = selected
            .selected_root()
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("autoremove selected root is not valid UTF-8"))?;
        super::preflight_ccs_remove_hook(
            conn,
            trove,
            selected_root,
            lifecycle_options.sandbox_mode,
        )?;
    }

    Ok(())
}

fn plan_autoremove(orphaned: Vec<Trove>) -> AutoremovePlan {
    let mut removable = Vec::new();
    let mut skipped = Vec::new();

    for trove in orphaned {
        if trove.install_source.is_adopted() {
            skipped.push((trove, AutoremoveSkipReason::AdoptedNativeAuthority));
        } else if trove.pinned {
            skipped.push((trove, AutoremoveSkipReason::Pinned));
        } else {
            removable.push(trove);
        }
    }

    AutoremovePlan { removable, skipped }
}

fn print_autoremove_candidates(prefix: &str, troves: &[Trove]) {
    println!("{prefix} {} orphaned package(s):", troves.len());
    for trove in troves {
        print_autoremove_trove(trove);
    }
}

fn print_autoremove_skips(skipped: &[(Trove, AutoremoveSkipReason)]) {
    if skipped.is_empty() {
        return;
    }

    let adopted = skipped
        .iter()
        .filter(|(_, reason)| *reason == AutoremoveSkipReason::AdoptedNativeAuthority)
        .collect::<Vec<_>>();
    if !adopted.is_empty() {
        println!(
            "Skipping adopted orphaned package(s); native package-manager authority is preserved:"
        );
        for (trove, _) in adopted {
            print_autoremove_trove(trove);
        }
    }

    let protected = skipped
        .iter()
        .filter(|(_, reason)| *reason != AutoremoveSkipReason::AdoptedNativeAuthority)
        .collect::<Vec<_>>();
    if !protected.is_empty() {
        println!("Skipping protected orphaned package(s):");
        for (trove, reason) in protected {
            print!("  {} {}", trove.name, trove.version);
            if let Some(arch) = &trove.architecture {
                print!(" [{}]", arch);
            }
            println!(" ({:?})", reason);
        }
    }
}

fn print_autoremove_trove(trove: &Trove) {
    print!("  {} {}", trove.name, trove.version);
    if let Some(arch) = &trove.architecture {
        print!(" [{}]", arch);
    }
    println!();
}

fn autoremove_identity(trove: &Trove) -> (String, String, Option<String>) {
    (
        trove.name.clone(),
        trove.version.clone(),
        trove.architecture.clone(),
    )
}

#[cfg(test)]
#[path = "autoremove/tests.rs"]
mod tests;
