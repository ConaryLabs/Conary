// apps/remi/src/server/scriptlet_evidence_queue/reconciliation/apparmor.rs

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use conary_core::db::models::{
    ConvertedPackage, NewScriptletEvidenceCluster, NewScriptletEvidenceSample,
    ScriptletEvidenceCluster, ScriptletEvidenceClusterReconciliationLink, ScriptletEvidenceSample,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::server::scriptlet_evidence_queue::SCRIPTLET_EVIDENCE_NORMALIZATION_VERSION;
use crate::server::scriptlet_evidence_queue::aggregation::evidence_samples_from_converted;
use crate::server::scriptlet_evidence_queue::types::PendingEvidenceSample;

const APPARMOR_BLOCKED_CLASS: &str = "apparmor";
const APPARMOR_SUPERSEDED_REASON: &str = "normalization-v2:apparmor-path-shape";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppArmorReconciliationResult {
    pub normalization_version: i64,
    pub dry_run: bool,
    pub complete: bool,
    pub next_after_cluster_key: Option<String>,
    pub scanned_cluster_count: i64,
    pub identity_cluster_count: i64,
    pub rekeyed_cluster_count: i64,
    pub split_source_cluster_count: i64,
    pub merged_target_cluster_count: i64,
    pub migrated_source_sample_count: i64,
    pub materialized_target_sample_count: i64,
    pub unresolved_cluster_count: i64,
    pub unresolved_by_reason: BTreeMap<String, i64>,
    pub remaining_cluster_count: i64,
    pub mappings: Vec<AppArmorReconciliationMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppArmorReconciliationMapping {
    pub source_cluster_key: String,
    pub target_cluster_keys: Vec<String>,
    pub source_sample_count: i64,
    pub target_sample_count: i64,
    pub status: String,
    pub unresolved_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct AppArmorCandidate {
    cluster: ScriptletEvidenceCluster,
    samples: Vec<ScriptletEvidenceSample>,
}

#[derive(Debug, Clone)]
struct SampleRematerialization {
    source: ScriptletEvidenceSample,
    targets: Vec<PendingEvidenceSample>,
}

#[derive(Debug, Clone)]
struct ResolvedPlan {
    candidate: AppArmorCandidate,
    target_clusters: BTreeMap<String, NewScriptletEvidenceCluster>,
    samples: Vec<SampleRematerialization>,
}

#[derive(Debug, Clone)]
enum CandidatePlan {
    Resolved(ResolvedPlan),
    Unresolved {
        candidate: AppArmorCandidate,
        reason: String,
    },
}

pub fn reconcile_apparmor_batch(
    db_path: &Path,
    cache_dir: &Path,
    dry_run: bool,
    limit: usize,
    after_cluster_key: Option<&str>,
) -> Result<AppArmorReconciliationResult> {
    let conn = crate::server::open_runtime_db(db_path)?;
    reconcile_apparmor_batch_inner(
        &conn,
        cache_dir,
        dry_run,
        limit.clamp(1, 5000),
        after_cluster_key,
    )
}

fn reconcile_apparmor_batch_inner(
    conn: &Connection,
    cache_dir: &Path,
    dry_run: bool,
    limit: usize,
    after_cluster_key: Option<&str>,
) -> Result<AppArmorReconciliationResult> {
    let candidates = reconciliation_candidates(conn, after_cluster_key.unwrap_or(""), limit)?;
    let complete = candidates.len() < limit;
    let next_after_cluster_key = (!complete)
        .then(|| {
            candidates
                .last()
                .map(|candidate| candidate.cluster.cluster_key.clone())
        })
        .flatten();
    let plans = candidates
        .into_iter()
        .map(|candidate| plan_candidate(conn, cache_dir, candidate))
        .collect::<Result<Vec<_>>>()?;

    let mut identity_cluster_count = 0_i64;
    let mut rekeyed_cluster_count = 0_i64;
    let mut split_source_cluster_count = 0_i64;
    let mut migrated_source_sample_count = 0_i64;
    let mut materialized_target_sample_count = 0_i64;
    let mut unresolved_cluster_count = 0_i64;
    let mut unresolved_by_reason = BTreeMap::new();
    let mut sources_by_target = BTreeMap::<String, BTreeSet<String>>::new();
    let mut mappings = Vec::with_capacity(plans.len());

    for plan in &plans {
        match plan {
            CandidatePlan::Resolved(plan) => {
                let target_cluster_keys = plan.target_cluster_keys();
                let is_identity = target_cluster_keys.len() == 1
                    && target_cluster_keys[0] == plan.candidate.cluster.cluster_key;
                if is_identity {
                    identity_cluster_count += 1;
                } else {
                    rekeyed_cluster_count += 1;
                    migrated_source_sample_count += plan.candidate.samples.len() as i64;
                    materialized_target_sample_count += plan
                        .samples
                        .iter()
                        .map(|sample| sample.targets.len() as i64)
                        .sum::<i64>();
                    if target_cluster_keys.len() > 1 {
                        split_source_cluster_count += 1;
                    }
                    for target_cluster_key in &target_cluster_keys {
                        sources_by_target
                            .entry(target_cluster_key.clone())
                            .or_default()
                            .insert(plan.candidate.cluster.cluster_key.clone());
                    }
                }
                mappings.push(plan.mapping());
            }
            CandidatePlan::Unresolved { candidate, reason } => {
                unresolved_cluster_count += 1;
                *unresolved_by_reason.entry(reason.clone()).or_default() += 1;
                mappings.push(AppArmorReconciliationMapping {
                    source_cluster_key: candidate.cluster.cluster_key.clone(),
                    target_cluster_keys: Vec::new(),
                    source_sample_count: candidate.samples.len() as i64,
                    target_sample_count: 0,
                    status: "unresolved".to_string(),
                    unresolved_reason: Some(reason.clone()),
                });
            }
        }
    }
    let merged_target_cluster_count = sources_by_target
        .values()
        .filter(|sources| sources.len() > 1)
        .count() as i64;

    if !dry_run {
        apply_plans(conn, &plans)?;
    }

    Ok(AppArmorReconciliationResult {
        normalization_version: SCRIPTLET_EVIDENCE_NORMALIZATION_VERSION,
        dry_run,
        complete,
        next_after_cluster_key,
        scanned_cluster_count: plans.len() as i64,
        identity_cluster_count,
        rekeyed_cluster_count,
        split_source_cluster_count,
        merged_target_cluster_count,
        migrated_source_sample_count,
        materialized_target_sample_count,
        unresolved_cluster_count,
        unresolved_by_reason,
        remaining_cluster_count: remaining_cluster_count(conn)?,
        mappings,
    })
}

impl ResolvedPlan {
    fn target_cluster_keys(&self) -> Vec<String> {
        self.target_clusters.keys().cloned().collect()
    }

    fn mapping(&self) -> AppArmorReconciliationMapping {
        let target_cluster_keys = self.target_cluster_keys();
        let status = if target_cluster_keys.len() == 1
            && target_cluster_keys[0] == self.candidate.cluster.cluster_key
        {
            "identity"
        } else if target_cluster_keys.len() > 1 {
            "split"
        } else {
            "rekey"
        };
        AppArmorReconciliationMapping {
            source_cluster_key: self.candidate.cluster.cluster_key.clone(),
            target_cluster_keys,
            source_sample_count: self.candidate.samples.len() as i64,
            target_sample_count: self
                .samples
                .iter()
                .map(|sample| sample.targets.len() as i64)
                .sum(),
            status: status.to_string(),
            unresolved_reason: None,
        }
    }
}

fn reconciliation_candidates(
    conn: &Connection,
    after_cluster_key: &str,
    limit: usize,
) -> Result<Vec<AppArmorCandidate>> {
    let mut stmt = conn.prepare(
        "SELECT cluster_key
         FROM scriptlet_evidence_clusters
         WHERE blocked_class = ?1
           AND normalization_version < ?2
           AND superseded_at IS NULL
           AND cluster_key > ?3
         ORDER BY cluster_key
         LIMIT ?4",
    )?;
    let cluster_keys = stmt
        .query_map(
            params![
                APPARMOR_BLOCKED_CLASS,
                SCRIPTLET_EVIDENCE_NORMALIZATION_VERSION,
                after_cluster_key,
                limit as i64
            ],
            |row| row.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    cluster_keys
        .into_iter()
        .map(|cluster_key| {
            let detail = ScriptletEvidenceCluster::detail(conn, &cluster_key)?
                .with_context(|| format!("load AppArmor queue cluster {cluster_key}"))?;
            Ok(AppArmorCandidate {
                cluster: detail.cluster,
                samples: detail.samples,
            })
        })
        .collect()
}

fn plan_candidate(
    conn: &Connection,
    cache_dir: &Path,
    candidate: AppArmorCandidate,
) -> Result<CandidatePlan> {
    if candidate.samples.is_empty() {
        return Ok(CandidatePlan::Unresolved {
            candidate,
            reason: "source-cluster-has-no-samples".to_string(),
        });
    }

    let mut target_clusters = BTreeMap::<String, NewScriptletEvidenceCluster>::new();
    let mut rematerialized = Vec::with_capacity(candidate.samples.len());
    for source in &candidate.samples {
        if source.cluster_key != candidate.cluster.cluster_key
            || source.distro != candidate.cluster.distro
        {
            return Ok(CandidatePlan::Unresolved {
                candidate,
                reason: "source-cluster-identity-mismatch".to_string(),
            });
        }
        let Some(converted) = ConvertedPackage::find_by_checksum(conn, &source.original_checksum)?
        else {
            return Ok(CandidatePlan::Unresolved {
                candidate,
                reason: "converted-package-missing".to_string(),
            });
        };
        if !source_matches_converted(source, &converted) {
            return Ok(CandidatePlan::Unresolved {
                candidate,
                reason: "converted-package-identity-mismatch".to_string(),
            });
        }

        let current =
            evidence_samples_from_converted(&converted, cache_dir).with_context(|| {
                format!(
                    "rematerialize AppArmor evidence for converted package checksum {}",
                    source.original_checksum
                )
            })?;
        let mut seen_target_keys = BTreeSet::new();
        let targets = current
            .into_iter()
            .filter(|pending| pending_matches_candidate(pending, &candidate.cluster, source))
            .filter(|pending| seen_target_keys.insert(pending.cluster.cluster_key.clone()))
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Ok(CandidatePlan::Unresolved {
                candidate,
                reason: "current-apparmor-observation-missing".to_string(),
            });
        }
        for target in &targets {
            match target_clusters.get(&target.cluster.cluster_key) {
                Some(existing) if existing != &target.cluster => {
                    return Ok(CandidatePlan::Unresolved {
                        candidate,
                        reason: "target-cluster-metadata-conflict".to_string(),
                    });
                }
                Some(_) => {}
                None => {
                    target_clusters
                        .insert(target.cluster.cluster_key.clone(), target.cluster.clone());
                }
            }
        }
        rematerialized.push(SampleRematerialization {
            source: source.clone(),
            targets,
        });
    }

    Ok(CandidatePlan::Resolved(ResolvedPlan {
        candidate,
        target_clusters,
        samples: rematerialized,
    }))
}

fn source_matches_converted(
    source: &ScriptletEvidenceSample,
    converted: &ConvertedPackage,
) -> bool {
    source
        .converted_package_id
        .is_none_or(|id| converted.id == Some(id))
        && converted.package_name.as_deref() == Some(source.package_name.as_str())
        && converted.package_version.as_deref() == Some(source.package_version.as_str())
        && converted.distro.as_deref() == Some(source.distro.as_str())
        && converted.package_architecture == source.package_architecture
}

fn pending_matches_candidate(
    pending: &PendingEvidenceSample,
    candidate: &ScriptletEvidenceCluster,
    source: &ScriptletEvidenceSample,
) -> bool {
    pending.cluster.blocked_class == APPARMOR_BLOCKED_CLASS
        && pending.cluster.schema_version == candidate.schema_version
        && pending.cluster.distro == candidate.distro
        && pending.cluster.target_profile == candidate.target_profile
        && pending.cluster.command == candidate.command
        && pending.cluster.lifecycle_phase == candidate.lifecycle_phase
        && pending.sample.original_checksum == source.original_checksum
        && pending.sample.distro == source.distro
        && pending.sample.package_name == source.package_name
        && pending.sample.package_version == source.package_version
        && pending.sample.package_architecture == source.package_architecture
}

fn apply_plans(conn: &Connection, plans: &[CandidatePlan]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for plan in plans {
        let CandidatePlan::Resolved(plan) = plan else {
            continue;
        };
        apply_resolved_plan(&tx, plan)?;
    }
    tx.commit()?;
    Ok(())
}

fn apply_resolved_plan(conn: &Connection, plan: &ResolvedPlan) -> Result<()> {
    let source_cluster_key = &plan.candidate.cluster.cluster_key;
    let target_cluster_keys = plan.target_cluster_keys();
    let identity = target_cluster_keys.len() == 1 && target_cluster_keys[0] == *source_cluster_key;
    let retains_source = target_cluster_keys
        .iter()
        .any(|target_cluster_key| target_cluster_key == source_cluster_key);

    if identity {
        ensure!(
            cluster_metadata_matches(
                &plan.candidate.cluster,
                plan.target_clusters
                    .get(source_cluster_key)
                    .context("identity AppArmor target cluster missing")?
            ),
            "AppArmor identity reconciliation metadata conflict for {}",
            source_cluster_key
        );
        for sample in &plan.samples {
            update_sample_row(conn, sample.source.id, &sample.targets[0].sample)?;
        }
        conn.execute(
            "UPDATE scriptlet_evidence_clusters
             SET normalization_version = ?2,
                 updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             WHERE cluster_key = ?1
               AND normalization_version < ?2
               AND superseded_at IS NULL",
            params![source_cluster_key, SCRIPTLET_EVIDENCE_NORMALIZATION_VERSION],
        )?;
        return Ok(());
    }

    for target in plan.target_clusters.values() {
        if let Some(existing) = ScriptletEvidenceCluster::find(conn, &target.cluster_key)? {
            ensure!(
                cluster_metadata_matches(&existing, target),
                "AppArmor reconciliation target cluster metadata conflict for {}",
                target.cluster_key
            );
            conn.execute(
                "UPDATE scriptlet_evidence_clusters
                 SET first_seen = MIN(first_seen, ?2),
                     last_seen = MAX(last_seen, ?3),
                     updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 WHERE cluster_key = ?1",
                params![
                    &target.cluster_key,
                    &plan.candidate.cluster.first_seen,
                    &plan.candidate.cluster.last_seen
                ],
            )?;
        } else {
            ScriptletEvidenceCluster::upsert(conn, target)?;
            conn.execute(
                "UPDATE scriptlet_evidence_clusters
                 SET first_seen = ?2,
                     last_seen = ?3,
                     updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 WHERE cluster_key = ?1",
                params![
                    &target.cluster_key,
                    &plan.candidate.cluster.first_seen,
                    &plan.candidate.cluster.last_seen
                ],
            )?;
        }
    }

    for sample in &plan.samples {
        if retains_source
            && sample
                .targets
                .iter()
                .any(|target| target.cluster.cluster_key == *source_cluster_key)
        {
            rematerialize_sample_retaining_source(conn, source_cluster_key, sample)?;
        } else {
            rematerialize_sample(conn, sample)?;
        }
    }

    for target_cluster_key in target_cluster_keys
        .iter()
        .filter(|target_cluster_key| *target_cluster_key != source_cluster_key)
    {
        let migrated_sample_count = plan
            .samples
            .iter()
            .filter(|sample| {
                sample
                    .targets
                    .iter()
                    .any(|target| &target.cluster.cluster_key == target_cluster_key)
            })
            .count() as i64;
        ScriptletEvidenceClusterReconciliationLink::record(
            conn,
            source_cluster_key,
            target_cluster_key,
            SCRIPTLET_EVIDENCE_NORMALIZATION_VERSION,
            migrated_sample_count,
        )?;
    }

    if retains_source {
        conn.execute(
            "UPDATE scriptlet_evidence_clusters
             SET normalization_version = ?2,
                 updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             WHERE cluster_key = ?1
               AND normalization_version < ?2
               AND superseded_at IS NULL",
            params![source_cluster_key, SCRIPTLET_EVIDENCE_NORMALIZATION_VERSION],
        )?;
        return Ok(());
    }

    let superseded_by_cluster_key =
        (target_cluster_keys.len() == 1).then(|| target_cluster_keys[0].as_str());
    conn.execute(
        "UPDATE scriptlet_evidence_clusters
         SET normalization_version = ?2,
             superseded_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
             superseded_reason = ?3,
             superseded_by_cluster_key = ?4,
             updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
         WHERE cluster_key = ?1
           AND normalization_version < ?2
           AND superseded_at IS NULL",
        params![
            source_cluster_key,
            SCRIPTLET_EVIDENCE_NORMALIZATION_VERSION,
            APPARMOR_SUPERSEDED_REASON,
            superseded_by_cluster_key
        ],
    )?;
    Ok(())
}

fn rematerialize_sample_retaining_source(
    conn: &Connection,
    source_cluster_key: &str,
    rematerialized: &SampleRematerialization,
) -> Result<()> {
    let source_target = rematerialized
        .targets
        .iter()
        .find(|target| target.cluster.cluster_key == source_cluster_key)
        .context("retained AppArmor source target missing")?;
    update_sample_row(conn, rematerialized.source.id, &source_target.sample)?;
    for target in &rematerialized.targets {
        if target.cluster.cluster_key != source_cluster_key {
            insert_or_update_sample_preserving_observed_at(
                conn,
                &target.sample,
                &rematerialized.source.observed_at,
            )?;
        }
    }
    Ok(())
}

fn cluster_metadata_matches(
    existing: &ScriptletEvidenceCluster,
    target: &NewScriptletEvidenceCluster,
) -> bool {
    existing.schema_version == target.schema_version
        && existing.distro == target.distro
        && existing.target_profile == target.target_profile
        && existing.blocked_class == target.blocked_class
        && existing.command == target.command
        && existing.normalized_command_shape == target.normalized_command_shape
        && existing.normalized_command_shape_hash == target.normalized_command_shape_hash
        && existing.lifecycle_phase == target.lifecycle_phase
}

fn rematerialize_sample(conn: &Connection, rematerialized: &SampleRematerialization) -> Result<()> {
    let mut reusable_target = None;
    for target in &rematerialized.targets {
        if observation_id(conn, &target.sample)?.is_none() {
            reusable_target = Some(target.cluster.cluster_key.clone());
            break;
        }
    }

    let mut source_row_reused = false;
    for target in &rematerialized.targets {
        if reusable_target.as_deref() == Some(target.cluster.cluster_key.as_str()) {
            update_sample_row(conn, rematerialized.source.id, &target.sample)?;
            source_row_reused = true;
        } else {
            insert_or_update_sample_preserving_observed_at(
                conn,
                &target.sample,
                &rematerialized.source.observed_at,
            )?;
        }
    }
    if !source_row_reused {
        conn.execute(
            "DELETE FROM scriptlet_evidence_cluster_samples WHERE id = ?1",
            [rematerialized.source.id],
        )?;
    }
    Ok(())
}

fn observation_id(conn: &Connection, sample: &NewScriptletEvidenceSample) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT id
         FROM scriptlet_evidence_cluster_samples
         WHERE cluster_key = ?1
           AND original_checksum = ?2
           AND package_name = ?3
           AND package_version = ?4
           AND COALESCE(package_architecture, '') = COALESCE(?5, '')",
        params![
            &sample.cluster_key,
            &sample.original_checksum,
            &sample.package_name,
            &sample.package_version,
            &sample.package_architecture
        ],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn update_sample_row(
    conn: &Connection,
    sample_id: i64,
    sample: &NewScriptletEvidenceSample,
) -> Result<()> {
    conn.execute(
        "UPDATE scriptlet_evidence_cluster_samples
         SET cluster_key = ?2,
             converted_package_id = ?3,
             original_checksum = ?4,
             distro = ?5,
             package_name = ?6,
             package_version = ?7,
             package_architecture = ?8,
             publication_status = ?9,
             scriptlet_fidelity = ?10,
             target_compatibility = ?11,
             reason_codes_json = ?12,
             blocked_classes_json = ?13,
             boot_security_intents_json = ?14,
             security_policy_intents_json = ?15,
             review_artifact_path = ?16,
             review_artifact_stale = ?17,
             evidence_digest = ?18,
             curation_evidence_digest = ?19
         WHERE id = ?1",
        params![
            sample_id,
            &sample.cluster_key,
            sample.converted_package_id,
            &sample.original_checksum,
            &sample.distro,
            &sample.package_name,
            &sample.package_version,
            &sample.package_architecture,
            &sample.publication_status,
            &sample.scriptlet_fidelity,
            &sample.target_compatibility,
            &sample.reason_codes_json,
            &sample.blocked_classes_json,
            &sample.boot_security_intents_json,
            &sample.security_policy_intents_json,
            &sample.review_artifact_path,
            i64::from(sample.review_artifact_stale),
            &sample.evidence_digest,
            &sample.curation_evidence_digest
        ],
    )?;
    Ok(())
}

fn insert_or_update_sample_preserving_observed_at(
    conn: &Connection,
    sample: &NewScriptletEvidenceSample,
    observed_at: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO scriptlet_evidence_cluster_samples (
            cluster_key, converted_package_id, original_checksum, distro,
            package_name, package_version, package_architecture,
            publication_status, scriptlet_fidelity, target_compatibility,
            reason_codes_json, blocked_classes_json, boot_security_intents_json,
            security_policy_intents_json, review_artifact_path,
            review_artifact_stale, evidence_digest, curation_evidence_digest,
            observed_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18, ?19
         )",
        params![
            &sample.cluster_key,
            sample.converted_package_id,
            &sample.original_checksum,
            &sample.distro,
            &sample.package_name,
            &sample.package_version,
            &sample.package_architecture,
            &sample.publication_status,
            &sample.scriptlet_fidelity,
            &sample.target_compatibility,
            &sample.reason_codes_json,
            &sample.blocked_classes_json,
            &sample.boot_security_intents_json,
            &sample.security_policy_intents_json,
            &sample.review_artifact_path,
            i64::from(sample.review_artifact_stale),
            &sample.evidence_digest,
            &sample.curation_evidence_digest,
            observed_at
        ],
    )?;
    let id = observation_id(conn, sample)?.context("materialized AppArmor observation missing")?;
    update_sample_row(conn, id, sample)?;
    conn.execute(
        "UPDATE scriptlet_evidence_cluster_samples
         SET observed_at = MIN(observed_at, ?2)
         WHERE id = ?1",
        params![id, observed_at],
    )?;
    Ok(())
}

fn remaining_cluster_count(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM scriptlet_evidence_clusters
         WHERE blocked_class = ?1
           AND normalization_version < ?2
           AND superseded_at IS NULL",
        params![
            APPARMOR_BLOCKED_CLASS,
            SCRIPTLET_EVIDENCE_NORMALIZATION_VERSION
        ],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests;
