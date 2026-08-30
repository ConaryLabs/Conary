// apps/remi/src/server/conversion/benchmark/report/tests.rs
use super::*;
use crate::server::conversion::{
    ConversionBenchmarkAuthority, ConversionBenchmarkCatalogQuery, ConversionBenchmarkEnvironment,
    ConversionBenchmarkSelectionKind, ConversionBenchmarkSetup, ConversionBenchmarkSubject,
    ConversionBenchmarkView, ConversionBenchmarkViews,
};
use crate::server::conversion_timing::{
    ConversionNestedPhase, ConversionPhase, ConversionSourceIdentity, ConversionTimingReport,
};
use conary_core::repository::catalog::CatalogVerificationEvidenceV1;
use std::os::unix::fs::symlink;
use std::time::Duration;

fn valid_process_usage() -> ConversionBenchmarkProcessUsage {
    ConversionBenchmarkProcessUsage {
        rss_start_bytes: 1,
        rss_end_bytes: 1,
        process_lifetime_peak_rss_bytes: 1,
        thread_count_start: 1,
        thread_count_end: 1,
        ..ConversionBenchmarkProcessUsage::default()
    }
}

fn valid_catalog_authority() -> ConversionBenchmarkCatalogAuthority {
    ConversionBenchmarkCatalogAuthority {
        resource_sha256: "a".repeat(64),
        artifact_sha256: "b".repeat(64),
        artifact_bytes: 1,
        logical_digest_sha256: "c".repeat(64),
        portable_manifest_sha256: "d".repeat(64),
        portable_manifest_bytes: portable_manifest_size_v1(1).unwrap(),
        portable_chunk_size: PORTABLE_CHUNK_SIZE_V1,
        portable_chunk_count: 1,
    }
}

fn valid_reopen_vfs() -> PortableVfsMetricsV1 {
    PortableVfsMetricsV1 {
        read_calls: 1,
        requested_bytes: 1,
        returned_bytes: 1,
        chunk_accesses: 1,
        cache_misses: 1,
        carrier_bytes_requested: 1,
        authenticated_chunks: 1,
        authenticated_bytes: 1,
        ..PortableVfsMetricsV1::default()
    }
}

fn valid_query_vfs() -> PortableVfsMetricsV1 {
    PortableVfsMetricsV1 {
        read_calls: 1,
        requested_bytes: 1,
        returned_bytes: 1,
        chunk_accesses: 1,
        cache_hits: 1,
        ..PortableVfsMetricsV1::default()
    }
}

fn valid_catalog_setup() -> ConversionBenchmarkCatalogSetup {
    let authority = valid_catalog_authority();
    let verification = CatalogVerificationEvidenceV1 {
        catalog_bytes: authority.artifact_bytes,
        portable_manifest_validation_passes: 1,
        portable_manifest_validation_bytes: authority.portable_manifest_bytes,
        stored_binding_checks: 1,
        ..CatalogVerificationEvidenceV1::default()
    };
    ConversionBenchmarkCatalogSetup {
        reopen: ConversionBenchmarkCatalogReopen {
            process: valid_process_usage(),
            verification,
            vfs: valid_reopen_vfs(),
        },
        query: ConversionBenchmarkCatalogQuery {
            process: valid_process_usage(),
            vfs: valid_query_vfs(),
        },
    }
}

fn valid_output() -> ConversionBenchmarkOutputProof {
    ConversionBenchmarkOutputProof {
        ccs_sha256: "3".repeat(64),
        ccs_size_bytes: 23,
        transport_sha256: "4".repeat(64),
        signed_object_set_sha256: "5".repeat(64),
        signed_object_count: 2,
        signed_object_bytes: 17,
        independent_transport_reopen_ms: 1,
        independent_transport_reopen_bytes: 23,
        independent_complete_archive_hash_ms: 1,
        independent_complete_archive_hash_bytes: 23,
    }
}

fn valid_timing(cold: bool) -> ConversionTimingReport {
    let mut timing = ConversionTimingReport::new("fedora", "fixture", Some("1"));
    timing.source = Some(ConversionSourceIdentity {
        source_profile: "fedora-44".to_string(),
        version: "1".to_string(),
        architecture: Some("x86_64".to_string()),
        checksum: "repository-checksum".to_string(),
        declared_size_bytes: 5,
    });
    timing.success = true;
    if cold {
        timing.record(
            ConversionPhase::NativeArchiveParseAndSpool,
            Duration::from_millis(7),
        );
        timing.record(
            ConversionPhase::CompleteArchiveCopy,
            Duration::from_millis(1),
        );
        timing.record(
            ConversionPhase::IndependentTransportReopen,
            Duration::from_millis(2),
        );
        timing.record(
            ConversionPhase::CompleteArchiveHash,
            Duration::from_millis(1),
        );
        timing.record_skipped(
            ConversionPhase::DurableCasIngestion,
            DURABLE_CAS_FUSED_SKIP_REASON,
        );
        timing.work.admitted_local_bytes = 5;
        timing.work.repository_checksum_bytes_hashed = 5;
        timing.work.source_artifact_bytes = 5;
        timing.work.source_bytes_hashed = 5;
        timing.work.native_payload_files_spooled = 1;
        timing.work.native_payload_bytes_spooled = 5;
        timing.work.native_payload_declared_bytes = 5;
        timing.work.native_payload_bytes_hashed = 5;
        timing.work.ccs_output_bytes = 23;
        timing.work.ccs_output_bytes_hashed = 23;
        timing.work.independent_transport_reopen_ccs_bytes = 23;
        timing.work.complete_archive_hash_bytes = 23;
        timing.work.complete_archive_copy_bytes = 23;
        timing.work.signed_object_count = 2;
        timing.work.signed_object_bytes = 17;
        timing.work.independent_transport_reopen_object_bytes_hashed = 17;
        timing.work.cas_incoming_bytes_hashed = 17;
        timing.work.cas_persistent_bytes_written = 17;
        timing.work.cas_objects_hashed = 2;
        timing.work.cas_misses = 2;
        timing.work.cas_staged_data_barriers = 1;
        timing.work.cas_canonical_name_barriers = 1;
        timing.total_ms = 11;
    } else {
        timing.record(ConversionPhase::PackageLookup, Duration::from_millis(1));
        timing.record(ConversionPhase::CacheLookup, Duration::from_millis(1));
        timing.record_skipped(
            ConversionPhase::LocalArtifactAdmission,
            "exact cache hit; local source artifact did not need admission",
        );
        for phase in HOT_SKIPPED_PHASES.into_iter().skip(1) {
            timing.record_skipped(phase, "cache hit; phase did not run");
        }
        timing.total_ms = 3;
    }
    timing
}

fn valid_report() -> ConversionBenchmarkReportV5 {
    let output = valid_output();
    ConversionBenchmarkReportV5 {
        schema_version: CONVERSION_BENCHMARK_SCHEMA_V5,
        environment: ConversionBenchmarkEnvironment {
            hardware_label: "fixture".to_string(),
            remi_version: "0".to_string(),
            source_commit: "fixture".to_string(),
            source_dirty: false,
            binary_path: "/fixture/remi".to_string(),
            binary_sha256: "6".repeat(64),
            os_release: "fixture".to_string(),
            kernel_release: "fixture".to_string(),
            cpu_model: "fixture".to_string(),
            logical_cpus: 1,
            memory_bytes: 1,
            roots: Vec::new(),
        },
        authority: ConversionBenchmarkAuthority {
            selection_kind: ConversionBenchmarkSelectionKind::Active,
            source_profile: "fedora-44".to_string(),
            profile: valid_catalog_authority(),
            source: valid_catalog_authority(),
            source_identity: "source".to_string(),
            repository_identity: "repository".to_string(),
            source_parser_config_sha256: "7".repeat(64),
            source_trust_policy_sha256: "8".repeat(64),
            authenticated_metadata_objects: 1,
        },
        setup: ConversionBenchmarkSetup {
            prepare: valid_process_usage(),
            profile: valid_catalog_setup(),
            source: valid_catalog_setup(),
            finalize: valid_process_usage(),
        },
        subject: ConversionBenchmarkSubject {
            package_key_sha256: "1".repeat(64),
            name: "fixture".to_string(),
            version: "1".to_string(),
            package_release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            repository_checksum: "repository-checksum".to_string(),
            source_size_bytes: 5,
            source_artifact_sha256: "2".repeat(64),
        },
        repetitions: vec![
            ConversionBenchmarkEvidence {
                iteration: 1,
                process: valid_process_usage(),
                views: ConversionBenchmarkViews {
                    conversion_core: ConversionBenchmarkView {
                        executed: true,
                        duration_ms: 7,
                    },
                    end_to_end: ConversionBenchmarkView {
                        executed: true,
                        duration_ms: 11,
                    },
                },
                outcome: ConversionBenchmarkOutcome::Success {
                    cache_state: "cold".to_string(),
                    timing: Box::new(valid_timing(true)),
                    output: output.clone(),
                },
            },
            ConversionBenchmarkEvidence {
                iteration: 2,
                process: valid_process_usage(),
                views: ConversionBenchmarkViews {
                    conversion_core: ConversionBenchmarkView {
                        executed: false,
                        duration_ms: 0,
                    },
                    end_to_end: ConversionBenchmarkView {
                        executed: true,
                        duration_ms: 3,
                    },
                },
                outcome: ConversionBenchmarkOutcome::Success {
                    cache_state: "hot".to_string(),
                    timing: Box::new(valid_timing(false)),
                    output,
                },
            },
        ],
    }
}

#[test]
fn accepts_exact_registered_reopen_and_query_counters() {
    validate_catalog_setup(
        &valid_catalog_setup(),
        &valid_catalog_authority(),
        "fixture",
    )
    .unwrap();
}

#[test]
fn accepts_fully_bound_cold_and_hot_repetition_evidence() {
    validate_report(&valid_report()).unwrap();
}

#[test]
fn accepts_one_shared_or_two_concurrent_rpm_digest_passes() {
    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing.work.native_payload_bytes_hashed = 10;

    validate_report(&report).unwrap();
}

#[test]
fn rejects_rpm_projection_spool_reopens_and_impossible_hash_work() {
    for mutate in [
        |work: &mut crate::server::conversion_timing::ConversionWorkMetrics| {
            work.native_payload_spool_file_reopens = 1;
        },
        |work: &mut crate::server::conversion_timing::ConversionWorkMetrics| {
            work.native_payload_spool_bytes_reread = 1;
        },
        |work: &mut crate::server::conversion_timing::ConversionWorkMetrics| {
            work.native_payload_declared_bytes = 4;
        },
        |work: &mut crate::server::conversion_timing::ConversionWorkMetrics| {
            work.native_payload_bytes_hashed = 0;
        },
        |work: &mut crate::server::conversion_timing::ConversionWorkMetrics| {
            work.native_payload_bytes_hashed = 7;
        },
        |work: &mut crate::server::conversion_timing::ConversionWorkMetrics| {
            work.native_payload_bytes_hashed = 15;
        },
    ] {
        let mut report = valid_report();
        let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
        else {
            unreachable!()
        };
        mutate(&mut timing.work);

        assert!(validate_report(&report).is_err());
    }
}

#[test]
fn rejects_legacy_schema_and_converter_reopen_members() {
    let mut legacy_schema = valid_report();
    legacy_schema.schema_version = 4;
    assert!(validate_report(&legacy_schema).is_err());

    let mut missing_reopen_counter = serde_json::to_value(valid_report()).unwrap();
    missing_reopen_counter["repetitions"][0]["outcome"]["timing"]["work"]
        .as_object_mut()
        .unwrap()
        .remove("native_payload_spool_file_reopens");
    let error =
        serde_json::from_value::<ConversionBenchmarkReportV5>(missing_reopen_counter).unwrap_err();
    assert!(error.to_string().contains("missing field"), "{error}");

    let mut legacy_work = serde_json::to_value(valid_report()).unwrap();
    legacy_work["repetitions"][0]["outcome"]["timing"]["work"]
        .as_object_mut()
        .unwrap()
        .insert(
            "immediate_converter_reopen_ccs_bytes".to_string(),
            serde_json::json!(23),
        );
    let error = serde_json::from_value::<ConversionBenchmarkReportV5>(legacy_work).unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");

    let mut legacy_phase = serde_json::to_value(valid_report()).unwrap();
    legacy_phase["repetitions"][0]["outcome"]["timing"]["phases"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "phase": "immediate_converter_reopen",
            "duration_ms": 1,
        }));
    let error = serde_json::from_value::<ConversionBenchmarkReportV5>(legacy_phase).unwrap_err();
    assert!(error.to_string().contains("unknown variant"), "{error}");
}

#[test]
fn raw_report_publication_never_overwrites_existing_targets() {
    let root = tempfile::tempdir().unwrap();
    let regular_path = root.path().join("existing-raw.json");
    fs::write(&regular_path, b"existing raw evidence").unwrap();
    let error = publish_and_reopen_report(&regular_path, &valid_report())
        .expect_err("existing raw report must not be replaced");
    assert!(error.to_string().contains("already exists"));
    assert_eq!(fs::read(&regular_path).unwrap(), b"existing raw evidence");

    let dangling_path = root.path().join("dangling-raw.json");
    symlink("missing-raw-target", &dangling_path).unwrap();
    let error = publish_and_reopen_report(&dangling_path, &valid_report())
        .expect_err("dangling raw target must not be replaced");
    assert!(error.to_string().contains("already exists"));
    assert!(
        fs::symlink_metadata(&dangling_path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(&dangling_path).unwrap(),
        Path::new("missing-raw-target")
    );
}

#[test]
fn terminal_independent_reopen_failure_retains_validated_conversion_evidence() {
    let mut report = valid_report();
    let cold_success = report.repetitions[0].clone();
    let ConversionBenchmarkOutcome::Success {
        cache_state,
        timing,
        ..
    } = cold_success.outcome.clone()
    else {
        unreachable!()
    };
    report.repetitions[0].outcome = ConversionBenchmarkOutcome::IndependentOutputReopenFailure {
        cache_state,
        timing,
        error: "independent benchmark output reopen failed: fixture corruption".to_string(),
    };
    report.repetitions[1] = cold_success;
    report.repetitions[1].iteration = 2;
    let error = validate_report(&report).expect_err("nonterminal failure must be rejected");
    assert_eq!(
        error.to_string(),
        "failed benchmark iteration 1 is not terminal"
    );

    report.repetitions.truncate(1);
    validate_report(&report).expect("terminal reopen failure is valid typed evidence");
    let mut tampered = report.clone();
    tampered.repetitions[0].views.end_to_end.duration_ms += 1;
    assert!(
        validate_report(&tampered).is_err(),
        "retained conversion views must remain bound to timing evidence"
    );
    let mut tampered = report.clone();
    let ConversionBenchmarkOutcome::IndependentOutputReopenFailure { timing, .. } =
        &mut tampered.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing.work.cas_incoming_bytes_hashed -= 1;
    assert!(
        validate_report(&tampered).is_err(),
        "retained cold conversion evidence must prove the fused verified-CAS pass"
    );

    let root = tempfile::tempdir().expect("create report publication root");
    let path = root.path().join(REPORT_FILE_NAME);
    publish_and_reopen_report(&path, &report)
        .expect("publish and strictly reopen terminal reopen-failure report");
    assert!(path.is_file());
}

#[test]
fn terminal_conversion_failure_requires_unexecuted_views() {
    let mut report = valid_report();
    report.repetitions.truncate(1);
    report.repetitions[0].views = ConversionBenchmarkViews {
        conversion_core: ConversionBenchmarkView {
            executed: false,
            duration_ms: 0,
        },
        end_to_end: ConversionBenchmarkView {
            executed: false,
            duration_ms: 0,
        },
    };
    report.repetitions[0].outcome = ConversionBenchmarkOutcome::Failure {
        error: "fixture conversion failure".to_string(),
    };
    validate_report(&report).expect("terminal conversion failure is valid typed evidence");
    report.repetitions[0].views.end_to_end.executed = true;
    assert!(validate_report(&report).is_err());
}

#[test]
fn terminal_hot_reopen_failure_requires_exact_completed_conversion_evidence() {
    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success {
        cache_state,
        timing,
        ..
    } = report.repetitions[1].outcome.clone()
    else {
        unreachable!()
    };
    report.repetitions[1].outcome = ConversionBenchmarkOutcome::IndependentOutputReopenFailure {
        cache_state,
        timing,
        error: "independent benchmark output reopen failed: fixture corruption".to_string(),
    };
    validate_report(&report).expect("terminal hot reopen failure retains exact evidence");

    let mut wrong_cache = report.clone();
    let ConversionBenchmarkOutcome::IndependentOutputReopenFailure { cache_state, .. } =
        &mut wrong_cache.repetitions[1].outcome
    else {
        unreachable!()
    };
    *cache_state = "cold".to_string();
    assert!(validate_report(&wrong_cache).is_err());

    let mut failed_timing = report;
    let ConversionBenchmarkOutcome::IndependentOutputReopenFailure { timing, .. } =
        &mut failed_timing.repetitions[1].outcome
    else {
        unreachable!()
    };
    timing.success = false;
    assert!(validate_report(&failed_timing).is_err());
}

#[test]
fn rejects_views_or_output_bytes_that_contradict_measured_evidence() {
    let mut report = valid_report();
    report.repetitions[0].views.end_to_end.duration_ms += 1;
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    report.repetitions[0].views.conversion_core.duration_ms += 1;
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { output, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    output.independent_complete_archive_hash_bytes -= 1;
    assert!(validate_report(&report).is_err());
}

#[test]
fn rejects_hot_output_or_cold_work_that_changes_exact_ccs_identity() {
    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { output, .. } = &mut report.repetitions[1].outcome
    else {
        unreachable!()
    };
    output.ccs_sha256 = "9".repeat(64);
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing.work.complete_archive_hash_bytes -= 1;
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing.work.ccs_output_bytes_hashed -= 1;
    assert!(validate_report(&report).is_err());
}

#[test]
fn rejects_a_second_cas_pass_or_inconsistent_direct_cas_work() {
    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing.record(
        ConversionPhase::DurableCasIngestion,
        Duration::from_millis(1),
    );
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing.work.cas_incoming_bytes_hashed -= 1;
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing
        .skipped_phases
        .iter_mut()
        .find(|phase| phase.phase == ConversionPhase::DurableCasIngestion)
        .unwrap()
        .reason = "fixture claims an unfused CAS pass".to_string();
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing.record_skipped(
        ConversionPhase::IndependentTransportReopen,
        "fixture contradicts the executed fused phase",
    );
    assert!(validate_report(&report).is_err());
}

#[test]
fn rejects_missing_or_duplicate_fused_independent_reopen() {
    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing
        .phases
        .retain(|phase| phase.phase != ConversionPhase::IndependentTransportReopen);
    let error = validate_report(&report).expect_err("cold evidence must retain the fused reopen");
    assert_eq!(
        error.to_string(),
        "cold benchmark omitted fused independent reopen into durable CAS"
    );

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing.record(
        ConversionPhase::IndependentTransportReopen,
        Duration::from_millis(1),
    );
    let error = validate_report(&report).expect_err("cold evidence must contain one fused reopen");
    assert_eq!(
        error.to_string(),
        "cold benchmark did not record one fused independent reopen into durable CAS"
    );
}

#[test]
fn rejects_missing_duplicate_skipped_or_reordered_archive_pipeline_phases() {
    for omitted in [
        ConversionPhase::CompleteArchiveCopy,
        ConversionPhase::CompleteArchiveHash,
    ] {
        let mut report = valid_report();
        let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
        else {
            unreachable!()
        };
        timing.phases.retain(|phase| phase.phase != omitted);
        assert!(validate_report(&report).is_err());
    }

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing.record(
        ConversionPhase::CompleteArchiveCopy,
        Duration::from_millis(1),
    );
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    timing.record_skipped(
        ConversionPhase::CompleteArchiveHash,
        "fixture contradicts executed canonical hash",
    );
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    let copy = timing
        .phases
        .iter()
        .position(|phase| phase.phase == ConversionPhase::CompleteArchiveCopy)
        .unwrap();
    let hash = timing
        .phases
        .iter()
        .position(|phase| phase.phase == ConversionPhase::CompleteArchiveHash)
        .unwrap();
    timing.phases.swap(copy, hash);
    assert!(validate_report(&report).is_err());
}

#[test]
fn rejects_a_fused_reopen_duration_larger_than_the_total() {
    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
    else {
        unreachable!()
    };
    let invalid_duration = timing.total_ms + 1;
    timing
        .phases
        .iter_mut()
        .find(|phase| phase.phase == ConversionPhase::IndependentTransportReopen)
        .unwrap()
        .duration_ms = invalid_duration;

    let error = validate_report(&report).expect_err("fused phase cannot exceed total wall time");
    assert_eq!(
        error.to_string(),
        "cold benchmark fused independent reopen duration exceeds timing total"
    );
}

#[test]
fn rejects_hot_evidence_outside_the_exact_cache_hit_path() {
    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[1].outcome
    else {
        unreachable!()
    };
    timing.record(ConversionPhase::Download, Duration::from_millis(1));
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[1].outcome
    else {
        unreachable!()
    };
    timing.record_nested(
        ConversionNestedPhase::TemporaryObjectStaging,
        ConversionPhase::PayloadObjectEmission,
        Duration::from_millis(1),
    );
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[1].outcome
    else {
        unreachable!()
    };
    timing.skipped_phases.swap(0, 1);
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[1].outcome
    else {
        unreachable!()
    };
    timing.skipped_phases.pop();
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[1].outcome
    else {
        unreachable!()
    };
    timing.work.downloaded_bytes = 1;
    assert!(validate_report(&report).is_err());

    let mut report = valid_report();
    let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[1].outcome
    else {
        unreachable!()
    };
    timing.work.r2.head_requests = 1;
    assert!(validate_report(&report).is_err());
}

#[test]
fn rejects_full_scan_registered_reopen_counters() {
    let mut setup = valid_catalog_setup();
    setup.reopen.verification.userspace_sha256_passes = 1;
    setup.reopen.verification.userspace_sha256_bytes = 1;
    assert!(validate_catalog_setup(&setup, &valid_catalog_authority(), "fixture").is_err());

    let mut setup = valid_catalog_setup();
    setup.reopen.verification.sqlite_integrity_passes = 1;
    setup.reopen.verification.sqlite_integrity_bytes_covered = 1;
    assert!(validate_catalog_setup(&setup, &valid_catalog_authority(), "fixture").is_err());

    let mut setup = valid_catalog_setup();
    setup.reopen.verification.logical_replay_passes = 1;
    assert!(validate_catalog_setup(&setup, &valid_catalog_authority(), "fixture").is_err());
}

#[test]
fn rejects_wrong_proof_geometry_and_vfs_counters() {
    let mut authority = valid_catalog_authority();
    authority.portable_chunk_count = 2;
    assert!(validate_catalog_authority(&authority, "fixture").is_err());

    let mut setup = valid_catalog_setup();
    setup.reopen.vfs.cache_misses = 0;
    assert!(validate_catalog_setup(&setup, &valid_catalog_authority(), "fixture").is_err());

    let mut setup = valid_catalog_setup();
    setup.query.vfs.integrity_failures = 1;
    assert!(validate_catalog_setup(&setup, &valid_catalog_authority(), "fixture").is_err());
}

#[test]
fn strict_schema_rejects_unknown_top_level_fields() {
    let process = serde_json::to_value(ConversionBenchmarkProcessUsage::default()).unwrap();
    let verification = serde_json::to_value(CatalogVerificationEvidenceV1::default()).unwrap();
    let vfs = serde_json::to_value(PortableVfsMetricsV1::default()).unwrap();
    let reopen = serde_json::json!({
        "process": process,
        "verification": verification,
        "vfs": vfs
    });
    let catalog = serde_json::json!({
        "resource_sha256": "a".repeat(64),
        "artifact_sha256": "b".repeat(64),
        "artifact_bytes": 1,
        "logical_digest_sha256": "c".repeat(64),
        "portable_manifest_sha256": "d".repeat(64),
        "portable_manifest_bytes": 1,
        "portable_chunk_size": PORTABLE_CHUNK_SIZE_V1,
        "portable_chunk_count": 1
    });
    let mut value = serde_json::json!({
        "schema_version": CONVERSION_BENCHMARK_SCHEMA_V5,
        "environment": {
            "hardware_label": "fixture",
            "remi_version": "0",
            "source_commit": "fixture",
            "source_dirty": false,
            "binary_path": "/fixture/remi",
            "binary_sha256": "a".repeat(64),
            "os_release": "fixture",
            "kernel_release": "fixture",
            "cpu_model": "fixture",
            "logical_cpus": 1,
            "memory_bytes": 1,
            "roots": []
        },
        "authority": {
            "selection_kind": "active",
            "source_profile": "fedora-44",
            "profile": catalog.clone(),
            "source": catalog,
            "source_identity": "fixture",
            "repository_identity": "fixture",
            "source_parser_config_sha256": "e".repeat(64),
            "source_trust_policy_sha256": "f".repeat(64),
            "authenticated_metadata_objects": 1
        },
        "subject": {
            "package_key_sha256": "1".repeat(64),
            "name": "fixture",
            "version": "1",
            "package_release": "1",
            "architecture": "x86_64",
            "repository_checksum": "sha256:fixture",
            "source_size_bytes": 1,
            "source_artifact_sha256": "2".repeat(64)
        },
        "setup": {
            "prepare": process.clone(),
            "profile": {
                "reopen": reopen.clone(),
                "query": {
                    "process": process.clone(),
                    "vfs": vfs.clone()
                }
            },
            "source": {
                "reopen": reopen,
                "query": {
                    "process": process.clone(),
                    "vfs": vfs
                }
            },
            "finalize": process
        },
        "repetitions": []
    });
    value
        .as_object_mut()
        .unwrap()
        .insert("legacy_v2_field".to_string(), serde_json::Value::Bool(true));
    let error = serde_json::from_value::<ConversionBenchmarkReportV5>(value).unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");
}
