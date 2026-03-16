// src/commands/self_update.rs

//! Self-update command: update the conary binary itself

use anyhow::Result;
use conary_core::db;
use conary_core::db::paths::objects_dir;
use conary_core::self_update::{
    LatestVersionInfo, VersionCheckResult, apply_update, check_for_update,
    download_update_with_progress, extract_binary, get_update_channel, verify_binary,
};

pub fn cmd_self_update(
    db_path: &str,
    check: bool,
    force: bool,
    version: Option<String>,
) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    let conn = db::open(db_path)?;
    let channel_url = get_update_channel(&conn)?;

    println!("Current version: {current_version}");
    println!("Update channel: {channel_url}");

    if let Some(ref v) = version {
        println!("Requested version: {v}");
        // TODO: support downloading a specific version instead of latest
    }

    // Check for updates
    let result = check_for_update(&channel_url, current_version)?;

    match &result {
        VersionCheckResult::UpToDate { version } => {
            if !force {
                println!("Already up to date (v{version})");
                return Ok(());
            }
            println!("Already at v{version}, but --force specified");
        }
        VersionCheckResult::UpdateAvailable {
            current,
            latest,
            size,
            ..
        } => {
            println!(
                "Update available: v{current} -> v{latest} ({:.1} MB)",
                *size as f64 / 1_048_576.0
            );
            if check {
                return Ok(());
            }
        }
    }

    // Determine download URL and expected version
    let (download_url, sha256, expected_version) = match &result {
        VersionCheckResult::UpdateAvailable {
            latest,
            download_url,
            sha256,
            ..
        } => (download_url.clone(), sha256.clone(), latest.clone()),
        VersionCheckResult::UpToDate { .. } => {
            // --force path: re-fetch latest info
            let info: LatestVersionInfo =
                reqwest::blocking::get(format!("{channel_url}/latest"))?.json()?;
            (info.download_url, info.sha256, info.version)
        }
    };

    // Determine target binary path (the currently running binary)
    let target_path = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Cannot determine current binary path: {e}"))?;

    // Download to temp dir on same filesystem as target
    let target_dir = target_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("/usr/bin"));
    let temp_dir = tempfile::tempdir_in(target_dir)?;

    println!("Downloading v{expected_version}...");
    let download_size = match &result {
        VersionCheckResult::UpdateAvailable { size, .. } => Some(*size),
        VersionCheckResult::UpToDate { .. } => None,
    };
    let ccs_path =
        download_update_with_progress(&download_url, &sha256, temp_dir.path(), download_size)?;

    println!("Extracting binary...");
    let new_binary = extract_binary(&ccs_path, target_dir)?;

    println!("Verifying new binary...");
    verify_binary(&new_binary, &expected_version)?;

    println!("Replacing binary...");
    let obj_dir = objects_dir(db_path);
    let obj_dir_str = obj_dir
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Objects directory path is not valid UTF-8"))?;
    apply_update(&new_binary, &target_path, obj_dir_str)?;

    println!(
        "Updated conary v{} -> v{}",
        current_version, expected_version
    );

    Ok(())
}
