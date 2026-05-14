// conary-core/src/transaction/recovery.rs

use super::TransactionEngine;
use crate::Result;
use crate::db::models::SystemState;
use crate::generation::artifact::{GenerationArtifact, load_generation_artifact_for_activation};
use rusqlite::Connection;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryScanPolicy {
    SelectedGenerationOnly,
    SelectedOrLatestArtifact,
}

impl TransactionEngine {
    /// Recover from an interrupted transaction.
    ///
    /// Uses an ordered recovery strategy to keep the selected boot generation
    /// coherent without doing live-root compatibility mounting during ordinary
    /// transactions:
    ///
    /// 1. Read `/conary/current` symlink; if the target generation artifact is
    ///    valid, mark the selected DB state active and return.
    /// 2. If the selected artifact is missing or invalid, rebuild that selected
    ///    generation from DB state.
    /// 3. For explicit boot-selection recovery, scan `/conary/generations/` by
    ///    number descending and try each valid generation artifact, mounting the
    ///    selected generation only for that explicit recovery command.
    /// 4. If nothing works, return `RecoveryFailed`.
    ///
    /// This replaces the old journal-based roll-forward/roll-back recovery.
    pub fn recover(&self, conn: &Connection) -> Result<()> {
        self.recover_with_policy(conn, RecoveryScanPolicy::SelectedGenerationOnly)
    }

    /// Recover the selected boot generation, allowing the explicit recovery
    /// command to promote the latest valid artifact when `/conary/current` is
    /// missing or invalid.
    pub fn recover_boot_selection(&self, conn: &Connection) -> Result<()> {
        self.recover_with_policy(conn, RecoveryScanPolicy::SelectedOrLatestArtifact)
    }

    fn recover_with_policy(&self, conn: &Connection, policy: RecoveryScanPolicy) -> Result<()> {
        use crate::generation::mount::current_generation;

        if let Ok(Some(current_num)) = current_generation(&self.config.root) {
            let gen_dir = self.config.generations_dir.join(current_num.to_string());

            match load_generation_artifact_for_number(current_num, &gen_dir) {
                Ok(artifact) => {
                    if policy == RecoveryScanPolicy::SelectedGenerationOnly {
                        tracing::debug!(
                            "Recovery: selected generation {} artifact is valid; leaving boot selection unmounted",
                            current_num
                        );
                        return mark_generation_state_active_if_present(conn, current_num);
                    }

                    let (required_verity, expected_digest) = artifact_mount_policy(&artifact);
                    let is_mounted = crate::generation::mount::is_generation_mounted(
                        &self.config.mount_point,
                        &artifact.erofs_path,
                        &artifact.cas_dir,
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
                    return self.mount_artifact_and_link(conn, current_num, &artifact);
                }
                Err(error) => {
                    tracing::warn!(
                        "Recovery: active generation {} failed artifact validation: {}",
                        current_num,
                        error
                    );
                }
            }

            return self.rebuild_or_scan(conn, Some(current_num), policy);
        }

        self.rebuild_or_scan(conn, None, policy)
    }

    fn rebuild_or_scan(
        &self,
        conn: &Connection,
        selected_generation: Option<i64>,
        policy: RecoveryScanPolicy,
    ) -> Result<()> {
        if let Some(expected) = selected_generation {
            tracing::info!(
                "Recovery: selected generation {} needs artifact repair, rebuilding in place",
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
                    if policy == RecoveryScanPolicy::SelectedGenerationOnly {
                        tracing::info!(
                            "Recovery: rebuilt selected generation {} artifact; leaving boot selection unmounted",
                            expected
                        );
                        return mark_generation_state_active_if_present(conn, expected);
                    }
                    return self.mount_artifact_and_link(conn, expected, &artifact);
                }
                Err(e) => {
                    if policy == RecoveryScanPolicy::SelectedGenerationOnly {
                        return Err(crate::Error::RecoveryFailed(format!(
                            "Selected generation {expected} could not be repaired from DB state: {e}"
                        )));
                    }
                    tracing::warn!(
                        "Recovery: rebuild from DB failed ({}), scanning artifacts",
                        e
                    );
                }
            }
        } else {
            if policy == RecoveryScanPolicy::SelectedGenerationOnly {
                tracing::debug!(
                    "Recovery: no selected generation; leaving inactive generation artifacts untouched"
                );
                return Ok(());
            }
            if !generations_dir_has_entries(&self.config.generations_dir) {
                tracing::debug!("Recovery: no selected generation and no generation images exist");
                return Ok(());
            }
            tracing::warn!("Recovery: no selected generation, scanning artifacts");
        }

        if let Some(artifact) = self.find_latest_intact_generation() {
            let gen_num = artifact.generation;
            tracing::info!(
                "Recovery: found valid generation artifact for generation {}, mounting",
                gen_num
            );
            return self.mount_artifact_and_link(conn, gen_num, &artifact);
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
    fn mount_artifact_and_link(
        &self,
        conn: &Connection,
        gen_num: i64,
        artifact: &GenerationArtifact,
    ) -> Result<()> {
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
        mark_generation_state_active_if_present(conn, gen_num)?;

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

fn mark_generation_state_active_if_present(conn: &Connection, gen_num: i64) -> Result<()> {
    match SystemState::find_by_number(conn, gen_num)? {
        Some(state) => state.set_active(conn),
        None => {
            tracing::warn!(
                "Recovery: generation {} has no DB state snapshot to mark active",
                gen_num
            );
            Ok(())
        }
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
    let artifact = load_generation_artifact_for_activation(gen_dir)?;
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
