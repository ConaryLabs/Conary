// apps/conary/src/commands/db_backup.rs

use anyhow::{Result, bail};
use conary_core::db::backup::{RecoveryOptions, list_backups, recover_latest, verify_latest};

pub fn cmd_db_backup_list(db_path: &str) -> Result<()> {
    let records = list_backups(db_path)?;
    if records.is_empty() {
        println!("No Conary DB backups found.");
        return Ok(());
    }

    println!("Conary DB backups:");
    for record in records.iter().rev() {
        println!(
            "  {}  {}  schema={}  {}",
            record.manifest.created_at,
            record.manifest.reason.as_str(),
            record.manifest.db_schema_version,
            record.backup_path.display()
        );
    }
    Ok(())
}

pub fn cmd_db_backup_verify(db_path: &str, latest: bool) -> Result<()> {
    require_latest(latest)?;
    let verification = verify_latest(db_path)?;
    println!(
        "Verified Conary DB backup: {} (schema={}, integrity_check={})",
        verification.backup_path.display(),
        verification.db_schema_version,
        verification.integrity_check
    );
    Ok(())
}

pub fn cmd_db_backup_recover(
    db_path: &str,
    latest: bool,
    dry_run: bool,
    yes: bool,
    replace_healthy_db: bool,
) -> Result<()> {
    require_latest(latest)?;
    let outcome = recover_latest(
        db_path,
        RecoveryOptions {
            dry_run,
            yes,
            replace_healthy_db,
        },
    )?;

    if outcome.dry_run {
        println!(
            "Would recover Conary DB from backup: {}",
            outcome.backup_path.display()
        );
        return Ok(());
    }

    println!(
        "Recovered Conary DB from backup: {}",
        outcome.backup_path.display()
    );
    if !outcome.quarantined_paths.is_empty() {
        println!("Quarantined previous DB files:");
        for path in outcome.quarantined_paths {
            println!("  {}", path.display());
        }
    }
    Ok(())
}

fn require_latest(latest: bool) -> Result<()> {
    if !latest {
        bail!("this preview recovery path requires --latest");
    }
    Ok(())
}
