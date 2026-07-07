// apps/remi/src/server/scriptlet_evidence_queue/storage.rs

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use conary_core::db::models::{
    ConvertedPackage, ScriptletEvidenceCluster, ScriptletEvidenceSample,
};
use rusqlite::Connection;

use super::aggregation::evidence_samples_from_converted;
use super::types::RecordSummary;

pub fn record_converted_package(
    conn: &Connection,
    cache_dir: &Path,
    converted: &ConvertedPackage,
) -> Result<RecordSummary> {
    let samples = evidence_samples_from_converted(converted, cache_dir)?;
    let mut clusters = BTreeSet::new();
    for pending in &samples {
        ScriptletEvidenceCluster::upsert(conn, &pending.cluster)?;
        ScriptletEvidenceSample::upsert(conn, &pending.sample)?;
        clusters.insert(pending.cluster.cluster_key.clone());
    }
    Ok(RecordSummary {
        sample_count: samples.len(),
        cluster_count: clusters.len(),
    })
}
