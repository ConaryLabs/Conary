// apps/remi/src/server/scriptlet_evidence_queue/backfill.rs

use std::path::Path;

use anyhow::{Context, Result};
use conary_core::db::models::{ConvertedPackage, ScriptletEvidenceBackfillRun};
use rusqlite::Connection;
use serde::Serialize;

use super::storage::record_converted_package;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackfillBatchResult {
    pub run_id: i64,
    pub status: String,
    pub complete: bool,
    pub last_converted_package_id: i64,
    pub scanned_count: i64,
    pub clustered_count: i64,
}

pub fn run_backfill_batch(
    db_path: &Path,
    cache_dir: &Path,
    limit: usize,
) -> Result<BackfillBatchResult> {
    let conn = crate::server::open_runtime_db(db_path)?;
    let limit = limit.clamp(1, 5000);
    let start_after = latest_backfill_checkpoint(&conn)?;
    let run = ScriptletEvidenceBackfillRun::create(&conn)?;

    match run_backfill_batch_inner(&conn, cache_dir, start_after, run.id, limit) {
        Ok(result) => Ok(result),
        Err(error) => {
            let _ = ScriptletEvidenceBackfillRun::fail(&conn, run.id, &error.to_string());
            Err(error)
        }
    }
}

fn run_backfill_batch_inner(
    conn: &Connection,
    cache_dir: &Path,
    start_after: i64,
    run_id: i64,
    limit: usize,
) -> Result<BackfillBatchResult> {
    let selected = ConvertedPackage::list_after_id(conn, start_after, limit)?;

    let mut scanned_count = 0_i64;
    let mut clustered_count = 0_i64;
    let mut last_converted_package_id = start_after;
    for converted in &selected {
        scanned_count += 1;
        last_converted_package_id = converted.id.unwrap_or(last_converted_package_id);
        if !backfill_candidate(converted) {
            continue;
        }
        let summary = record_converted_package(conn, cache_dir, converted).with_context(|| {
            format!(
                "record scriptlet evidence for converted package id {}",
                converted.id.unwrap_or_default()
            )
        })?;
        clustered_count += summary.sample_count as i64;
    }

    let complete = selected.len() < limit;
    let run = if complete {
        ScriptletEvidenceBackfillRun::complete(
            conn,
            run_id,
            last_converted_package_id,
            scanned_count,
            clustered_count,
        )?
    } else {
        ScriptletEvidenceBackfillRun::update_progress(
            conn,
            run_id,
            last_converted_package_id,
            scanned_count,
            clustered_count,
        )?
    };

    Ok(BackfillBatchResult {
        run_id: run.id,
        status: run.status.as_str().to_string(),
        complete,
        last_converted_package_id: run.last_converted_package_id,
        scanned_count: run.scanned_count,
        clustered_count: run.clustered_count,
    })
}

fn latest_backfill_checkpoint(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(last_converted_package_id), 0)
         FROM scriptlet_evidence_backfill_runs",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn backfill_candidate(converted: &ConvertedPackage) -> bool {
    converted.publication_status != "public" || !converted.scriptlet_summary_for_publication().valid
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::convert::ScriptletBundleSummary;

    fn create_test_db() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();
        (temp, db_path)
    }

    fn seed_converted(
        conn: &rusqlite::Connection,
        name: &str,
        checksum: &str,
        summary: ScriptletBundleSummary,
    ) {
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            name.to_string(),
            "1.0.0".to_string(),
            "rpm".to_string(),
            checksum.to_string(),
            "partial".to_string(),
            &["sha256:chunk".to_string()],
            42,
            format!("sha256:content-{name}"),
            format!("/tmp/{name}.ccs"),
        );
        converted.set_scriptlet_metadata(&summary).unwrap();
        converted.insert(conn).unwrap();
    }

    fn blocked_summary(class: &str) -> ScriptletBundleSummary {
        ScriptletBundleSummary {
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            publication_status: "blocked".to_string(),
            blocked_reason_codes: vec![class.to_string()],
            blocked_classes: vec![class.to_string()],
            ..ScriptletBundleSummary::default()
        }
    }

    #[test]
    fn scriptlet_evidence_backfill_batches_resume_without_duplicate_samples() {
        let (temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        seed_converted(&conn, "first", "sha256:first", blocked_summary("network"));
        seed_converted(
            &conn,
            "second",
            "sha256:second",
            blocked_summary("initramfs"),
        );

        let first = run_backfill_batch(&db_path, temp.path(), 1).unwrap();
        assert!(!first.complete);
        assert_eq!(first.scanned_count, 1);

        let second = run_backfill_batch(&db_path, temp.path(), 1).unwrap();
        assert!(!second.complete);
        assert_eq!(second.scanned_count, 1);

        let third = run_backfill_batch(&db_path, temp.path(), 1).unwrap();
        assert!(third.complete);
        assert_eq!(third.scanned_count, 0);

        let sample_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM scriptlet_evidence_cluster_samples",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sample_count, 2);
    }

    #[test]
    fn scriptlet_evidence_backfill_limit_bounds_id_ordered_sql_scan() {
        let (temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        seed_converted(
            &conn,
            "public-ready",
            "sha256:public-ready",
            ScriptletBundleSummary::default(),
        );
        seed_converted(
            &conn,
            "blocked",
            "sha256:blocked",
            blocked_summary("initramfs"),
        );

        let first = run_backfill_batch(&db_path, temp.path(), 1).unwrap();
        assert!(!first.complete);
        assert_eq!(first.scanned_count, 1);
        assert_eq!(first.clustered_count, 0);
        assert_eq!(first.last_converted_package_id, 1);

        let sample_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM scriptlet_evidence_cluster_samples",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sample_count, 0);

        let second = run_backfill_batch(&db_path, temp.path(), 1).unwrap();
        assert!(!second.complete);
        assert_eq!(second.scanned_count, 1);
        assert_eq!(second.clustered_count, 1);
        assert_eq!(second.last_converted_package_id, 2);
    }

    #[test]
    fn scriptlet_evidence_backfill_skips_public_rows_and_includes_malformed_summaries() {
        let (temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        seed_converted(
            &conn,
            "public-ready",
            "sha256:public-ready",
            ScriptletBundleSummary::default(),
        );
        let mut malformed = ConvertedPackage::new_server(
            "fedora".to_string(),
            "malformed".to_string(),
            "1.0.0".to_string(),
            "rpm".to_string(),
            "sha256:malformed".to_string(),
            "partial".to_string(),
            &["sha256:chunk".to_string()],
            42,
            "sha256:content-malformed".to_string(),
            "/tmp/malformed.ccs".to_string(),
        );
        malformed.scriptlet_summary_json = "{not-json".to_string();
        malformed.insert(&conn).unwrap();

        let result = run_backfill_batch(&db_path, temp.path(), 100).unwrap();
        assert!(result.complete);
        assert_eq!(result.scanned_count, 2);
        assert_eq!(result.clustered_count, 1);

        let sample_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM scriptlet_evidence_cluster_samples",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sample_count, 1);
    }
}
