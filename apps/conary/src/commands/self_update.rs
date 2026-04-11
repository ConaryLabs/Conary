// src/commands/self_update.rs

//! Self-update command: update the conary binary itself

use super::open_db;
use anyhow::Result;
use conary_core::db::models::settings;
use conary_core::db::paths::objects_dir;
use conary_core::self_update::{
    LatestVersionInfo, VersionCheckResult, apply_update, check_for_update,
    download_update_with_progress, extract_binary, fetch_latest_version_info, fetch_version_info,
    get_update_channel,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;
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
        // No trusted keys are configured in this build, so signature
        // verification is impossible. Refuse by default; the user must pass
        // --no-verify to proceed with an unverifiable update. This prevents
        // silent unsigned binary replacement when TRUSTED_UPDATE_KEYS is empty.
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

fn validate_requested_version(version: &str) -> Result<()> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() == 3 && parts.iter().all(|part| part.parse::<u64>().is_ok()) {
        Ok(())
    } else {
        anyhow::bail!("Invalid version format: {version} (expected SemVer x.y.z)");
    }
}

fn validate_sha256_hex(sha256: &str) -> Result<()> {
    if sha256.len() == 64 && sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Ok(())
    } else {
        anyhow::bail!("Invalid SHA-256 digest: expected 64 hex characters");
    }
}

fn print_trusted_update_keys() {
    if conary_core::self_update::TRUSTED_UPDATE_KEYS.is_empty() {
        println!("No trusted self-update keys configured.");
        return;
    }

    for key in conary_core::self_update::TRUSTED_UPDATE_KEYS {
        println!("{key}");
    }
}

fn verify_detached_signature_file(
    sha256_hex: &str,
    signature_path: &Path,
    trusted_keys: &[String],
) -> Result<()> {
    validate_sha256_hex(sha256_hex)?;

    let signature = std::fs::read_to_string(signature_path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to read signature file {}: {e}",
            signature_path.display()
        )
    })?;
    let signature = signature.trim();
    if signature.is_empty() {
        anyhow::bail!("Signature file {} is empty", signature_path.display());
    }

    let mut key_refs: Vec<&str> = trusted_keys.iter().map(String::as_str).collect();
    key_refs.extend(
        conary_core::self_update::TRUSTED_UPDATE_KEYS
            .iter()
            .copied(),
    );

    if key_refs.is_empty() {
        anyhow::bail!(
            "No trusted keys configured or provided. Supply --trusted-key <HEX> or configure TRUSTED_UPDATE_KEYS first."
        );
    }

    conary_core::self_update::verify_update_signature_with_keys(sha256_hex, signature, &key_refs)
        .map_err(|e| anyhow::anyhow!("Update signature verification failed: {e}"))?;

    println!("Signature verified");
    Ok(())
}

pub struct SelfUpdateOptions {
    pub check: bool,
    pub force: bool,
    pub version: Option<String>,
    pub no_verify: bool,
    pub verify_sha256: Option<String>,
    pub verify_signature_file: Option<String>,
    pub trusted_keys: Vec<String>,
    pub print_trusted_keys: bool,
}

pub async fn cmd_self_update(db_path: &str, options: SelfUpdateOptions) -> Result<()> {
    let SelfUpdateOptions {
        check,
        force,
        version,
        no_verify,
        verify_sha256,
        verify_signature_file,
        trusted_keys,
        print_trusted_keys,
    } = options;

    if print_trusted_keys {
        if check
            || force
            || version.is_some()
            || no_verify
            || verify_sha256.is_some()
            || verify_signature_file.is_some()
            || !trusted_keys.is_empty()
        {
            anyhow::bail!(
                "--print-trusted-keys cannot be combined with update or offline verification flags"
            );
        }

        print_trusted_update_keys();
        return Ok(());
    }

    let offline_verify_mode =
        verify_sha256.is_some() || verify_signature_file.is_some() || !trusted_keys.is_empty();
    if offline_verify_mode {
        if check || force || version.is_some() || no_verify {
            anyhow::bail!(
                "Offline signature verification mode cannot be combined with update/install flags"
            );
        }

        let sha256 = verify_sha256.ok_or_else(|| {
            anyhow::anyhow!("--verify-sha256 is required for offline signature verification")
        })?;
        let signature_file = verify_signature_file.ok_or_else(|| {
            anyhow::anyhow!(
                "--verify-signature-file is required for offline signature verification"
            )
        })?;

        verify_detached_signature_file(&sha256, Path::new(&signature_file), &trusted_keys)?;
        return Ok(());
    }

    let current_version = env!("CARGO_PKG_VERSION");
    let conn = open_db(db_path)?;
    let channel_url = get_update_channel(&conn)?;
    let have_trusted_keys = !conary_core::self_update::TRUSTED_UPDATE_KEYS.is_empty();
    let user_agent = format!("conary/{current_version}");

    println!("Current version: {current_version}");
    println!("Update channel: {channel_url}");

    // Check for updates
    let result = if let Some(requested_version) = version.as_deref() {
        validate_requested_version(requested_version)?;
        let info = fetch_version_info(&channel_url, requested_version, &user_agent).await?;
        if info.version == current_version {
            VersionCheckResult::UpToDate {
                version: info.version,
            }
        } else {
            VersionCheckResult::UpdateAvailable {
                current: current_version.to_string(),
                latest: info.version,
                download_url: info.download_url,
                sha256: info.sha256,
                size: info.size,
                signature: info.signature,
            }
        }
    } else {
        check_for_update(&channel_url, current_version).await?
    };

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
            let info: LatestVersionInfo = if let Some(requested_version) = version.as_deref() {
                fetch_version_info(&channel_url, requested_version, &user_agent).await?
            } else {
                fetch_latest_version_info(&channel_url, &user_agent).await?
            };
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
    use super::{
        NO_VERIFY_AUDIT_KEY, record_no_verify_audit_event, validate_requested_version,
        verify_detached_signature_file,
    };
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
    use conary_core::db::models::settings;
    use conary_core::db::schema;
    use ed25519_dalek::Signer;
    use rusqlite::Connection;
    use tempfile::tempdir;

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
            "https://remi.conary.io/v1/ccs/conary",
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

    #[test]
    fn validate_requested_version_accepts_semver_triple() {
        validate_requested_version("1.2.3").unwrap();
    }

    #[test]
    fn validate_requested_version_rejects_non_semver() {
        let err = validate_requested_version("latest").unwrap_err();
        assert!(err.to_string().contains("Invalid version format"));
    }

    #[test]
    fn verify_detached_signature_file_accepts_custom_trusted_key() {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let verifying_key_hex = hex::encode(signing_key.verifying_key().as_bytes());
        let sha256_hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let signature = signing_key.sign(sha256_hex.as_bytes());
        let signature_b64 = BASE64.encode(signature.to_bytes());

        let temp_dir = tempdir().unwrap();
        let signature_path = temp_dir.path().join("conary.sig");
        std::fs::write(&signature_path, format!("{signature_b64}\n")).unwrap();

        verify_detached_signature_file(sha256_hex, &signature_path, &[verifying_key_hex])
            .expect("custom trusted key should verify the detached signature");
    }

    #[test]
    fn verify_detached_signature_file_rejects_untrusted_signature() {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
        let sha256_hex = "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd";
        let signature = signing_key.sign(sha256_hex.as_bytes());
        let signature_b64 = BASE64.encode(signature.to_bytes());

        let temp_dir = tempdir().unwrap();
        let signature_path = temp_dir.path().join("conary.sig");
        std::fs::write(&signature_path, format!("{signature_b64}\n")).unwrap();

        let err = verify_detached_signature_file(sha256_hex, &signature_path, &[])
            .expect_err("verification should fail when no trusted key matches the signature");
        assert!(
            err.to_string()
                .contains("Update signature verification failed: invalid signature")
        );
    }
}
