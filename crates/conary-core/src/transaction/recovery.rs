// conary-core/src/transaction/recovery.rs

use super::TransactionEngine;
use crate::Result;
use crate::generation::artifact::{GenerationArtifact, load_generation_artifact};
use rusqlite::Connection;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// EROFS superblock magic number (little-endian u32 at byte offset 1024).
pub(crate) const EROFS_MAGIC: u32 = 0xE0F5_E1E2;

/// Minimum plausible EROFS image size in bytes (one superblock page).
const EROFS_MIN_SIZE: u64 = 4096;

impl TransactionEngine {
    /// Recover from an interrupted transaction.
    ///
    /// Uses a 4-step fallback strategy to restore a bootable system state:
    ///
    /// 1. Read `/conary/current` symlink; if the target generation artifact is
    ///    valid, mount that generation (no rebuild needed).
    /// 2. If the current artifact is missing or invalid, rebuild from DB state.
    /// 3. If the DB is corrupted or has no active state, scan
    ///    `/conary/generations/` by number descending and try each valid
    ///    generation artifact.
    /// 4. If nothing works, return `RecoveryFailed`.
    ///
    /// This replaces the old journal-based roll-forward/roll-back recovery.
    pub fn recover(&self, conn: &Connection) -> Result<()> {
        use crate::generation::mount::current_generation;

        let mut saw_current_generation = false;

        if let Ok(Some(current_num)) = current_generation(&self.config.root) {
            saw_current_generation = true;
            let gen_dir = self.config.generations_dir.join(current_num.to_string());

            match load_generation_artifact_for_number(current_num, &gen_dir) {
                Ok(artifact) => {
                    let (required_verity, expected_digest) = artifact_mount_policy(&artifact);
                    let is_mounted = crate::generation::mount::is_generation_mounted(
                        &self.config.mount_point,
                        &artifact.erofs_path,
                        required_verity,
                        expected_digest.as_deref(),
                    )
                    .unwrap_or(false);

                    if is_mounted {
                        tracing::debug!(
                            "Recovery: generation {} artifact is valid and mounted, no action needed",
                            current_num
                        );
                        return Ok(());
                    }

                    tracing::info!(
                        "Recovery: generation {} has valid artifact but is not mounted, mounting",
                        current_num
                    );
                    return self.mount_artifact_and_link(current_num, &artifact);
                }
                Err(error) => {
                    tracing::warn!(
                        "Recovery: active generation {} failed artifact validation: {}",
                        current_num,
                        error
                    );
                }
            }
        }

        self.rebuild_or_scan(conn, saw_current_generation)
    }

    fn rebuild_or_scan(&self, conn: &Connection, saw_current_generation: bool) -> Result<()> {
        let db_gen: Option<i64> = match conn.query_row(
            "SELECT MAX(state_number) FROM system_states WHERE is_active = 1",
            [],
            |row| row.get(0),
        ) {
            Ok(val) => val,
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => {
                tracing::warn!("Recovery: DB query failed ({}), scanning artifacts", e);
                None
            }
        };

        if let Some(expected) = db_gen {
            tracing::info!(
                "Recovery: DB says generation {} should be active, rebuilding in place",
                expected
            );

            match crate::generation::builder::rebuild_generation_image(
                conn,
                &self.config.generations_dir,
                expected,
                &format!("Recovery rebuild of generation {expected}"),
            ) {
                Ok(_build_result) => {
                    let gen_dir = self.config.generations_dir.join(expected.to_string());
                    let artifact = load_generation_artifact_for_number(expected, &gen_dir)?;
                    return self.mount_artifact_and_link(expected, &artifact);
                }
                Err(e) => {
                    tracing::warn!(
                        "Recovery: rebuild from DB failed ({}), scanning artifacts",
                        e
                    );
                }
            }
        } else {
            if !saw_current_generation && !generations_dir_has_entries(&self.config.generations_dir)
            {
                tracing::debug!(
                    "Recovery: no active generation recorded and no generation images exist yet"
                );
                return Ok(());
            }
            tracing::warn!("Recovery: no active generation in DB, scanning artifacts");
        }

        if let Some(artifact) = self.find_latest_intact_generation() {
            let gen_num = artifact.generation;
            tracing::info!(
                "Recovery: found valid generation artifact for generation {}, mounting",
                gen_num
            );
            return self.mount_artifact_and_link(gen_num, &artifact);
        }

        Err(crate::Error::RecoveryFailed(
            "All recovery strategies exhausted: no valid generation artifact found and \
             DB rebuild failed. Manual intervention required."
                .to_string(),
        ))
    }

    /// Mount a generation by number and update the `/conary/current` symlink.
    ///
    /// Mounts the composefs image at the configured mount point. The `/etc`
    /// overlay is NOT set up here -- it requires distinct lower/target paths
    /// that depend on the calling context (boot vs live-switch). CLI callers
    /// (switch.rs, composefs_ops.rs) handle the /etc overlay themselves.
    fn mount_artifact_and_link(&self, gen_num: i64, artifact: &GenerationArtifact) -> Result<()> {
        let (requested_verity, digest) = artifact_mount_policy(artifact);

        let _mount_outcome =
            crate::generation::mount::mount_generation(&crate::generation::mount::MountOptions {
                image_path: artifact.erofs_path.clone(),
                basedir: artifact.cas_dir.clone(),
                mount_point: self.config.mount_point.clone(),
                verity: requested_verity,
                digest,
                upperdir: None,
                workdir: None,
            })?;

        crate::generation::mount::update_current_symlink(&self.config.root, gen_num)?;

        tracing::info!(
            "Recovery: generation {} mounted and symlink updated",
            gen_num
        );
        Ok(())
    }

    /// Scan the generations directory descending by number and return the
    /// highest generation whose artifact manifest and metadata validate.
    pub(super) fn find_latest_intact_generation(&self) -> Option<GenerationArtifact> {
        if !self.config.generations_dir.exists() {
            return None;
        }

        let mut candidates: Vec<i64> = std::fs::read_dir(&self.config.generations_dir)
            .ok()?
            .flatten()
            .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i64>().ok())
            .collect();

        candidates.sort_unstable_by(|a, b| b.cmp(a));

        for gen_num in candidates {
            let gen_dir = self.config.generations_dir.join(gen_num.to_string());
            match load_generation_artifact_for_number(gen_num, &gen_dir) {
                Ok(artifact) => return Some(artifact),
                Err(error) => {
                    tracing::debug!(
                        "Recovery: generation {} failed artifact validation, skipping: {}",
                        gen_num,
                        error
                    );
                }
            }
        }

        None
    }
}

impl Drop for TransactionEngine {
    fn drop(&mut self) {
        self.release_lock();
    }
}

fn generations_dir_has_entries(path: &Path) -> bool {
    std::fs::read_dir(path)
        .ok()
        .and_then(|mut entries| entries.next())
        .is_some()
}

fn load_generation_artifact_for_number(gen_num: i64, gen_dir: &Path) -> Result<GenerationArtifact> {
    let artifact = load_generation_artifact(gen_dir)?;
    if artifact.generation != gen_num {
        return Err(crate::Error::InvalidPath(format!(
            "generation directory {} contains artifact for generation {}",
            gen_num, artifact.generation
        )));
    }
    Ok(artifact)
}

fn artifact_mount_policy(artifact: &GenerationArtifact) -> (bool, Option<String>) {
    let requested_verity =
        artifact.metadata.fsverity_enabled && artifact.metadata.erofs_verity_digest.is_some();
    let digest = if requested_verity {
        artifact.metadata.erofs_verity_digest.clone()
    } else {
        None
    };
    (requested_verity, digest)
}

/// Return `true` if `path` looks like a valid EROFS image.
///
/// Checks:
/// 1. File exists and is at least `EROFS_MIN_SIZE` bytes.
/// 2. The 4-byte EROFS magic is present at byte offset 1024.
///
/// This is a lightweight sanity check; it does not verify the full image
/// structure or any checksums.
pub fn is_valid_erofs_image(path: &Path) -> bool {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };

    if !meta.is_file() || meta.len() < EROFS_MIN_SIZE {
        return false;
    }

    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    if file.seek(SeekFrom::Start(1024)).is_err() {
        return false;
    }

    let mut buf = [0u8; 4];
    if file.read_exact(&mut buf).is_err() {
        return false;
    }

    u32::from_le_bytes(buf) == EROFS_MAGIC
}
