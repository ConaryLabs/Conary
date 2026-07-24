// apps/remi/src/server/scriptlet_evidence_queue/reconciliation/apparmor/tests.rs

use super::*;
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::ccs::legacy_scriptlets::BootSecurityIntentEvidence;
use conary_core::db::models::{
    NewScriptletEvidenceCluster, ScriptletEvidenceClusterListFilter, ScriptletEvidenceNote,
    ScriptletEvidenceState,
};

use crate::server::scriptlet_evidence_queue::normalization::stable_cluster_key;
use crate::server::scriptlet_evidence_queue::types::ClusterKeyInput;

fn test_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("test.db");
    let conn = Connection::open(&db_path).unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();
    (temp, db_path)
}

fn apparmor_summary(paths: &[&str]) -> ScriptletBundleSummary {
    ScriptletBundleSummary {
        scriptlet_fidelity: "blocked".to_string(),
        target_compatibility: "blocked".to_string(),
        publication_status: "blocked".to_string(),
        evidence_digest: Some("sha256:apparmor-evidence".to_string()),
        blocked_reason_codes: vec!["blocked-class-apparmor".to_string()],
        blocked_classes: vec!["apparmor".to_string()],
        boot_security_intents: paths
            .iter()
            .map(|path| BootSecurityIntentEvidence {
                class_id: "apparmor".to_string(),
                reason_code: "blocked-class-apparmor".to_string(),
                command: "apparmor_parser".to_string(),
                argv: vec!["-r".to_string(), (*path).to_string()],
                phase: Some("postinstall".to_string()),
                lifecycle_paths: vec![(*path).to_string()],
            })
            .collect(),
        ..ScriptletBundleSummary::default()
    }
}

fn seed_converted(
    conn: &Connection,
    name: &str,
    checksum: &str,
    paths: &[&str],
) -> ConvertedPackage {
    let mut converted = ConvertedPackage::new_server(
        "ubuntu".to_string(),
        name.to_string(),
        "1.0.0".to_string(),
        "deb".to_string(),
        checksum.to_string(),
        "partial".to_string(),
        &["sha256:chunk".to_string()],
        42,
        format!("sha256:content-{name}"),
        format!("/cache/{name}.ccs"),
    );
    converted.package_architecture = Some("amd64".to_string());
    converted
        .set_scriptlet_metadata(&apparmor_summary(paths))
        .unwrap();
    converted.insert(conn).unwrap();
    converted
}

fn seed_legacy_cluster(
    conn: &Connection,
    cache_dir: &Path,
    converted: &ConvertedPackage,
    legacy_shape: &str,
) -> (String, Vec<String>, i64) {
    let current = evidence_samples_from_converted(converted, cache_dir).unwrap();
    let target_keys = current
        .iter()
        .map(|pending| pending.cluster.cluster_key.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let stable = stable_cluster_key(&ClusterKeyInput {
        schema_version: 1,
        distro: "ubuntu".to_string(),
        target_profile: "ubuntu-26.04".to_string(),
        blocked_class: APPARMOR_BLOCKED_CLASS.to_string(),
        command: "apparmor_parser".to_string(),
        normalized_command_shape: legacy_shape.to_string(),
        lifecycle_phase: "postinstall".to_string(),
    });
    ScriptletEvidenceCluster::upsert(
        conn,
        &NewScriptletEvidenceCluster {
            cluster_key: stable.cluster_key.clone(),
            schema_version: 1,
            normalization_version: 1,
            distro: "ubuntu".to_string(),
            target_profile: "ubuntu-26.04".to_string(),
            blocked_class: APPARMOR_BLOCKED_CLASS.to_string(),
            command: "apparmor_parser".to_string(),
            normalized_command_shape: legacy_shape.to_string(),
            normalized_command_shape_hash: stable.normalized_command_shape_hash,
            lifecycle_phase: "postinstall".to_string(),
        },
    )
    .unwrap();

    let mut sample = current[0].sample.clone();
    sample.cluster_key = stable.cluster_key.clone();
    sample.boot_security_intents_json = sample
        .boot_security_intents_json
        .replace("/etc/apparmor.d/usr.bin.demo", "<path>")
        .replace("/etc/apparmor.d/usr.bin.second", "<path>");
    let sample_id = ScriptletEvidenceSample::upsert(conn, &sample).unwrap();
    (stable.cluster_key, target_keys, sample_id)
}

#[test]
fn dry_run_reports_key_transformation_without_mutation() {
    let (temp, db_path) = test_db();
    let conn = Connection::open(&db_path).unwrap();
    let converted = seed_converted(
        &conn,
        "apparmor-demo",
        "sha256:apparmor-demo",
        &["/etc/apparmor.d/usr.bin.demo"],
    );
    let (source_key, target_keys, _) =
        seed_legacy_cluster(&conn, temp.path(), &converted, "apparmor_parser -r <path>");

    let result = reconcile_apparmor_batch(&db_path, temp.path(), true, 100, None).unwrap();

    assert_eq!(result.scanned_cluster_count, 1);
    assert_eq!(result.rekeyed_cluster_count, 1);
    assert_eq!(result.remaining_cluster_count, 1);
    assert_eq!(result.mappings[0].source_cluster_key, source_key);
    assert_eq!(result.mappings[0].target_cluster_keys, target_keys);
    let source = ScriptletEvidenceCluster::detail(&conn, &source_key)
        .unwrap()
        .unwrap();
    assert!(source.cluster.superseded_at.is_none());
    assert_eq!(source.samples.len(), 1);
    assert_eq!(
        ConvertedPackage::find_by_checksum(&conn, "sha256:apparmor-demo")
            .unwrap()
            .unwrap()
            .publication_status,
        "blocked"
    );
}

#[test]
fn identity_apply_advances_version_without_supersession_or_links() {
    let (temp, db_path) = test_db();
    let conn = Connection::open(&db_path).unwrap();
    let converted = seed_converted(
        &conn,
        "apparmor-identity",
        "sha256:apparmor-identity",
        &["/tmp/private-profile"],
    );
    let (source_key, target_keys, source_sample_id) =
        seed_legacy_cluster(&conn, temp.path(), &converted, "apparmor_parser -r <path>");
    assert_eq!(target_keys, vec![source_key.clone()]);

    let result = reconcile_apparmor_batch(&db_path, temp.path(), false, 100, None).unwrap();

    assert_eq!(result.identity_cluster_count, 1);
    assert_eq!(result.rekeyed_cluster_count, 0);
    assert_eq!(result.remaining_cluster_count, 0);
    let source = ScriptletEvidenceCluster::detail(&conn, &source_key)
        .unwrap()
        .unwrap();
    assert_eq!(
        source.cluster.normalization_version,
        SCRIPTLET_EVIDENCE_NORMALIZATION_VERSION
    );
    assert!(source.cluster.superseded_at.is_none());
    assert_eq!(source.samples[0].id, source_sample_id);
    assert!(source.outgoing_reconciliation_links.is_empty());
    assert!(source.incoming_reconciliation_links.is_empty());
}

#[test]
fn apply_rekeys_observation_preserves_workflow_history_and_is_idempotent() {
    let (temp, db_path) = test_db();
    let conn = Connection::open(&db_path).unwrap();
    let converted = seed_converted(
        &conn,
        "apparmor-demo",
        "sha256:apparmor-demo",
        &["/etc/apparmor.d/usr.bin.demo"],
    );
    let (source_key, target_keys, source_sample_id) =
        seed_legacy_cluster(&conn, temp.path(), &converted, "apparmor_parser -r <path>");
    ScriptletEvidenceCluster::update_state(
        &conn,
        &source_key,
        ScriptletEvidenceState::AdapterCandidate,
        "maintainer",
        Some("reviewed before normalization"),
    )
    .unwrap();
    ScriptletEvidenceNote::insert(&conn, &source_key, "maintainer", "private rationale").unwrap();
    let source_observed_at = ScriptletEvidenceSample::list_for_cluster(&conn, &source_key).unwrap()
        [0]
    .observed_at
    .clone();

    let result = reconcile_apparmor_batch(&db_path, temp.path(), false, 100, None).unwrap();

    assert_eq!(result.rekeyed_cluster_count, 1);
    assert_eq!(result.remaining_cluster_count, 0);
    let source = ScriptletEvidenceCluster::detail(&conn, &source_key)
        .unwrap()
        .unwrap();
    assert_eq!(
        source.cluster.state,
        ScriptletEvidenceState::AdapterCandidate
    );
    assert_eq!(
        source.cluster.superseded_reason.as_deref(),
        Some(APPARMOR_SUPERSEDED_REASON)
    );
    assert!(source.samples.is_empty());
    assert_eq!(source.state_events.len(), 1);
    assert_eq!(source.notes.len(), 1);
    assert_eq!(source.outgoing_reconciliation_links.len(), 1);

    let target = ScriptletEvidenceCluster::detail(&conn, &target_keys[0])
        .unwrap()
        .unwrap();
    assert_eq!(target.cluster.state, ScriptletEvidenceState::NeedsTriage);
    assert_eq!(target.samples.len(), 1);
    assert_eq!(target.samples[0].id, source_sample_id);
    assert_eq!(target.samples[0].observed_at, source_observed_at);
    assert_eq!(target.incoming_reconciliation_links.len(), 1);
    assert!(
        target.samples[0]
            .boot_security_intents_json
            .contains("/etc/apparmor.d/usr.bin.demo")
    );

    let repeat = reconcile_apparmor_batch(&db_path, temp.path(), false, 100, None).unwrap();
    assert_eq!(repeat.scanned_cluster_count, 0);
    assert_eq!(repeat.remaining_cluster_count, 0);
    assert_eq!(
        ConvertedPackage::find_by_checksum(&conn, "sha256:apparmor-demo")
            .unwrap()
            .unwrap()
            .publication_status,
        "blocked"
    );
}

#[test]
fn bounded_batches_resume_from_the_returned_cursor() {
    let (temp, db_path) = test_db();
    let conn = Connection::open(&db_path).unwrap();
    let converted = seed_converted(
        &conn,
        "apparmor-bounded",
        "sha256:apparmor-bounded",
        &["/etc/apparmor.d/usr.bin.demo"],
    );
    seed_legacy_cluster(
        &conn,
        temp.path(),
        &converted,
        "apparmor_parser --replace <path>",
    );
    seed_legacy_cluster(&conn, temp.path(), &converted, "apparmor_parser -r <path>");

    let first = reconcile_apparmor_batch(&db_path, temp.path(), false, 1, None).unwrap();

    assert_eq!(first.scanned_cluster_count, 1);
    assert!(!first.complete);
    assert_eq!(first.remaining_cluster_count, 1);
    let cursor = first.next_after_cluster_key.as_deref().unwrap();

    let second = reconcile_apparmor_batch(&db_path, temp.path(), false, 1, Some(cursor)).unwrap();

    assert_eq!(second.scanned_cluster_count, 1);
    assert!(!second.complete);
    assert_eq!(second.remaining_cluster_count, 0);
    let final_cursor = second.next_after_cluster_key.as_deref().unwrap();
    let final_batch =
        reconcile_apparmor_batch(&db_path, temp.path(), false, 1, Some(final_cursor)).unwrap();
    assert!(final_batch.complete);
    assert_eq!(final_batch.scanned_cluster_count, 0);
    assert_eq!(final_batch.remaining_cluster_count, 0);
}

#[test]
fn apply_records_split_links_without_duplicate_source_observation() {
    let (temp, db_path) = test_db();
    let conn = Connection::open(&db_path).unwrap();
    let converted = seed_converted(
        &conn,
        "apparmor-split",
        "sha256:apparmor-split",
        &[
            "/etc/apparmor.d/usr.bin.demo",
            "/etc/apparmor.d/usr.bin.second",
        ],
    );
    let (source_key, target_keys, source_sample_id) =
        seed_legacy_cluster(&conn, temp.path(), &converted, "apparmor_parser -r <path>");

    let result = reconcile_apparmor_batch(&db_path, temp.path(), false, 100, None).unwrap();

    assert_eq!(result.split_source_cluster_count, 1);
    assert_eq!(result.materialized_target_sample_count, 2);
    let source = ScriptletEvidenceCluster::detail(&conn, &source_key)
        .unwrap()
        .unwrap();
    assert!(source.cluster.superseded_by_cluster_key.is_none());
    assert!(source.samples.is_empty());
    assert_eq!(source.outgoing_reconciliation_links.len(), 2);

    let target_samples = target_keys
        .iter()
        .flat_map(|target_key| {
            ScriptletEvidenceSample::list_for_cluster(&conn, target_key).unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(target_samples.len(), 2);
    assert!(
        target_samples
            .iter()
            .any(|sample| sample.id == source_sample_id)
    );
    let all = ScriptletEvidenceCluster::list(
        &conn,
        &ScriptletEvidenceClusterListFilter {
            include_superseded: true,
            ..ScriptletEvidenceClusterListFilter::default()
        },
    )
    .unwrap();
    assert_eq!(
        all.iter().map(|cluster| cluster.attempt_count).sum::<i64>(),
        2
    );
}

#[test]
fn split_can_retain_the_source_as_one_current_target() {
    let (temp, db_path) = test_db();
    let conn = Connection::open(&db_path).unwrap();
    let converted = seed_converted(
        &conn,
        "apparmor-mixed-split",
        "sha256:apparmor-mixed-split",
        &["/etc/apparmor.d/usr.bin.demo", "/tmp/private-profile"],
    );
    let (source_key, target_keys, source_sample_id) =
        seed_legacy_cluster(&conn, temp.path(), &converted, "apparmor_parser -r <path>");
    assert!(target_keys.contains(&source_key));
    assert_eq!(target_keys.len(), 2);

    let result = reconcile_apparmor_batch(&db_path, temp.path(), false, 100, None).unwrap();

    assert_eq!(result.split_source_cluster_count, 1);
    assert_eq!(result.remaining_cluster_count, 0);
    let source = ScriptletEvidenceCluster::detail(&conn, &source_key)
        .unwrap()
        .unwrap();
    assert!(source.cluster.superseded_at.is_none());
    assert_eq!(
        source.cluster.normalization_version,
        SCRIPTLET_EVIDENCE_NORMALIZATION_VERSION
    );
    assert_eq!(source.samples.len(), 1);
    assert_eq!(source.samples[0].id, source_sample_id);
    assert_eq!(source.outgoing_reconciliation_links.len(), 1);

    let new_target_key = target_keys
        .iter()
        .find(|target_key| *target_key != &source_key)
        .unwrap();
    let new_target = ScriptletEvidenceCluster::detail(&conn, new_target_key)
        .unwrap()
        .unwrap();
    assert_eq!(new_target.samples.len(), 1);
    assert_eq!(new_target.incoming_reconciliation_links.len(), 1);
}

#[test]
fn split_moves_only_samples_whose_current_identity_changed() {
    let (temp, db_path) = test_db();
    let conn = Connection::open(&db_path).unwrap();
    let approved = seed_converted(
        &conn,
        "apparmor-approved",
        "sha256:apparmor-approved",
        &["/etc/apparmor.d/usr.bin.demo"],
    );
    let unsafe_path = seed_converted(
        &conn,
        "apparmor-unsafe",
        "sha256:apparmor-unsafe",
        &["/tmp/private-profile"],
    );
    let (source_key, approved_targets, approved_sample_id) =
        seed_legacy_cluster(&conn, temp.path(), &approved, "apparmor_parser -r <path>");
    let (same_source_key, unsafe_targets, unsafe_sample_id) = seed_legacy_cluster(
        &conn,
        temp.path(),
        &unsafe_path,
        "apparmor_parser -r <path>",
    );
    assert_eq!(source_key, same_source_key);
    assert_eq!(unsafe_targets, vec![source_key.clone()]);
    assert!(!approved_targets.contains(&source_key));

    let result = reconcile_apparmor_batch(&db_path, temp.path(), false, 100, None).unwrap();

    assert_eq!(result.split_source_cluster_count, 1);
    let source = ScriptletEvidenceCluster::detail(&conn, &source_key)
        .unwrap()
        .unwrap();
    assert_eq!(source.samples.len(), 1);
    assert_eq!(source.samples[0].id, unsafe_sample_id);
    assert_ne!(source.samples[0].id, approved_sample_id);
    assert_eq!(source.outgoing_reconciliation_links.len(), 1);
    assert_eq!(
        source.outgoing_reconciliation_links[0].migrated_sample_count,
        1
    );

    let approved_target = ScriptletEvidenceCluster::detail(&conn, &approved_targets[0])
        .unwrap()
        .unwrap();
    assert_eq!(approved_target.samples.len(), 1);
    assert_eq!(approved_target.samples[0].id, approved_sample_id);
}

#[test]
fn merge_keeps_conflicting_workflow_history_on_linked_sources() {
    let (temp, db_path) = test_db();
    let conn = Connection::open(&db_path).unwrap();
    let first = seed_converted(
        &conn,
        "apparmor-first",
        "sha256:apparmor-first",
        &["/etc/apparmor.d/usr.bin.demo"],
    );
    let second = seed_converted(
        &conn,
        "apparmor-second",
        "sha256:apparmor-second",
        &["/etc/apparmor.d/usr.bin.demo"],
    );
    let (first_source, first_targets, _) = seed_legacy_cluster(
        &conn,
        temp.path(),
        &first,
        "apparmor_parser --replace <path>",
    );
    let (second_source, second_targets, _) =
        seed_legacy_cluster(&conn, temp.path(), &second, "apparmor_parser -r <path>");
    assert_eq!(first_targets, second_targets);
    ScriptletEvidenceCluster::update_state(
        &conn,
        &first_source,
        ScriptletEvidenceState::InDesign,
        "maintainer",
        None,
    )
    .unwrap();
    ScriptletEvidenceCluster::update_state(
        &conn,
        &second_source,
        ScriptletEvidenceState::WontSupport,
        "maintainer",
        Some("different disposition"),
    )
    .unwrap();
    ScriptletEvidenceNote::insert(&conn, &first_source, "maintainer", "first note").unwrap();
    ScriptletEvidenceNote::insert(&conn, &second_source, "maintainer", "second note").unwrap();

    let result = reconcile_apparmor_batch(&db_path, temp.path(), false, 100, None).unwrap();

    assert_eq!(result.merged_target_cluster_count, 1);
    let target = ScriptletEvidenceCluster::detail(&conn, &first_targets[0])
        .unwrap()
        .unwrap();
    assert_eq!(target.cluster.state, ScriptletEvidenceState::NeedsTriage);
    assert_eq!(target.incoming_reconciliation_links.len(), 2);
    let first_source = ScriptletEvidenceCluster::detail(&conn, &first_source)
        .unwrap()
        .unwrap();
    let second_source = ScriptletEvidenceCluster::detail(&conn, &second_source)
        .unwrap()
        .unwrap();
    assert_eq!(first_source.cluster.state, ScriptletEvidenceState::InDesign);
    assert_eq!(
        second_source.cluster.state,
        ScriptletEvidenceState::WontSupport
    );
    assert_eq!(first_source.notes[0].body, "first note");
    assert_eq!(second_source.notes[0].body, "second note");
}

#[test]
fn missing_converted_source_stays_active_and_reports_unresolved() {
    let (temp, db_path) = test_db();
    let conn = Connection::open(&db_path).unwrap();
    let converted = seed_converted(
        &conn,
        "apparmor-missing",
        "sha256:apparmor-missing",
        &["/etc/apparmor.d/usr.bin.demo"],
    );
    let (source_key, _, _) =
        seed_legacy_cluster(&conn, temp.path(), &converted, "apparmor_parser -r <path>");
    conn.execute(
        "UPDATE scriptlet_evidence_cluster_samples
             SET converted_package_id = NULL,
                 original_checksum = 'sha256:not-present'
             WHERE cluster_key = ?1",
        [&source_key],
    )
    .unwrap();

    let result = reconcile_apparmor_batch(&db_path, temp.path(), false, 100, None).unwrap();

    assert_eq!(result.unresolved_cluster_count, 1);
    assert_eq!(
        result.unresolved_by_reason.get("converted-package-missing"),
        Some(&1)
    );
    assert_eq!(result.remaining_cluster_count, 1);
    let source = ScriptletEvidenceCluster::find(&conn, &source_key)
        .unwrap()
        .unwrap();
    assert!(source.superseded_at.is_none());
    assert_eq!(source.normalization_version, 1);
}
