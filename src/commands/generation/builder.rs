// src/commands/generation/builder.rs
//! Generation builder — creates a new generation tree from current system state

use super::metadata::{
    GenerationMetadata, ROOT_SYMLINKS, detect_kernel_version, generation_path, generations_dir,
    is_excluded,
};
use anyhow::{Context, Result, anyhow};
use conary::db::models::{FileEntry, StateEngine, Trove};
use conary::db::paths::objects_dir;
use conary::filesystem::FileDeployer;
use conary::filesystem::reflink::supports_reflinks;
use tracing::{debug, info, warn};

/// Build a new generation tree from the current system state
///
/// Creates a snapshot of the system state, then deploys all installed package
/// files into a generation directory using reflinks (CoW) where supported.
pub fn build_generation(
    conn: &rusqlite::Connection,
    db_path: &str,
    summary: &str,
) -> Result<i64> {
    // Step 1: Ensure generations base directory exists
    std::fs::create_dir_all(generations_dir())
        .context("Failed to create generations directory")?;

    // Step 2: Check reflink support (warn only, don't error)
    let gen_base = generations_dir();
    if !supports_reflinks(&gen_base) {
        warn!(
            "Filesystem at {} does not support reflinks; files will be copied instead",
            gen_base.display()
        );
    }

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
    std::fs::create_dir_all(&gen_dir)
        .with_context(|| format!("Failed to create generation directory: {}", gen_dir.display()))?;

    // Step 5: Get all installed packages
    let troves = Trove::list_all(conn).context("Failed to list installed packages")?;

    // Step 6: Create file deployer
    let obj_dir = objects_dir(db_path);
    let deployer = FileDeployer::new(&obj_dir, &gen_dir)
        .context("Failed to create file deployer")?;

    // Step 7-9: Deploy files for each trove
    let mut files_deployed: u64 = 0;
    let mut errors: u64 = 0;

    for trove in &troves {
        let trove_id = match trove.id {
            Some(id) => id,
            None => {
                debug!("Skipping trove without ID: {}", trove.name);
                continue;
            }
        };

        let files = FileEntry::find_by_trove(conn, trove_id)
            .with_context(|| format!("Failed to get files for trove {}", trove.name))?;

        for file in &files {
            // Skip excluded paths
            if is_excluded(&file.path) {
                continue;
            }

            #[allow(clippy::cast_sign_loss)]
            let permissions = file.permissions as u32;

            match deployer.deploy_file_reflink(&file.path, &file.sha256_hash, permissions) {
                Ok(()) => {
                    files_deployed += 1;
                }
                Err(e) => {
                    debug!("Failed to deploy {}: {}", file.path, e);
                    errors += 1;
                }
            }
        }
    }

    // Step 10: Create root-level symlinks
    for (link, target) in ROOT_SYMLINKS {
        let link_path = gen_dir.join(link);
        let target_path = gen_dir.join(target);

        if target_path.exists() && !link_path.exists() {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, &link_path).with_context(|| {
                    format!("Failed to create symlink {} -> {}", link, target)
                })?;
            }
        }
    }

    // Step 11: Write generation metadata
    let metadata = GenerationMetadata {
        generation: gen_number,
        created_at: chrono::Utc::now().to_rfc3339(),
        package_count: troves.len() as i64,
        kernel_version: detect_kernel_version(&gen_dir),
        summary: summary.to_string(),
    };
    metadata
        .write_to(&gen_dir)
        .context("Failed to write generation metadata")?;

    // Step 12: Log summary
    info!(
        "Generation {} built: {} files deployed, {} errors, {} packages",
        gen_number,
        files_deployed,
        errors,
        troves.len()
    );

    // Step 13: Return generation number
    Ok(gen_number)
}
