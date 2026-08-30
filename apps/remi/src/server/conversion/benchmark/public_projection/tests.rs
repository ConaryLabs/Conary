// apps/remi/src/server/conversion/benchmark/public_projection/tests.rs
use super::*;
use crate::server::conversion::{
    CONVERSION_BENCHMARK_SCHEMA_V7, ConversionBenchmarkCatalogAuthority,
    ConversionBenchmarkCatalogQuery, ConversionBenchmarkCatalogReopen,
    ConversionBenchmarkCatalogSetup, ConversionBenchmarkEnvironment, ConversionBenchmarkEvidence,
    ConversionBenchmarkRootIdentity, ConversionBenchmarkSelectionKind, ConversionBenchmarkView,
};
use crate::server::conversion_timing::{
    ConversionPhase, ConversionSourceIdentity, ConversionTimingReport, ConversionWorkMetrics,
    DURABLE_CAS_FUSED_SKIP_REASON,
};
use conary_core::repository::catalog::{
    CatalogVerificationEvidenceV1, PORTABLE_CHUNK_SIZE_V1, PortableVfsMetricsV1,
    portable_manifest_size_v1,
};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::Duration;

const PRIVATE_PATH_SENTINEL: &str = "/private/operator/benchmark-sentinel";

fn valid_process_usage() -> ConversionBenchmarkProcessUsage {
    ConversionBenchmarkProcessUsage {
        wall_time_us: 19,
        user_cpu_us: 7,
        system_cpu_us: 3,
        rss_start_bytes: 11,
        rss_end_bytes: 13,
        process_lifetime_peak_rss_bytes: 17,
        minor_faults: 23,
        major_faults: 29,
        block_input_operations: 31,
        block_output_operations: 37,
        logical_read_bytes: 41,
        logical_write_bytes: 43,
        read_syscalls: 47,
        write_syscalls: 53,
        storage_read_bytes: 59,
        storage_write_bytes: 61,
        cancelled_write_bytes: 0,
        voluntary_context_switches: 67,
        involuntary_context_switches: 71,
        thread_count_start: 2,
        thread_count_end: 2,
        runnable_threads_start: 1,
        runnable_threads_end: 1,
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
        read_calls: 2,
        requested_bytes: 2,
        returned_bytes: 2,
        chunk_accesses: 2,
        cache_hits: 1,
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
    ConversionBenchmarkCatalogSetup {
        reopen: ConversionBenchmarkCatalogReopen {
            process: valid_process_usage(),
            verification: CatalogVerificationEvidenceV1 {
                catalog_bytes: authority.artifact_bytes,
                portable_manifest_validation_passes: 1,
                portable_manifest_validation_bytes: authority.portable_manifest_bytes,
                stored_binding_checks: 1,
                ..CatalogVerificationEvidenceV1::default()
            },
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
        independent_transport_reopen_ms: 5,
        independent_transport_reopen_bytes: 23,
        independent_complete_archive_hash_ms: 7,
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
            Duration::from_millis(6),
        );
        timing.record(
            ConversionPhase::PayloadDerivationAndObjectStaging,
            Duration::from_millis(1),
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
            ConversionPhase::Download,
            format!("artifact supplied from {PRIVATE_PATH_SENTINEL}"),
        );
        timing.record_skipped(
            ConversionPhase::DurableCasIngestion,
            DURABLE_CAS_FUSED_SKIP_REASON,
        );
        timing.work.admitted_local_bytes = 5;
        timing.work.repository_checksum_bytes_hashed = 5;
        timing.work.source_artifact_bytes = 5;
        timing.work.source_bytes_hashed = 5;
        timing.work.native_archive_entries_traversed = 79;
        timing.work.native_decompressed_archive_bytes_read = 83;
        timing.work.native_payload_entries = 2;
        timing.work.native_payload_regular_files = 2;
        timing.work.native_payload_files_spooled = 2;
        timing.work.native_payload_bytes_spooled = 20;
        timing.work.native_payload_declared_bytes = 20;
        timing.work.native_payload_bytes_hashed = 20;
        timing.work.payload_files_examined = 2;
        timing.work.payload_chunks_derived = 4;
        timing.work.unique_payload_chunks_derived = 3;
        timing.work.payload_source_files_opened = 2;
        timing.work.payload_source_bytes_read = 20;
        timing.work.payload_chunk_identity_bytes_hashed = 12;
        timing.work.payload_whole_content_bytes_hashed = 20;
        timing.work.payload_crypto_bytes_hashed = 32;
        timing.work.staged_object_bytes_written = 17;
        timing.work.staged_object_deduplicated_bytes = 3;
        timing.work.staged_object_deduplications = 1;
        timing.work.staged_unique_objects = 2;
        timing.work.archive_input_bytes = 20;
        timing.work.archive_compression_input_bytes = 2048;
        timing.work.archive_compression_workers = 1;
        timing.work.archive_compression_block_bytes =
            conary_core::ccs::CcsArchiveCompression::BLOCK_BYTES as u64;
        timing.work.archive_compression_blocks = 1;
        timing.work.archive_compression_buffer_ceiling_bytes =
            conary_core::ccs::CcsArchiveCompression::default()
                .buffer_ceiling_bytes()
                .unwrap();
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
        for phase in [
            ConversionPhase::Download,
            ConversionPhase::Checksum,
            ConversionPhase::NativeArchiveParseAndSpool,
            ConversionPhase::ArtifactIdentityAndAuthorityValidation,
            ConversionPhase::MetadataLifecycleAndAuthorityProjection,
            ConversionPhase::OutputWorkspacePreparation,
            ConversionPhase::PayloadDerivationAndObjectStaging,
            ConversionPhase::ControlProjectionAndSigning,
            ConversionPhase::ArchiveAssemblyAndGzip,
            ConversionPhase::NativeProvenanceProjection,
            ConversionPhase::CompleteArchiveCopy,
            ConversionPhase::IndependentTransportReopen,
            ConversionPhase::CompleteArchiveHash,
            ConversionPhase::DurableCasIngestion,
            ConversionPhase::R2WriteThrough,
            ConversionPhase::DatabasePersistence,
        ] {
            timing.record_skipped(phase, "cache hit; phase did not run");
        }
        timing.total_ms = 3;
    }
    timing
}

fn valid_report() -> ConversionBenchmarkReportV7 {
    let output = valid_output();
    ConversionBenchmarkReportV7 {
        schema_version: CONVERSION_BENCHMARK_SCHEMA_V7,
        environment: ConversionBenchmarkEnvironment {
            hardware_label: "production-xfs".to_string(),
            remi_version: "0.1.0".to_string(),
            source_commit: "e".repeat(40),
            source_dirty: false,
            binary_path: format!("{PRIVATE_PATH_SENTINEL}/bin/remi"),
            binary_sha256: "6".repeat(64),
            os_release: "Production Linux".to_string(),
            kernel_release: "6.12.0-production".to_string(),
            cpu_model: "Fixture CPU".to_string(),
            logical_cpus: 32,
            memory_bytes: 64 * 1024 * 1024 * 1024,
            roots: EXPECTED_ROOT_ROLES
                .into_iter()
                .map(|role| ConversionBenchmarkRootIdentity {
                    role: role.to_string(),
                    path: format!("{PRIVATE_PATH_SENTINEL}/{role}"),
                    device_id: 99,
                    filesystem_type: "0x58465342".to_string(),
                    block_size: 4096,
                })
                .collect(),
        },
        authority: ConversionBenchmarkAuthority {
            selection_kind: ConversionBenchmarkSelectionKind::Active,
            source_profile: "fedora-44".to_string(),
            profile: valid_catalog_authority(),
            source: valid_catalog_authority(),
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-44-everything-x86_64".to_string(),
            source_parser_config_sha256: "7".repeat(64),
            source_trust_policy_sha256: "8".repeat(64),
            authenticated_metadata_objects: 2,
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

fn publish_raw(root: &Path, report: &ConversionBenchmarkReportV7) -> std::path::PathBuf {
    let raw_path = root.join(super::super::REPORT_FILE_NAME);
    super::super::report::publish_and_reopen_report(&raw_path, report)
        .expect("publish raw benchmark fixture");
    raw_path
}

fn assert_no_key(value: &Value, forbidden: &str) {
    match value {
        Value::Array(values) => {
            for value in values {
                assert_no_key(value, forbidden);
            }
        }
        Value::Object(values) => {
            assert!(!values.contains_key(forbidden), "found key {forbidden}");
            for value in values.values() {
                assert_no_key(value, forbidden);
            }
        }
        _ => {}
    }
}

fn assert_publication_rejected(report: &ConversionBenchmarkReportV7) -> String {
    let root = tempfile::tempdir().expect("create rejected-publication root");
    let raw_path = publish_raw(root.path(), report);
    let public_path = root.path().join(PUBLIC_REPORT_FILE_NAME);
    let error = publish_and_reopen_public_report(&raw_path, &public_path)
        .expect_err("invalid raw report must not produce a public projection");
    assert!(!public_path.exists());
    error.to_string()
}

#[test]
fn projection_omits_private_fields_and_preserves_exact_evidence() {
    let root = tempfile::tempdir().expect("create publication root");
    let report = valid_report();
    let raw_path = publish_raw(root.path(), &report);
    let raw_bytes = fs::read(&raw_path).expect("read raw report bytes");
    let public_path = root.path().join(PUBLIC_REPORT_FILE_NAME);

    publish_and_reopen_public_report(&raw_path, &public_path)
        .expect("publish public benchmark projection");

    assert_eq!(
        fs::metadata(&raw_path).unwrap().permissions().mode() & 0o777,
        PRIVATE_FILE_MODE
    );
    assert_eq!(
        fs::metadata(&public_path).unwrap().permissions().mode() & 0o777,
        PRIVATE_FILE_MODE
    );
    let public_bytes = fs::read(&public_path).expect("read public report bytes");
    let public_text = std::str::from_utf8(&public_bytes).unwrap();
    assert!(!public_text.contains(PRIVATE_PATH_SENTINEL));
    let value: Value = serde_json::from_slice(&public_bytes).unwrap();
    for forbidden in ["binary_path", "path", "device_id", "reason", "error"] {
        assert_no_key(&value, forbidden);
    }
    for retired in [
        "nested_phases",
        "payload_reference_bytes_read",
        "payload_reference_bytes_hashed",
        "payload_files_reopened",
        "payload_object_bytes_read",
        "second_pass_chunk_reference_bytes_hashed",
        "second_pass_reconstructed_content_bytes_hashed",
        "temporary_object_incoming_bytes_hashed",
    ] {
        assert_no_key(&value, retired);
    }
    assert_eq!(value["schema_version"], PUBLIC_REPORT_SCHEMA_V5);
    assert_eq!(
        value["raw_report"]["schema_version"],
        CONVERSION_BENCHMARK_SCHEMA_V7
    );
    assert_eq!(
        value["raw_report"]["sha256"],
        conary_core::hash::sha256(&raw_bytes)
    );
    assert_eq!(
        value["raw_report"]["size_bytes"],
        u64::try_from(raw_bytes.len()).unwrap()
    );
    assert_eq!(
        value["authority"],
        serde_json::to_value(&report.authority).unwrap()
    );
    assert_eq!(value["setup"], serde_json::to_value(&report.setup).unwrap());
    assert_eq!(
        value["subject"],
        serde_json::to_value(&report.subject).unwrap()
    );
    for (index, repetition) in report.repetitions.iter().enumerate() {
        let projected = &value["repetitions"][index];
        assert_eq!(
            projected["process"],
            serde_json::to_value(&repetition.process).unwrap()
        );
        assert_eq!(
            projected["views"],
            serde_json::to_value(&repetition.views).unwrap()
        );
        let ConversionBenchmarkOutcome::Success { timing, output, .. } = &repetition.outcome else {
            unreachable!()
        };
        assert_eq!(
            projected["timing"]["phases"],
            serde_json::to_value(&timing.phases).unwrap()
        );
        assert!(projected["timing"].get("nested_phases").is_none());
        assert_eq!(
            projected["timing"]["work"],
            serde_json::to_value(&timing.work).unwrap()
        );
        assert_eq!(projected["output"], serde_json::to_value(output).unwrap());
    }
    assert_eq!(
        value["repetitions"][0]["timing"]["skipped_phases"],
        serde_json::json!(["download", "durable_cas_ingestion"])
    );
    assert_eq!(
        value["repetitions"][1]["timing"]["phases"],
        serde_json::json!([
            {"phase": "package_lookup", "duration_ms": 1},
            {"phase": "cache_lookup", "duration_ms": 1},
        ])
    );
    assert!(
        value["repetitions"][1]["timing"]
            .get("nested_phases")
            .is_none()
    );
    assert_eq!(
        value["repetitions"][1]["timing"]["skipped_phases"],
        serde_json::json!([
            "local_artifact_admission",
            "download",
            "checksum",
            "native_archive_parse_and_spool",
            "artifact_identity_and_authority_validation",
            "metadata_lifecycle_and_authority_projection",
            "output_workspace_preparation",
            "payload_derivation_and_object_staging",
            "control_projection_and_signing",
            "archive_assembly_and_gzip",
            "native_provenance_projection",
            "complete_archive_copy",
            "independent_transport_reopen",
            "complete_archive_hash",
            "durable_cas_ingestion",
            "r2_write_through",
            "database_persistence",
        ])
    );
    assert_eq!(
        value["repetitions"][1]["timing"]["work"],
        serde_json::to_value(ConversionWorkMetrics::default()).unwrap()
    );
    assert_eq!(
        value["environment"]["roots"][0],
        serde_json::json!({
            "role": "source_config",
            "filesystem_type": "0x58465342",
            "block_size": 4096,
        })
    );
}

#[test]
fn public_schema_v5_rejects_legacy_public_and_raw_versions() {
    let report = valid_report();
    let raw_bytes = serde_json::to_vec(&report).unwrap();
    let mut public = project_report(&raw_bytes, &report).unwrap();

    public.schema_version = 4;
    assert!(validate_public_report(&public).is_err());

    public.schema_version = PUBLIC_REPORT_SCHEMA_V5;
    public.raw_report.schema_version = 6;
    assert!(validate_public_report(&public).is_err());
}

#[test]
fn public_schema_v5_requires_exact_payload_source_reopen_work() {
    let report = valid_report();
    let raw_bytes = serde_json::to_vec(&report).unwrap();
    let public = project_report(&raw_bytes, &report).unwrap();
    let mut value = serde_json::to_value(public).unwrap();
    value["repetitions"][0]["timing"]["work"]
        .as_object_mut()
        .unwrap()
        .remove("payload_source_files_reopened");

    let error = serde_json::from_value::<ConversionBenchmarkPublicReportV5>(value).unwrap_err();
    assert!(error.to_string().contains("missing field"), "{error}");
}

#[test]
fn public_schema_v5_rejects_retired_payload_shape() {
    let report = valid_report();
    let raw_bytes = serde_json::to_vec(&report).unwrap();

    let mut legacy_work =
        serde_json::to_value(project_report(&raw_bytes, &report).unwrap()).unwrap();
    legacy_work["repetitions"][0]["timing"]["work"]
        .as_object_mut()
        .unwrap()
        .insert(
            "temporary_object_incoming_bytes_hashed".to_string(),
            serde_json::json!(20),
        );
    let error =
        serde_json::from_value::<ConversionBenchmarkPublicReportV5>(legacy_work).unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");

    let mut legacy_nested =
        serde_json::to_value(project_report(&raw_bytes, &report).unwrap()).unwrap();
    legacy_nested["repetitions"][0]["timing"]
        .as_object_mut()
        .unwrap()
        .insert("nested_phases".to_string(), serde_json::json!([]));
    let error =
        serde_json::from_value::<ConversionBenchmarkPublicReportV5>(legacy_nested).unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");

    let mut legacy_phase =
        serde_json::to_value(project_report(&raw_bytes, &report).unwrap()).unwrap();
    legacy_phase["repetitions"][0]["timing"]["phases"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "phase": "payload_object_emission",
            "duration_ms": 1,
        }));
    let error =
        serde_json::from_value::<ConversionBenchmarkPublicReportV5>(legacy_phase).unwrap_err();
    assert!(error.to_string().contains("unknown variant"), "{error}");
}

#[test]
fn exact_raw_byte_tamper_changes_public_binding() {
    let report = valid_report();
    let first = tempfile::tempdir().unwrap();
    let first_raw = publish_raw(first.path(), &report);
    let first_public = first.path().join(PUBLIC_REPORT_FILE_NAME);
    publish_and_reopen_public_report(&first_raw, &first_public).unwrap();

    let second = tempfile::tempdir().unwrap();
    let second_raw = publish_raw(second.path(), &report);
    let mut raw_output = OpenOptions::new().append(true).open(&second_raw).unwrap();
    raw_output.write_all(b" ").unwrap();
    raw_output.sync_all().unwrap();
    let second_public = second.path().join(PUBLIC_REPORT_FILE_NAME);
    publish_and_reopen_public_report(&second_raw, &second_public).unwrap();

    let first_value: Value = serde_json::from_slice(&fs::read(first_public).unwrap()).unwrap();
    let second_value: Value = serde_json::from_slice(&fs::read(second_public).unwrap()).unwrap();
    assert_ne!(
        first_value["raw_report"]["sha256"],
        second_value["raw_report"]["sha256"]
    );
    assert_eq!(
        second_value["raw_report"]["size_bytes"].as_u64().unwrap(),
        first_value["raw_report"]["size_bytes"].as_u64().unwrap() + 1
    );
}

#[test]
fn raw_report_must_remain_private_and_unaliased() {
    let mode_root = tempfile::tempdir().unwrap();
    let mode_raw = publish_raw(mode_root.path(), &valid_report());
    fs::set_permissions(&mode_raw, fs::Permissions::from_mode(0o640)).unwrap();
    let mode_public = mode_root.path().join(PUBLIC_REPORT_FILE_NAME);
    let error = publish_and_reopen_public_report(&mode_raw, &mode_public)
        .expect_err("non-private raw report must not be projected");
    assert!(error.to_string().contains("private mode 0600"));
    assert!(!mode_public.exists());

    let link_root = tempfile::tempdir().unwrap();
    let link_raw = publish_raw(link_root.path(), &valid_report());
    fs::hard_link(&link_raw, link_root.path().join("raw-alias.json")).unwrap();
    let link_public = link_root.path().join(PUBLIC_REPORT_FILE_NAME);
    let error = publish_and_reopen_public_report(&link_raw, &link_public)
        .expect_err("hard-linked raw report must not be projected");
    assert!(error.to_string().contains("with one link"));
    assert!(!link_public.exists());
}

#[test]
fn public_report_parent_must_be_a_plain_directory() {
    let root = tempfile::tempdir().unwrap();
    let raw_path = publish_raw(root.path(), &valid_report());
    let actual_parent = root.path().join("actual-public-parent");
    fs::create_dir(&actual_parent).unwrap();
    let linked_parent = root.path().join("linked-public-parent");
    symlink(&actual_parent, &linked_parent).unwrap();
    let public_path = linked_parent.join(PUBLIC_REPORT_FILE_NAME);

    let error = publish_and_reopen_public_report(&raw_path, &public_path)
        .expect_err("symlinked public-report parent must be rejected");
    assert!(error.to_string().contains("plain directory"));
    assert!(!actual_parent.join(PUBLIC_REPORT_FILE_NAME).exists());
}

#[test]
fn dirty_unknown_and_failed_reports_are_not_public() {
    let mut dirty = valid_report();
    dirty.environment.source_dirty = true;
    assert!(assert_publication_rejected(&dirty).contains("dirty source identity"));

    let mut unknown = valid_report();
    unknown.environment.source_commit = "unknown".to_string();
    assert!(assert_publication_rejected(&unknown).contains("source commit"));

    let mut unknown_authority = valid_report();
    unknown_authority.authority.source_identity = "unknown".to_string();
    assert!(assert_publication_rejected(&unknown_authority).contains("authority identity"));

    let mut failed = valid_report();
    failed.repetitions.truncate(1);
    failed.repetitions[0].views = ConversionBenchmarkViews {
        conversion_core: ConversionBenchmarkView {
            executed: false,
            duration_ms: 0,
        },
        end_to_end: ConversionBenchmarkView {
            executed: false,
            duration_ms: 0,
        },
    };
    failed.repetitions[0].outcome = ConversionBenchmarkOutcome::Failure {
        error: format!("failed while reading {PRIVATE_PATH_SENTINEL}"),
    };
    assert!(assert_publication_rejected(&failed).contains("not successful"));
}

#[test]
fn unsafe_retained_strings_are_rejected_recursively() {
    for unsafe_value in [
        "/etc/private-host",
        "https://private.example.invalid/evidence",
        "token=private-credential",
        r"C:\private\benchmark",
    ] {
        let mut report = valid_report();
        report.environment.os_release = unsafe_value.to_string();
        assert!(
            assert_publication_rejected(&report).contains("forbidden"),
            "unsafe public string was accepted: {unsafe_value}"
        );
    }
}

#[test]
fn public_report_publication_never_overwrites() {
    let root = tempfile::tempdir().unwrap();
    let raw_path = publish_raw(root.path(), &valid_report());
    let public_path = root.path().join(PUBLIC_REPORT_FILE_NAME);
    fs::write(&public_path, b"existing-public-evidence").unwrap();

    let error = publish_and_reopen_public_report(&raw_path, &public_path)
        .expect_err("existing public report must not be replaced");
    assert!(error.to_string().contains("already exists"));
    assert_eq!(fs::read(&public_path).unwrap(), b"existing-public-evidence");

    let dangling_path = root.path().join("dangling-public.json");
    symlink("missing-public-target", &dangling_path).unwrap();
    let error = publish_and_reopen_public_report(&raw_path, &dangling_path)
        .expect_err("dangling public-report target must not be replaced");
    assert!(error.to_string().contains("already exists"));
    assert!(
        fs::symlink_metadata(&dangling_path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(&dangling_path).unwrap(),
        Path::new("missing-public-target")
    );
}
