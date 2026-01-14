// src/commands/config.rs

//! Configuration file management CLI commands
//!
//! Commands for tracking, diffing, and managing configuration files.

use anyhow::Result;
use std::path::Path;
use tracing::info;

use conary::db::models::{ConfigBackup, ConfigFile, ConfigStatus, Trove};
use conary::filesystem::CasStore;

/// List configuration files
///
/// With no arguments, lists all modified config files.
/// With a package name, lists all config files for that package.
pub fn cmd_config_list(db_path: &str, package: Option<&str>, all: bool) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    if let Some(pkg_name) = package {
        // List config files for a specific package
        let troves = Trove::find_by_name(&conn, pkg_name)?;
        if troves.is_empty() {
            return Err(anyhow::anyhow!("Package '{}' is not installed", pkg_name));
        }

        for trove in &troves {
            if let Some(trove_id) = trove.id {
                let configs = ConfigFile::find_by_trove(&conn, trove_id)?;
                if configs.is_empty() {
                    println!("{} {}: no config files", trove.name, trove.version);
                } else {
                    println!("{} {} ({} config files):", trove.name, trove.version, configs.len());
                    for config in &configs {
                        let status_marker = match config.status {
                            ConfigStatus::Pristine => " ",
                            ConfigStatus::Modified => "M",
                            ConfigStatus::Missing => "!",
                        };
                        let noreplace = if config.noreplace { "N" } else { " " };
                        println!("  {} {} {}", status_marker, noreplace, config.path);
                    }
                }
            }
        }
    } else if all {
        // List all config files
        let configs = ConfigFile::list_all(&conn)?;
        if configs.is_empty() {
            println!("No config files tracked.");
            return Ok(());
        }

        println!("All config files ({}):", configs.len());
        for config in &configs {
            let status_marker = match config.status {
                ConfigStatus::Pristine => " ",
                ConfigStatus::Modified => "M",
                ConfigStatus::Missing => "!",
            };
            let noreplace = if config.noreplace { "N" } else { " " };
            println!("  {} {} {}", status_marker, noreplace, config.path);
        }
    } else {
        // List only modified config files
        let configs = ConfigFile::find_modified(&conn)?;
        if configs.is_empty() {
            println!("No modified config files.");
            return Ok(());
        }

        println!("Modified config files ({}):", configs.len());
        for config in &configs {
            let noreplace = if config.noreplace { " (noreplace)" } else { "" };
            println!("  {}{}", config.path, noreplace);
            if let Some(modified_at) = &config.modified_at {
                println!("    Modified: {}", modified_at);
            }
        }
    }

    Ok(())
}

/// Show diff between installed config file and package version
pub fn cmd_config_diff(db_path: &str, path: &str, root: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let config = ConfigFile::find_by_path(&conn, path)?
        .ok_or_else(|| anyhow::anyhow!("Config file '{}' is not tracked", path))?;

    // Get the CAS store
    let objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");
    let cas = CasStore::new(&objects_dir)?;

    // Get the original (package) content from CAS
    let original_content = cas.retrieve(&config.original_hash)
        .map_err(|_| anyhow::anyhow!("Original config content not found in CAS"))?;
    let original_str = String::from_utf8_lossy(&original_content);

    // Get the current filesystem content
    let fs_path = Path::new(root).join(path.trim_start_matches('/'));
    if !fs_path.exists() {
        println!("--- {} (package version)", path);
        println!("+++ {} (missing)", path);
        println!("File has been deleted from filesystem");
        return Ok(());
    }

    let current_content = std::fs::read_to_string(&fs_path)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path, e))?;

    if original_str == current_content {
        println!("Config file '{}' is unchanged from package version", path);
        return Ok(());
    }

    // Generate a simple diff
    println!("--- {} (package version)", path);
    println!("+++ {} (current)", path);
    println!();

    // Simple line-by-line diff (could use a proper diff library for better output)
    let original_lines: Vec<&str> = original_str.lines().collect();
    let current_lines: Vec<&str> = current_content.lines().collect();

    // Very basic diff - show lines that differ
    let max_lines = original_lines.len().max(current_lines.len());
    for i in 0..max_lines {
        let orig = original_lines.get(i);
        let curr = current_lines.get(i);

        match (orig, curr) {
            (Some(o), Some(c)) if o != c => {
                println!("-{}", o);
                println!("+{}", c);
            }
            (Some(o), None) => {
                println!("-{}", o);
            }
            (None, Some(c)) => {
                println!("+{}", c);
            }
            _ => {} // Lines match, don't print
        }
    }

    Ok(())
}

/// Backup a config file to CAS
pub fn cmd_config_backup(db_path: &str, path: &str, root: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let config = ConfigFile::find_by_path(&conn, path)?
        .ok_or_else(|| anyhow::anyhow!("Config file '{}' is not tracked", path))?;

    let config_id = config.id
        .ok_or_else(|| anyhow::anyhow!("Config file has no ID"))?;

    // Read the current file
    let fs_path = Path::new(root).join(path.trim_start_matches('/'));
    if !fs_path.exists() {
        return Err(anyhow::anyhow!("Config file '{}' does not exist", path));
    }

    let content = std::fs::read(&fs_path)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path, e))?;

    // Store in CAS
    let objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");
    let cas = CasStore::new(&objects_dir)?;
    let hash = cas.store(&content)?;

    // Create backup record
    let mut backup = ConfigBackup::new(config_id, hash.clone(), "manual".to_string());
    backup.insert(&conn)?;

    info!("Backed up {} with hash {}", path, hash);
    println!("Backed up: {}", path);
    println!("  Hash: {}", hash);

    Ok(())
}

/// Restore a config file from backup
pub fn cmd_config_restore(db_path: &str, path: &str, root: &str, backup_id: Option<i64>) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let config = ConfigFile::find_by_path(&conn, path)?
        .ok_or_else(|| anyhow::anyhow!("Config file '{}' is not tracked", path))?;

    let config_id = config.id
        .ok_or_else(|| anyhow::anyhow!("Config file has no ID"))?;

    // Find the backup to restore
    let backup = if let Some(id) = backup_id {
        ConfigBackup::find_by_config_file(&conn, config_id)?
            .into_iter()
            .find(|b| b.id == Some(id))
            .ok_or_else(|| anyhow::anyhow!("Backup {} not found for {}", id, path))?
    } else {
        // Use most recent backup
        ConfigBackup::find_latest(&conn, config_id)?
            .ok_or_else(|| anyhow::anyhow!("No backups found for {}", path))?
    };

    // Get content from CAS
    let objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");
    let cas = CasStore::new(&objects_dir)?;

    let content = cas.retrieve(&backup.backup_hash)
        .map_err(|_| anyhow::anyhow!("Backup content not found in CAS"))?;

    // Write to filesystem
    let fs_path = Path::new(root).join(path.trim_start_matches('/'));

    // Backup current version first
    if fs_path.exists() {
        let current = std::fs::read(&fs_path)?;
        let current_hash = cas.store(&current)?;
        let mut pre_restore = ConfigBackup::new(config_id, current_hash, "pre-restore".to_string());
        pre_restore.insert(&conn)?;
    }

    // Write restored content
    std::fs::write(&fs_path, &content)
        .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path, e))?;

    // Update config status
    config.mark_pristine(&conn, &backup.backup_hash)?;

    info!("Restored {} from backup", path);
    println!("Restored: {}", path);
    println!("  From backup: {} ({})", backup.backup_hash, backup.reason);

    Ok(())
}

/// Check and update status of config files
pub fn cmd_config_check(db_path: &str, root: &str, package: Option<&str>) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let configs = if let Some(pkg_name) = package {
        let troves = Trove::find_by_name(&conn, pkg_name)?;
        if troves.is_empty() {
            return Err(anyhow::anyhow!("Package '{}' is not installed", pkg_name));
        }
        let mut all_configs = Vec::new();
        for trove in &troves {
            if let Some(trove_id) = trove.id {
                all_configs.extend(ConfigFile::find_by_trove(&conn, trove_id)?);
            }
        }
        all_configs
    } else {
        ConfigFile::list_all(&conn)?
    };

    if configs.is_empty() {
        println!("No config files to check.");
        return Ok(());
    }

    let _objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");

    let mut modified_count = 0;
    let mut missing_count = 0;
    let mut pristine_count = 0;

    for config in &configs {
        let fs_path = Path::new(root).join(config.path.trim_start_matches('/'));

        if !fs_path.exists() {
            if config.status != ConfigStatus::Missing {
                config.mark_missing(&conn)?;
            }
            missing_count += 1;
            continue;
        }

        // Compute current hash
        let content = std::fs::read(&fs_path)?;
        let current_hash = CasStore::compute_hash(&content);

        if current_hash == config.original_hash {
            if config.status != ConfigStatus::Pristine {
                config.mark_pristine(&conn, &current_hash)?;
            }
            pristine_count += 1;
        } else {
            if config.status != ConfigStatus::Modified {
                config.mark_modified(&conn, &current_hash)?;
            }
            modified_count += 1;
        }
    }

    println!("Config file status check complete:");
    println!("  Pristine: {}", pristine_count);
    println!("  Modified: {}", modified_count);
    println!("  Missing:  {}", missing_count);

    Ok(())
}

/// Show backups for a config file
pub fn cmd_config_backups(db_path: &str, path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let config = ConfigFile::find_by_path(&conn, path)?
        .ok_or_else(|| anyhow::anyhow!("Config file '{}' is not tracked", path))?;

    let config_id = config.id
        .ok_or_else(|| anyhow::anyhow!("Config file has no ID"))?;

    let backups = ConfigBackup::find_by_config_file(&conn, config_id)?;

    if backups.is_empty() {
        println!("No backups for {}", path);
        return Ok(());
    }

    println!("Backups for {} ({}):", path, backups.len());
    for backup in &backups {
        let id = backup.id.unwrap_or(0);
        let created = backup.created_at.as_deref().unwrap_or("unknown");
        println!("  [{}] {} - {} ({})", id, &backup.backup_hash[..12], backup.reason, created);
    }

    Ok(())
}
