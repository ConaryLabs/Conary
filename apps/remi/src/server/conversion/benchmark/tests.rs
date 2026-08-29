// apps/remi/src/server/conversion/benchmark/tests.rs
//! Registered-authority and operator-boundary tests for schema-v3 benchmarks.

use super::*;
use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};

fn successful_timing() -> crate::server::conversion_timing::ConversionTimingReport {
    let mut timing = crate::server::conversion_timing::ConversionTimingReport::new(
        "fedora",
        "benchmark-fixture",
        Some("1"),
    );
    timing.record(
        crate::server::conversion_timing::ConversionPhase::NativeArchiveParseAndSpool,
        std::time::Duration::from_millis(7),
    );
    timing.finish(true);
    timing.total_ms = 11;
    timing
}

#[test]
fn independent_output_reopen_failure_remains_typed_iteration_evidence() {
    let (views, outcome) =
        postprocess_successful_conversion("cold".to_string(), Some(successful_timing()), || {
            Err(anyhow::anyhow!("fixture CCS corruption"))
        })
        .expect("independent reopen failure remains reportable evidence");

    assert!(views.conversion_core.executed);
    assert_eq!(views.conversion_core.duration_ms, 7);
    assert!(views.end_to_end.executed);
    assert_eq!(views.end_to_end.duration_ms, 11);
    let ConversionBenchmarkOutcome::IndependentOutputReopenFailure {
        cache_state,
        timing,
        error,
    } = outcome
    else {
        panic!("independent reopen failure was mislabeled");
    };
    assert_eq!(cache_state, "cold");
    assert_eq!(timing.total_ms, 11);
    assert!(
        error.contains("independent benchmark output reopen failed: fixture CCS corruption"),
        "unexpected error: {error}"
    );
}

#[test]
fn invalid_success_evidence_remains_fatal_without_reopen() {
    let error = postprocess_successful_conversion("cold".to_string(), None, || {
        panic!("missing timing must fail before output reopen")
    })
    .expect_err("missing timing must remain a fatal evidence defect");
    assert!(error.to_string().contains("omitted timing evidence"));

    let error = postprocess_successful_conversion(
        "invented-cache-state".to_string(),
        Some(successful_timing()),
        || panic!("invalid cache state must fail before output reopen"),
    )
    .expect_err("invalid cache state must remain a fatal evidence defect");
    assert!(error.to_string().contains("unknown cache state"));
}

#[test]
fn schema_v3_vfs_query_delta_fails_closed_on_counter_regression() {
    assert!(
        portable_vfs_delta(
            PortableVfsMetricsV1::default(),
            PortableVfsMetricsV1 {
                read_calls: 1,
                ..PortableVfsMetricsV1::default()
            },
        )
        .is_err()
    );
}

#[test]
fn canonical_hashes_are_normalized_to_schema_sha256_values() {
    let digest = "a".repeat(64);
    assert_eq!(
        bare_sha256(&format!("sha256:{digest}"), "fixture").unwrap(),
        digest
    );
    assert!(bare_sha256(&digest, "fixture").is_err());
}

#[test]
fn benchmark_runtime_authority_excludes_an_existing_owner() {
    let root = tempfile::tempdir().unwrap();
    let mut config = RemiConfig::default();
    config.storage.root = root.path().join("runtime");
    let server = config.to_server_config().unwrap();
    std::fs::create_dir_all(server.db_path.parent().unwrap()).unwrap();
    conary_core::db::init(&server.db_path).unwrap();
    let existing = crate::server::acquire_existing_runtime_storage(&config, &server).unwrap();

    let error = acquire_benchmark_runtime_storage(&config, &server).unwrap_err();
    assert!(
        error.to_string().contains("service must be stopped"),
        "{error:#}"
    );
    assert!(format!("{error:#}").contains("already owned"), "{error:#}");

    drop(existing);
    acquire_benchmark_runtime_storage(&config, &server).unwrap();
}

#[test]
fn resolve_subject_records_fresh_registered_profile_and_source_vfs_work() {
    let fixture = ActiveCatalogFixture::new();
    let profile = "fedora-44";
    let revision = fixture.activate(
        profile,
        1,
        vec![package(
            profile,
            "benchmark-fixture",
            "1",
            "1",
            Some("x86_64"),
            4096,
            "benchmark-fixture-artifact",
        )],
    );
    let selection = ProfileRevisionSelection {
        source_profile: profile.to_string(),
        profile_revision_sha256: revision,
    };
    let discovery = fixture
        .authority()
        .open_selected_profile(&selection)
        .expect("open fixture only to discover its derived package key");
    let package_key = ProfileCatalog::new(&discovery)
        .find_package_records_by_name("benchmark-fixture")
        .unwrap()
        .into_iter()
        .next()
        .expect("fixture package")
        .package_key_sha256;
    drop(discovery);

    let authority = CatalogAuthority::from_paths(
        fixture.db_path().to_path_buf(),
        fixture.catalog_dir().to_path_buf(),
        DatabaseWriter::default(),
    );
    let resolved = resolve_subject(&authority, &selection, &package_key).unwrap();

    assert_eq!(resolved.package.package_key_sha256, package_key);
    assert_registered_catalog_work(&resolved.profile_setup);
    assert_registered_catalog_work(&resolved.source_setup);
}

fn assert_registered_catalog_work(setup: &ConversionBenchmarkCatalogSetup) {
    let verification = &setup.reopen.verification;
    assert_eq!(verification.portable_manifest_validation_passes, 1);
    assert!(verification.portable_manifest_validation_bytes > 0);
    assert_eq!(verification.stored_binding_checks, 1);
    assert_eq!(verification.userspace_sha256_passes, 0);
    assert_eq!(verification.userspace_sha256_bytes, 0);
    assert_eq!(verification.sqlite_integrity_passes, 0);
    assert_eq!(verification.sqlite_integrity_bytes_covered, 0);
    assert_eq!(verification.logical_replay_passes, 0);
    assert_eq!(verification.logical_replay_wall_us, 0);

    let reopen = setup.reopen.vfs;
    assert!(reopen.read_calls > 0);
    assert!(reopen.authenticated_chunks > 0);
    assert_eq!(reopen.cache_misses, reopen.authenticated_chunks);
    assert_eq!(reopen.carrier_bytes_requested, reopen.authenticated_bytes);
    assert_eq!(reopen.integrity_failures, 0);

    let query = setup.query.vfs;
    assert!(query.read_calls > 0);
    assert!(query.chunk_accesses > 0);
    assert_eq!(query.chunk_accesses, query.cache_hits + query.cache_misses);
    assert_eq!(query.cache_misses, query.authenticated_chunks);
    assert_eq!(query.carrier_bytes_requested, query.authenticated_bytes);
    assert_eq!(query.integrity_failures, 0);
}

#[test]
fn failed_publication_rollback_removes_only_the_staged_inode() {
    let root = tempfile::tempdir().unwrap();
    let temporary = root.path().join("staged-report");
    let target = root.path().join("published-report");
    fs::write(&temporary, b"owned evidence").unwrap();
    let owned = PublishedInode::from_metadata(&fs::metadata(&temporary).unwrap());
    fs::hard_link(&temporary, &target).unwrap();

    rollback_failed_publication(&target, &temporary, Some(owned));
    assert!(!temporary.exists());
    assert!(!target.exists());

    let raced_temporary = root.path().join("staged-raced-report");
    let raced_target = root.path().join("published-raced-report");
    fs::write(&raced_temporary, b"owned evidence").unwrap();
    let raced_owned = PublishedInode::from_metadata(&fs::metadata(&raced_temporary).unwrap());
    fs::hard_link(&raced_temporary, &raced_target).unwrap();
    fs::remove_file(&raced_target).unwrap();
    fs::write(&raced_target, b"unrelated raced target").unwrap();

    rollback_failed_publication(&raced_target, &raced_temporary, Some(raced_owned));
    assert!(!raced_temporary.exists());
    assert_eq!(
        fs::read(&raced_target).unwrap(),
        b"unrelated raced target",
        "rollback removed an unrelated target that raced into the name"
    );
}
