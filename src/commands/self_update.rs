// src/commands/self_update.rs

//! Self-update command: update the conary binary itself

use super::open_db;
use anyhow::Result;
use conary_core::db::paths::objects_dir;
use conary_core::self_update::{
    LatestVersionInfo, VersionCheckResult, apply_update, check_for_update,
    download_update_with_progress, extract_binary, get_update_channel, verify_binary,
};

fn check_update_signature(sha256: &str, signature: &Option<String>) -> Result<()> {
    let have_trusted_keys = !conary_core::self_update::TRUSTED_UPDATE_KEYS.is_empty();

    if !have_trusted_keys {
        // No trusted keys shipped yet -- signature verification is impossible.
        // Warn but allow the update.  Once release-signing keys are added to
        // TRUSTED_UPDATE_KEYS, this early return disappears and all updates
        // (signed or unsigned) must pass verification.
        if signature.is_some() {
            eprintln!(
                "Warning: update has a signature but no trusted keys are configured to verify it. \
                 Skipping verification."
            );
        } else {
            eprintln!(
                "Warning: update has no signature and no trusted keys are configured. \
                 Signature enforcement will be enabled once release keys are shipped."
            );
        }
        return Ok(());
    }

    // Trusted keys are configured — enforce signature verification.
    match signature {
        Some(sig) => {
            conary_core::self_update::verify_update_signature(sha256, sig)
                .map_err(|e| anyhow::anyhow!("Update signature verification failed: {e}"))?;
            println!("Signature verified");
        }
        None => {
            anyhow::bail!(
                "Update has no signature. Refusing to install an unsigned release. \
                 If this is a pre-signing development build, use a signed channel."
            );
        }
    }
    Ok(())
}

pub async fn cmd_self_update(
    db_path: &str,
    check: bool,
    force: bool,
    version: Option<String>,
) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    let conn = open_db(db_path)?;
    let channel_url = get_update_channel(&conn)?;

    println!("Current version: {current_version}");
    println!("Update channel: {channel_url}");

    if let Some(ref v) = version {
        anyhow::bail!(
            "--version {v} is not yet implemented. \
             Omit --version to install the latest available version."
        );
    }

    // Check for updates
    let result = check_for_update(&channel_url, current_version).await?;

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

    // Verify the SHA-256 digest signature *before* downloading the binary.
    // This ensures the server-advertised checksum is authentic (signed by a
    // trusted key) so that the post-download hash comparison is meaningful.
    // Flow: 1) check version -> 2) verify signature on digest -> 3) download
    //       -> 4) compare downloaded hash against signed digest -> 5) replace.
    if let VersionCheckResult::UpdateAvailable {
        ref sha256,
        ref signature,
        ..
    } = result
    {
        check_update_signature(sha256, signature)?;
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
            let info: LatestVersionInfo = reqwest::get(format!("{channel_url}/latest"))
                .await?
                .json()
                .await?;
            check_update_signature(&info.sha256, &info.signature)?;
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
        download_update_with_progress(&download_url, &sha256, temp_dir.path(), download_size)
            .await?;

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
