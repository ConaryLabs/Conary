// crates/conary-core/src/transaction/recovery.rs

use super::TransactionEngine;
use crate::Result;
use crate::db::models::{GenerationPublication, SystemState};
use crate::generation::artifact::{GenerationArtifact, load_generation_artifact_with_verified_cas};
use crate::generation::verity_policy::VerityPolicy;
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
        self.recover_with_policy(
            conn,
            RecoveryScanPolicy::SelectedGenerationOnly,
            &VerityPolicy::Verified,
        )
    }

    /// Recover the selected boot generation, allowing the explicit recovery
    /// command to promote the latest valid artifact when `/conary/current` is
    /// missing or invalid.
    pub fn recover_boot_selection(&self, conn: &Connection) -> Result<()> {
        let cmdline = std::fs::read_to_string("/proc/cmdline")?;
        self.recover_boot_selection_with_verity(conn, &VerityPolicy::from_kernel_cmdline(&cmdline))
    }

    fn recover_boot_selection_with_verity(
        &self,
        conn: &Connection,
        verity: &VerityPolicy,
    ) -> Result<()> {
        // Validate before DB inspection, repair, scanning or mount reuse. An
        // invalid command line is not a damaged artifact that can be bypassed.
        verity.requires_verification()?;
        if let Some(warning) = verity.warning() {
            eprintln!("{warning}");
        }
        self.recover_with_policy(conn, RecoveryScanPolicy::SelectedOrLatestArtifact, verity)
    }

    fn recover_with_policy(
        &self,
        conn: &Connection,
        policy: RecoveryScanPolicy,
        verity: &VerityPolicy,
    ) -> Result<()> {
        use crate::generation::mount::current_generation;

        let pending_debt = pending_publication_debt(conn)?;
        if policy == RecoveryScanPolicy::SelectedOrLatestArtifact && !pending_debt.is_empty() {
            tracing::warn!(
                count = pending_debt.len(),
                "Boot-selection recovery found pending generation publication debt; booting a valid published generation and leaving debt visible for later publish retry"
            );
        }

        if let Some(current_num) = current_generation(&self.config.root)? {
            let gen_dir = self.config.generations_dir.join(current_num.to_string());

            match load_generation_artifact_for_number(current_num, &gen_dir) {
                Ok(artifact) => {
                    if policy == RecoveryScanPolicy::SelectedGenerationOnly {
                        if !pending_debt.is_empty() {
                            tracing::warn!(
                                count = pending_debt.len(),
                                "Recovery found pending generation publication debt; the selected link does not prove configuration projection or generation DB backup completion"
                            );
                        }
                        tracing::debug!(
                            "Recovery: selected generation {} artifact is valid; leaving boot selection unmounted",
                            current_num
                        );
                        return mark_generation_state_active_if_present(conn, current_num);
                    }

                    let (required_verity, expected_digest) =
                        verity.mount_requirements(&artifact.metadata)?;
                    let is_mounted = crate::generation::mount::is_generation_mounted(
                        &self.config.mount_point,
                        &artifact.erofs_path,
                        &artifact.cas_dir,
                        required_verity,
                        expected_digest.as_deref(),
                    )?;

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
                    return self.mount_artifact_and_link(conn, current_num, &artifact, verity);
                }
                Err(error) => {
                    tracing::warn!(
                        "Recovery: active generation {} failed artifact validation: {}",
                        current_num,
                        error
                    );
                }
            }

            return self.rebuild_or_scan(conn, Some(current_num), policy, verity);
        }

        self.rebuild_or_scan(conn, None, policy, verity)
    }

    fn rebuild_or_scan(
        &self,
        conn: &Connection,
        selected_generation: Option<i64>,
        policy: RecoveryScanPolicy,
        verity: &VerityPolicy,
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
                    return self.mount_artifact_and_link(conn, expected, &artifact, verity);
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
            if !generations_dir_has_entries(&self.config.generations_dir)? {
                tracing::debug!("Recovery: no selected generation and no generation images exist");
                return Ok(());
            }
            tracing::warn!("Recovery: no selected generation, scanning artifacts");
        }

        if let Some(artifact) = self.find_latest_intact_generation()? {
            let gen_num = artifact.generation;
            tracing::info!(
                "Recovery: found valid generation artifact for generation {}, mounting",
                gen_num
            );
            return self.mount_artifact_and_link(conn, gen_num, &artifact, verity);
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
        verity: &VerityPolicy,
    ) -> Result<()> {
        let (requested_verity, digest) = verity.mount_requirements(&artifact.metadata)?;

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
    pub(super) fn find_latest_intact_generation(&self) -> Result<Option<GenerationArtifact>> {
        let entries = match std::fs::read_dir(&self.config.generations_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        let mut candidates = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                tracing::debug!(
                    path = %entry.path().display(),
                    "Recovery: ignoring generation entry whose name is not valid UTF-8"
                );
                continue;
            };
            let Ok(generation) = name.parse::<i64>() else {
                tracing::debug!(
                    path = %entry.path().display(),
                    "Recovery: ignoring non-generation directory"
                );
                continue;
            };
            candidates.push(generation);
        }

        candidates.sort_unstable_by(|a, b| b.cmp(a));

        for gen_num in candidates {
            let gen_dir = self.config.generations_dir.join(gen_num.to_string());
            match load_generation_artifact_for_number(gen_num, &gen_dir) {
                Ok(artifact) => return Ok(Some(artifact)),
                Err(error) => {
                    tracing::debug!(
                        "Recovery: generation {} failed artifact validation, skipping: {}",
                        gen_num,
                        error
                    );
                }
            }
        }

        Ok(None)
    }
}

fn pending_publication_debt(conn: &Connection) -> Result<Vec<GenerationPublication>> {
    GenerationPublication::pending_recoverable(conn)
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

fn generations_dir_has_entries(path: &Path) -> Result<bool> {
    let mut entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    match entries.next() {
        Some(entry) => {
            entry?;
            Ok(true)
        }
        None => Ok(false),
    }
}

fn load_generation_artifact_for_number(gen_num: i64, gen_dir: &Path) -> Result<GenerationArtifact> {
    let artifact = load_generation_artifact_with_verified_cas(gen_dir)?;
    if artifact.generation != gen_num {
        return Err(crate::Error::InvalidPath(format!(
            "generation directory {} contains artifact for generation {}",
            gen_num, artifact.generation
        )));
    }
    Ok(artifact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::GenerationPublicationStatus;
    use crate::generation::verity_policy::VerityPolicyError;
    use tempfile::TempDir;

    fn recovery_fixture() -> (TempDir, Connection, TransactionEngine, GenerationArtifact) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        crate::db::init(&db_path).unwrap();
        let conn = crate::db::open(&db_path).unwrap();
        let engine = TransactionEngine::new(super::super::TransactionConfig::new(root)).unwrap();
        crate::transaction::tests::write_valid_generation_artifact(root, 1);
        let artifact = load_generation_artifact_for_number(1, &root.join("generations/1")).unwrap();
        (tmp, conn, engine, artifact)
    }

    #[test]
    fn absent_and_on_reject_plain_selected_or_scanned_generation() {
        for cmdline in [
            "quiet",
            "conary.verity=on",
            "conary.verity=off conary.verity=on",
        ] {
            for selected in [false, true] {
                let (tmp, conn, engine, _) = recovery_fixture();
                if selected {
                    crate::generation::mount::update_current_symlink(tmp.path(), 1).unwrap();
                }
                let error = engine
                    .recover_boot_selection_with_verity(
                        &conn,
                        &VerityPolicy::from_kernel_cmdline(cmdline),
                    )
                    .unwrap_err();
                assert!(
                    matches!(
                        error,
                        crate::Error::BootVerity(VerityPolicyError::MissingGenerationVerity {
                            generation: 1
                        })
                    ),
                    "{error}"
                );
                assert_eq!(tmp.path().join("current").exists(), selected);
            }
        }
    }

    #[test]
    fn verified_recovery_requires_both_verity_flag_and_digest() {
        let (_tmp, _conn, _engine, mut artifact) = recovery_fixture();
        let policy = VerityPolicy::from_kernel_cmdline("conary.verity=on");
        for (enabled, digest) in [(false, Some("abc")), (true, None), (true, Some(""))] {
            artifact.metadata.fsverity_enabled = enabled;
            artifact.metadata.erofs_verity_digest = digest.map(str::to_owned);
            assert_eq!(
                policy.mount_requirements(&artifact.metadata),
                Err(VerityPolicyError::MissingGenerationVerity { generation: 1 })
            );
        }
        artifact.metadata.fsverity_enabled = true;
        artifact.metadata.erofs_verity_digest = Some("abc".into());
        assert_eq!(
            policy.mount_requirements(&artifact.metadata),
            Ok((true, Some("abc".into())))
        );
    }

    #[test]
    fn explicit_off_warns_and_requests_plain_mount_regardless_of_metadata() {
        let (_tmp, _conn, _engine, mut artifact) = recovery_fixture();
        let policy = VerityPolicy::from_kernel_cmdline("conary.verity=on conary.verity=off");
        assert_eq!(
            policy.mount_requirements(&artifact.metadata),
            Ok((false, None))
        );
        artifact.metadata.fsverity_enabled = true;
        artifact.metadata.erofs_verity_digest = Some("abc".into());
        assert_eq!(
            policy.mount_requirements(&artifact.metadata),
            Ok((false, None))
        );
        assert!(
            policy
                .warning()
                .unwrap()
                .contains("disables composefs fs-verity verification")
        );
    }

    #[test]
    fn invalid_or_empty_policy_fails_before_db_repair_scan_or_mount() {
        let tmp = TempDir::new().unwrap();
        let engine =
            TransactionEngine::new(super::super::TransactionConfig::new(tmp.path())).unwrap();
        // No schema: any recovery DB access before policy validation would fail
        // with a database error instead of the typed invalid-argument error.
        let conn = Connection::open_in_memory().unwrap();
        for value in ["", "invalid", "OFF"] {
            let policy = VerityPolicy::from_kernel_cmdline(&format!(
                "conary.verity=off conary.verity={value}"
            ));
            let error = engine
                .recover_boot_selection_with_verity(&conn, &policy)
                .unwrap_err();
            assert!(matches!(
                error,
                crate::Error::BootVerity(VerityPolicyError::InvalidArgument { value: actual })
                    if actual == value
            ));
            assert!(!tmp.path().join("current").exists());
        }
    }

    #[test]
    fn pending_publication_debt_reads_recoverable_rows() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let db_path = root.join("conary.db");
        crate::db::init(&db_path).unwrap();
        let conn = crate::db::open(&db_path).unwrap();

        conn.execute(
            "INSERT INTO generation_publications (
                db_path, runtime_root, phase, status, summary
             ) VALUES (?1, ?2, 'pending_build', 'failed', 'fixture')",
            (db_path.display().to_string(), root.display().to_string()),
        )
        .unwrap();

        let debts = pending_publication_debt(&conn).unwrap();
        assert_eq!(debts.len(), 1);
        assert_eq!(debts[0].status, GenerationPublicationStatus::Failed);
    }
}
