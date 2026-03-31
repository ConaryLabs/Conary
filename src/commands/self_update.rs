// src/commands/self_update.rs

//! Self-update command: update the conary binary itself

use super::open_db;
use anyhow::Result;
use conary_core::db::models::settings;
use conary_core::db::paths::objects_dir;
use conary_core::self_update::{
    LatestVersionInfo, VersionCheckResult, apply_update, check_for_update,
    download_update_with_progress, extract_binary, fetch_latest_version_info, get_update_channel,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const NO_VERIFY_AUDIT_KEY: &str = "self-update.no-verify-audit";
const MAX_NO_VERIFY_AUDIT_EVENTS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoVerifyAuditEvent {
    timestamp_unix: u64,
    channel_url: String,
    current_version: String,
    target_version: String,
}

fn check_update_signature(sha256: &str, signature: &Option<String>, no_verify: bool) -> Result<()> {
    // --no-verify explicitly bypasses all signature checks
    if no_verify {
        eprintln!("Warning: --no-verify specified, skipping signature verification.");
        return Ok(());
    }

    let have_trusted_keys = !conary_core::self_update::TRUSTED_UPDATE_KEYS.is_empty();

    if !have_trusted_keys {
        // No trusted keys shipped yet -- signature verification is impossible.
        // Refuse by default; the user must pass --no-verify to proceed with
        // an unverifiable update.  This prevents silent unsigned binary
        // replacement when TRUSTED_UPDATE_KEYS is empty.
        if signature.is_some() {
            anyhow::bail!(
                "Update has a signature but no trusted keys are configured to verify it. \
                 Use --no-verify to proceed without verification (NOT RECOMMENDED)."
            );
        }
        anyhow::bail!(
            "Update has no signature and no trusted keys are configured. \
             Refusing unsigned update. Use --no-verify to override (NOT RECOMMENDED)."
        );
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
                 Use --no-verify to override (NOT RECOMMENDED)."
            );
        }
    }
    Ok(())
}

fn record_no_verify_audit_event(
    conn: &Connection,
    have_trusted_keys: bool,
    channel_url: &str,
    current_version: &str,
    target_version: &str,
) -> Result<()> {
    if !have_trusted_keys {
        return Ok(());
    }

    let mut events = settings::get(conn, NO_VERIFY_AUDIT_KEY)?
        .and_then(|value| serde_json::from_str::<Vec<NoVerifyAuditEvent>>(&value).ok())
        .unwrap_or_default();

    let timestamp_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow::anyhow!("System clock before Unix epoch: {e}"))?
        .as_secs();

    events.push(NoVerifyAuditEvent {
        timestamp_unix,
        channel_url: channel_url.to_string(),
        current_version: current_version.to_string(),
        target_version: target_version.to_string(),
    });

    if events.len() > MAX_NO_VERIFY_AUDIT_EVENTS {
        let drop_count = events.len() - MAX_NO_VERIFY_AUDIT_EVENTS;
        events.drain(0..drop_count);
    }

    settings::set(conn, NO_VERIFY_AUDIT_KEY, &serde_json::to_string(&events)?)?;
    eprintln!(
        "Warning: --no-verify bypassed update signature verification even though trusted keys are configured. Event recorded in {}.",
        NO_VERIFY_AUDIT_KEY
    );
    Ok(())
}

pub async fn cmd_self_update(
    db_path: &str,
    check: bool,
    force: bool,
    version: Option<String>,
    no_verify: bool,
) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    let conn = open_db(db_path)?;
    let channel_url = get_update_channel(&conn)?;
    let have_trusted_keys = !conary_core::self_update::TRUSTED_UPDATE_KEYS.is_empty();

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
        check_update_signature(sha256, signature, no_verify)?;
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
            // --force path: re-fetch latest info using the same bounded metadata path
            let info: LatestVersionInfo =
                fetch_latest_version_info(&channel_url, &format!("conary/{current_version}"))
                    .await?;
            check_update_signature(&info.sha256, &info.signature, no_verify)?;
            (info.download_url, info.sha256, info.version)
        }
    };

    if no_verify {
        record_no_verify_audit_event(
            &conn,
            have_trusted_keys,
            &channel_url,
            current_version,
            &expected_version,
        )?;
    }

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

#[cfg(test)]
mod tests {
    use super::{NO_VERIFY_AUDIT_KEY, record_no_verify_audit_event};
    use conary_core::db::models::settings;
    use conary_core::db::schema;
    use rusqlite::Connection;

    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn record_no_verify_audit_event_persists_when_trusted_keys_exist() {
        let conn = create_test_db();

        record_no_verify_audit_event(
            &conn,
            true,
            "https://packages.conary.io/v1/ccs/conary",
            "0.7.0",
            "0.8.0",
        )
        .unwrap();

        let value = settings::get(&conn, NO_VERIFY_AUDIT_KEY)
            .unwrap()
            .expect("audit record should be written");
        assert!(value.contains("\"current_version\":\"0.7.0\""));
        assert!(value.contains("\"target_version\":\"0.8.0\""));
    }
}
