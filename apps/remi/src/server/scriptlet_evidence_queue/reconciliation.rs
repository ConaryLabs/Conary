// apps/remi/src/server/scriptlet_evidence_queue/reconciliation.rs

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Serialize;

use super::classification::{
    UNKNOWN_COMMAND_NORMALIZATION_VERSION, UnknownCommandDisposition, classify_unknown_command,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnknownCommandReconciliationResult {
    pub normalization_version: i64,
    pub dry_run: bool,
    pub complete: bool,
    pub next_after_cluster_key: Option<String>,
    pub scanned_cluster_count: i64,
    pub retained_signal_cluster_count: i64,
    pub retained_signal_attempt_count: i64,
    pub superseded_noise_cluster_count: i64,
    pub superseded_noise_attempt_count: i64,
    pub superseded_by_reason: BTreeMap<String, i64>,
    pub remaining_cluster_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReconciliationCandidate {
    cluster_key: String,
    command: String,
    attempt_count: i64,
}

pub fn reconcile_unknown_command_batch(
    db_path: &Path,
    dry_run: bool,
    limit: usize,
    after_cluster_key: Option<&str>,
) -> Result<UnknownCommandReconciliationResult> {
    let conn = crate::server::open_runtime_db(db_path)?;
    reconcile_unknown_command_batch_inner(&conn, dry_run, limit.clamp(1, 5000), after_cluster_key)
}

fn reconcile_unknown_command_batch_inner(
    conn: &Connection,
    dry_run: bool,
    limit: usize,
    after_cluster_key: Option<&str>,
) -> Result<UnknownCommandReconciliationResult> {
    let candidates = reconciliation_candidates(conn, after_cluster_key.unwrap_or(""), limit)?;
    let complete = candidates.len() < limit;
    let next_after_cluster_key = (!complete)
        .then(|| {
            candidates
                .last()
                .map(|candidate| candidate.cluster_key.clone())
        })
        .flatten();

    let mut retained_signal_cluster_count = 0_i64;
    let mut retained_signal_attempt_count = 0_i64;
    let mut superseded_noise_cluster_count = 0_i64;
    let mut superseded_noise_attempt_count = 0_i64;
    let mut superseded_by_reason = BTreeMap::new();

    for candidate in &candidates {
        match classify_unknown_command(&candidate.command) {
            UnknownCommandDisposition::Signal => {
                retained_signal_cluster_count += 1;
                retained_signal_attempt_count += candidate.attempt_count;
            }
            UnknownCommandDisposition::StructuralNoise(reason) => {
                superseded_noise_cluster_count += 1;
                superseded_noise_attempt_count += candidate.attempt_count;
                *superseded_by_reason.entry(reason.to_string()).or_default() += 1;
            }
        }
    }

    if !dry_run {
        let tx = conn.unchecked_transaction()?;
        for candidate in &candidates {
            match classify_unknown_command(&candidate.command) {
                UnknownCommandDisposition::Signal => {
                    tx.execute(
                        "UPDATE scriptlet_evidence_clusters
                         SET normalization_version = ?2,
                             updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                         WHERE cluster_key = ?1
                           AND normalization_version < ?2
                           AND superseded_at IS NULL",
                        params![
                            &candidate.cluster_key,
                            UNKNOWN_COMMAND_NORMALIZATION_VERSION
                        ],
                    )?;
                }
                UnknownCommandDisposition::StructuralNoise(reason) => {
                    let superseded_reason = format!(
                        "normalization-v{}:{reason}",
                        UNKNOWN_COMMAND_NORMALIZATION_VERSION
                    );
                    tx.execute(
                        "UPDATE scriptlet_evidence_clusters
                         SET normalization_version = ?2,
                             superseded_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                             superseded_reason = ?3,
                             superseded_by_cluster_key = NULL,
                             updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                         WHERE cluster_key = ?1
                           AND normalization_version < ?2
                           AND superseded_at IS NULL",
                        params![
                            &candidate.cluster_key,
                            UNKNOWN_COMMAND_NORMALIZATION_VERSION,
                            superseded_reason
                        ],
                    )?;
                }
            }
        }
        tx.commit()?;
    }

    Ok(UnknownCommandReconciliationResult {
        normalization_version: UNKNOWN_COMMAND_NORMALIZATION_VERSION,
        dry_run,
        complete,
        next_after_cluster_key,
        scanned_cluster_count: candidates.len() as i64,
        retained_signal_cluster_count,
        retained_signal_attempt_count,
        superseded_noise_cluster_count,
        superseded_noise_attempt_count,
        superseded_by_reason,
        remaining_cluster_count: remaining_cluster_count(conn)?,
    })
}

fn reconciliation_candidates(
    conn: &Connection,
    after_cluster_key: &str,
    limit: usize,
) -> Result<Vec<ReconciliationCandidate>> {
    let mut stmt = conn.prepare(
        "SELECT c.cluster_key, c.command, COUNT(s.id)
         FROM scriptlet_evidence_clusters c
         LEFT JOIN scriptlet_evidence_cluster_samples s ON s.cluster_key = c.cluster_key
         WHERE c.blocked_class = 'unknown-command'
           AND c.normalization_version < ?1
           AND c.superseded_at IS NULL
           AND c.cluster_key > ?2
         GROUP BY c.cluster_key
         ORDER BY c.cluster_key
         LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(
            params![
                UNKNOWN_COMMAND_NORMALIZATION_VERSION,
                after_cluster_key,
                limit as i64
            ],
            |row| {
                Ok(ReconciliationCandidate {
                    cluster_key: row.get(0)?,
                    command: row.get(1)?,
                    attempt_count: row.get(2)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn remaining_cluster_count(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM scriptlet_evidence_clusters
         WHERE blocked_class = 'unknown-command'
           AND normalization_version < ?1
           AND superseded_at IS NULL",
        [UNKNOWN_COMMAND_NORMALIZATION_VERSION],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{
        NewScriptletEvidenceCluster, NewScriptletEvidenceSample, ScriptletEvidenceCluster,
        ScriptletEvidenceClusterListFilter, ScriptletEvidenceNote, ScriptletEvidenceSample,
        ScriptletEvidenceState,
    };

    fn test_db() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();
        (temp, db_path)
    }

    fn seed_cluster(
        conn: &Connection,
        cluster_key: &str,
        blocked_class: &str,
        command: &str,
        package_name: &str,
    ) {
        ScriptletEvidenceCluster::upsert(
            conn,
            &NewScriptletEvidenceCluster {
                cluster_key: cluster_key.to_string(),
                schema_version: 1,
                normalization_version: 1,
                distro: "fedora".to_string(),
                target_profile: "fedora-44".to_string(),
                blocked_class: blocked_class.to_string(),
                command: command.to_string(),
                normalized_command_shape: command.to_string(),
                normalized_command_shape_hash: format!("hash-{cluster_key}"),
                lifecycle_phase: "unknown".to_string(),
            },
        )
        .unwrap();
        ScriptletEvidenceSample::upsert(
            conn,
            &NewScriptletEvidenceSample {
                cluster_key: cluster_key.to_string(),
                converted_package_id: None,
                original_checksum: format!("sha256:{package_name}"),
                distro: "fedora".to_string(),
                package_name: package_name.to_string(),
                package_version: "1.0.0".to_string(),
                package_architecture: Some("x86_64".to_string()),
                publication_status: "private-review".to_string(),
                scriptlet_fidelity: "legacy".to_string(),
                target_compatibility: "private-review".to_string(),
                reason_codes_json: r#"["unknown-command"]"#.to_string(),
                blocked_classes_json: "[]".to_string(),
                boot_security_intents_json: "[]".to_string(),
                security_policy_intents_json: "[]".to_string(),
                review_artifact_path: None,
                review_artifact_stale: false,
                evidence_digest: Some(format!("sha256:evidence-{package_name}")),
                curation_evidence_digest: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn dry_run_reports_impact_without_changing_queue_state() {
        let (_temp, db_path) = test_db();
        let conn = Connection::open(&db_path).unwrap();
        seed_cluster(
            &conn,
            "s1-custom",
            "unknown-command",
            "custom-helper",
            "custom",
        );
        seed_cluster(&conn, "s1-set", "unknown-command", "set", "set-noise");
        seed_cluster(&conn, "s1-true", "unknown-command", "true", "true-noise");

        let result =
            reconcile_unknown_command_batch(&db_path, true, 100, None).expect("dry run succeeds");

        assert!(result.complete);
        assert_eq!(result.scanned_cluster_count, 3);
        assert_eq!(result.retained_signal_cluster_count, 1);
        assert_eq!(result.superseded_noise_cluster_count, 2);
        assert_eq!(result.superseded_noise_attempt_count, 2);
        assert_eq!(result.remaining_cluster_count, 3);
        assert_eq!(result.superseded_by_reason.get("shell-builtin"), Some(&2));
        for key in ["s1-custom", "s1-set", "s1-true"] {
            let cluster = ScriptletEvidenceCluster::find(&conn, key).unwrap().unwrap();
            assert_eq!(cluster.normalization_version, 1);
            assert!(cluster.superseded_at.is_none());
        }
    }

    #[test]
    fn reconciliation_is_bounded_resumable_and_preserves_history() {
        let (_temp, db_path) = test_db();
        let conn = Connection::open(&db_path).unwrap();
        seed_cluster(
            &conn,
            "s1-custom",
            "unknown-command",
            "custom-helper",
            "custom",
        );
        seed_cluster(&conn, "s1-set", "unknown-command", "set", "set-noise");
        seed_cluster(&conn, "s1-true", "unknown-command", "true", "true-noise");
        seed_cluster(&conn, "s1-dracut", "initramfs", "dracut", "kernel");
        ScriptletEvidenceCluster::update_state(
            &conn,
            "s1-set",
            ScriptletEvidenceState::AdapterCandidate,
            "maintainer",
            Some("historical disposition"),
        )
        .unwrap();
        ScriptletEvidenceNote::insert(&conn, "s1-set", "maintainer", "keep this private note")
            .unwrap();

        let first = reconcile_unknown_command_batch(&db_path, false, 2, None).unwrap();
        assert!(!first.complete);
        assert_eq!(first.scanned_cluster_count, 2);
        assert_eq!(first.remaining_cluster_count, 1);

        let second = reconcile_unknown_command_batch(&db_path, false, 2, None).unwrap();
        assert!(second.complete);
        assert_eq!(second.scanned_cluster_count, 1);
        assert_eq!(second.remaining_cluster_count, 0);

        let third = reconcile_unknown_command_batch(&db_path, false, 2, None).unwrap();
        assert!(third.complete);
        assert_eq!(third.scanned_cluster_count, 0);

        let custom = ScriptletEvidenceCluster::find(&conn, "s1-custom")
            .unwrap()
            .unwrap();
        assert_eq!(
            custom.normalization_version,
            UNKNOWN_COMMAND_NORMALIZATION_VERSION
        );
        assert!(custom.superseded_at.is_none());

        let set = ScriptletEvidenceCluster::detail(&conn, "s1-set")
            .unwrap()
            .unwrap();
        assert_eq!(set.cluster.state, ScriptletEvidenceState::AdapterCandidate);
        assert_eq!(set.cluster.normalization_version, 2);
        assert_eq!(
            set.cluster.superseded_reason.as_deref(),
            Some("normalization-v2:shell-builtin")
        );
        assert_eq!(set.samples.len(), 1);
        assert_eq!(set.state_events.len(), 1);
        assert_eq!(set.notes.len(), 1);
        assert_eq!(set.notes[0].body, "keep this private note");
        assert_eq!(set.samples[0].publication_status, "private-review");

        let active =
            ScriptletEvidenceCluster::list(&conn, &ScriptletEvidenceClusterListFilter::default())
                .unwrap();
        assert_eq!(active.len(), 2);
        assert!(
            active
                .iter()
                .any(|row| row.cluster.cluster_key == "s1-custom")
        );
        assert!(
            active
                .iter()
                .any(|row| row.cluster.cluster_key == "s1-dracut")
        );

        let all = ScriptletEvidenceCluster::list(
            &conn,
            &ScriptletEvidenceClusterListFilter {
                include_superseded: true,
                ..ScriptletEvidenceClusterListFilter::default()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 4);
    }
}
