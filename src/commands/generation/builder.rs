// src/commands/generation/builder.rs
//! Generation builder — creates EROFS images from current system state

use super::composefs::preflight_composefs;
use super::metadata::{
    GenerationMetadata, ROOT_SYMLINKS, generation_path, generations_dir, is_excluded,
};
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::{FileEntry, StateEngine, Trove};
use conary_core::db::paths::objects_dir;
use std::io::BufWriter;
use tracing::{debug, info};

/// Build a new generation as an EROFS image from the current system state
///
/// Creates a snapshot of the system state, then builds an EROFS image
/// containing all installed package files with CAS digest references
/// suitable for composefs mounting.
pub fn build_generation(
    conn: &rusqlite::Connection,
    db_path: &str,
    summary: &str,
) -> Result<i64> {
    // Step 1: Composefs preflight check
    let obj_dir = objects_dir(db_path);
    let caps =
        preflight_composefs(&obj_dir).context("Composefs preflight failed")?;

    // Step 2: Ensure generations base directory exists
    std::fs::create_dir_all(generations_dir())
        .context("Failed to create generations directory")?;

    // Step 3: Create system state snapshot
    let engine = StateEngine::new(conn);
    let state = engine
        .create_snapshot(summary, None, None)
        .context("Failed to create system state snapshot")?;
    let gen_number = state.state_number;

    // Step 4: Verify generation dir doesn't exist, then create it
    let gen_dir = generation_path(gen_number);
    if gen_dir.exists() {
        return Err(anyhow!(
            "Generation directory already exists: {}",
            gen_dir.display()
        ));
    }
    std::fs::create_dir_all(&gen_dir).with_context(|| {
        format!(
            "Failed to create generation directory: {}",
            gen_dir.display()
        )
    })?;

    // Step 5: Build EROFS image
    let mut builder = conary_erofs::builder::ErofsBuilder::new();

    // Add root directory
    builder.add_directory("/", 0o755, 0, 0);

    // Step 6: Add all installed package files
    let troves =
        Trove::list_all(conn).context("Failed to list installed packages")?;
    let mut files_added: u64 = 0;

    for trove in &troves {
        let trove_id = match trove.id {
            Some(id) => id,
            None => {
                debug!("Skipping trove without ID: {}", trove.name);
                continue;
            }
        };

        let files = FileEntry::find_by_trove(conn, trove_id)
            .with_context(|| {
                format!("Failed to get files for trove {}", trove.name)
            })?;

        for file in &files {
            if is_excluded(&file.path) {
                continue;
            }

            // Parse hex digest to bytes -- skip files with invalid hashes
            // (e.g. directories, adopted files with placeholder hashes)
            let digest = match hex_to_digest(&file.sha256_hash) {
                Ok(d) => d,
                Err(_) => {
                    debug!(
                        "Skipping file with invalid digest ({} chars): {}",
                        file.sha256_hash.len(),
                        file.path
                    );
                    continue;
                }
            };

            #[allow(clippy::cast_sign_loss)]
            let permissions = file.permissions as u32;
            #[allow(clippy::cast_sign_loss)]
            let size = file.size as u64;

            // ErofsBuilder handles implicit parent directory creation
            builder.add_file(&file.path, &digest, size, permissions, 0, 0);
            files_added += 1;
        }
    }

    // Step 7: Add root-level symlinks
    for (link, target) in ROOT_SYMLINKS {
        builder.add_symlink(&format!("/{link}"), target, 0o777);
    }

    // Step 8: Build EROFS image
    let image_path = gen_dir.join("root.erofs");
    let file = std::fs::File::create(&image_path).with_context(|| {
        format!("Failed to create EROFS image: {}", image_path.display())
    })?;
    let stats = builder
        .build(BufWriter::new(file))
        .map_err(|e| anyhow!("EROFS build failed: {e}"))?;

    info!(
        "EROFS image built: {} bytes, {} inodes, {} files",
        stats.image_size, stats.inode_count, stats.file_count
    );

    // Step 9: Enable fs-verity on CAS objects (if supported)
    if caps.fsverity {
        debug!("fs-verity supported, enabling on CAS objects");
        let (enabled, already, errors) =
            conary_core::filesystem::fsverity::enable_fsverity_on_cas(&obj_dir);
        info!(
            "fs-verity: {enabled} newly enabled, {already} already enabled, {errors} errors"
        );
    } else {
        debug!(
            "fs-verity not supported on CAS filesystem, skipping"
        );
    }

    // Step 10: Write generation metadata
    #[allow(clippy::cast_possible_wrap)]
    let metadata = GenerationMetadata {
        generation: gen_number,
        format: "composefs".to_string(),
        erofs_size: Some(stats.image_size as i64),
        cas_objects_referenced: Some(stats.file_count as i64),
        fsverity_enabled: caps.fsverity,
        created_at: chrono::Utc::now().to_rfc3339(),
        package_count: troves.len() as i64,
        kernel_version: detect_kernel_version_from_db(conn),
        summary: summary.to_string(),
    };
    metadata
        .write_to(&gen_dir)
        .context("Failed to write generation metadata")?;

    info!(
        "Generation {} built: {} files, {} packages, composefs format",
        gen_number,
        files_added,
        troves.len()
    );

    Ok(gen_number)
}

/// Convert hex string to 32-byte digest
fn hex_to_digest(hex: &str) -> Result<[u8; 32]> {
    if hex.len() != 64 {
        return Err(anyhow!(
            "Expected 64-char hex digest, got {} chars",
            hex.len()
        ));
    }
    let mut digest = [0u8; 32];
    for i in 0..32 {
        digest[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .with_context(|| format!("Invalid hex at position {}", i * 2))?;
    }
    Ok(digest)
}

/// Get kernel version from DB rather than scanning the generation tree
/// (since we no longer have a file tree to scan)
fn detect_kernel_version_from_db(
    conn: &rusqlite::Connection,
) -> Option<String> {
    // Look for kernel package in troves
    let troves = Trove::list_all(conn).ok()?;
    for trove in &troves {
        if trove.name.starts_with("kernel")
            || trove.name.starts_with("linux-image")
        {
            return Some(trove.version.clone());
        }
    }
    // Fall back to running kernel
    std::fs::read_to_string("/proc/version")
        .ok()
        .and_then(|v| v.split_whitespace().nth(2).map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_to_digest_valid() {
        let hex = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let digest = hex_to_digest(hex).unwrap();
        assert_eq!(digest[0], 0xab);
        assert_eq!(digest[1], 0xcd);
        assert_eq!(digest[31], 0x89);
    }

    #[test]
    fn test_hex_to_digest_wrong_length() {
        let result = hex_to_digest("abcd");
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Expected 64-char")
        );
    }

    #[test]
    fn test_hex_to_digest_invalid_chars() {
        let hex = "zzcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let result = hex_to_digest(hex);
        assert!(result.is_err());
    }
}
