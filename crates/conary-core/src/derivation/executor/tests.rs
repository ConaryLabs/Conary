// crates/conary-core/src/derivation/executor/tests.rs

use super::*;
use crate::db::schema::ensure_current;
use crate::derivation::test_helpers::helpers::test_cas;
use tempfile::TempDir;

struct StdinTerminalOverrideGuard;

impl StdinTerminalOverrideGuard {
    fn non_tty() -> Self {
        set_stdin_is_terminal_override_for_tests(Some(false));
        Self
    }
}

impl Drop for StdinTerminalOverrideGuard {
    fn drop(&mut self) {
        set_stdin_is_terminal_override_for_tests(None);
    }
}

/// Create a minimal recipe for testing via TOML deserialization.
fn test_recipe(name: &str, version: &str) -> Recipe {
    let toml_str = format!(
        r#"
[package]
name = "{name}"
version = "{version}"

[source]
archive = "https://example.com/{name}-{version}.tar.gz"
checksum = "sha256:abc123"

[build]
make = "make"
install = "make install"
"#
    );
    toml::from_str(&toml_str).expect("test recipe must parse")
}

/// Set up an in-memory database with the current schema.
fn setup_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    conn
}

#[test]
fn cache_hit_returns_existing_record() {
    let tmp = TempDir::new().unwrap();
    let cas = test_cas(tmp.path());
    let conn = setup_db();

    let recipe = test_recipe("glibc", "2.39");
    let dep_ids = BTreeMap::new();
    let build_env_hash = "env_hash_abc";
    let target_triple = "x86_64-unknown-linux-gnu";

    // Compute the derivation ID that execute() will produce.
    let src_hash = recipe_hash::source_hash(&recipe);
    let script_hash = recipe_hash::build_script_hash(&recipe);
    let inputs = DerivationInputs {
        source_hash: src_hash,
        build_script_hash: script_hash,
        dependency_ids: dep_ids.clone(),
        build_env_hash: build_env_hash.to_owned(),
        target_triple: target_triple.to_owned(),
        build_options: BTreeMap::new(),
    };
    let expected_id = DerivationId::compute(&inputs).unwrap();

    // Pre-insert a record so execute() finds it.
    let output_hash = "a".repeat(64);
    let record = DerivationRecord {
        derivation_id: expected_id.as_str().to_owned(),
        output_hash: output_hash.clone(),
        package_name: "glibc".to_owned(),
        package_version: "2.39".to_owned(),
        manifest_cas_hash: "manifest_hash_456".to_owned(),
        stage: Some("phase1".to_owned()),
        build_env_hash: Some(build_env_hash.to_owned()),
        built_at: "2026-03-19T12:00:00Z".to_owned(),
        build_duration_secs: 30,
        trust_level: crate::derivation::index::DerivationTrustLevel::Unverified,
        provenance_cas_hash: None,
        reproducible: None,
    };
    let index = DerivationIndex::new(&conn);
    index.insert(&record).unwrap();

    // Execute -- should get a cache hit without building.
    let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), ExecutorConfig::default());
    let sysroot = tmp.path().join("sysroot");
    std::fs::create_dir_all(&sysroot).unwrap();

    let result = executor
        .execute(
            &recipe,
            build_env_hash,
            &dep_ids,
            target_triple,
            &sysroot,
            &conn,
        )
        .expect("execute must succeed");

    match result {
        ExecutionResult::CacheHit {
            derivation_id,
            record: hit_record,
        } => {
            assert_eq!(derivation_id, expected_id);
            assert_eq!(hit_record.output_hash, output_hash);
            assert_eq!(hit_record.package_name, "glibc");
            assert_eq!(hit_record.manifest_cas_hash, "manifest_hash_456");
        }
        ExecutionResult::Built { .. } => {
            panic!("expected CacheHit, got Built");
        }
    }
}

#[test]
fn execute_rejects_local_source_recipe_before_build() {
    let tmp = TempDir::new().unwrap();
    let cas = test_cas(tmp.path());
    let conn = setup_db();
    let recipe: Recipe = toml::from_str(
        r#"
[package]
name = "local"
version = "1.0"

[source]
path = "src"

[build]
make = "true"
install = "true"
"#,
    )
    .expect("local source recipe must parse");
    let sysroot = tmp.path().join("sysroot");
    std::fs::create_dir_all(&sysroot).unwrap();

    let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), ExecutorConfig::default());
    let result = executor.execute(
        &recipe,
        "env_hash",
        &BTreeMap::new(),
        "x86_64-unknown-linux-gnu",
        &sysroot,
        &conn,
    );

    let error = result.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("local source recipes are not supported by derivation IDs in M1a"),
        "expected explicit M1a derivation rejection, got: {error}"
    );
}

#[test]
fn derivation_id_is_deterministic_across_calls() {
    let recipe = test_recipe("zlib", "1.3.1");
    let dep_ids = BTreeMap::new();

    let compute_id = |recipe: &Recipe| {
        let inputs = DerivationInputs {
            source_hash: recipe_hash::source_hash(recipe),
            build_script_hash: recipe_hash::build_script_hash(recipe),
            dependency_ids: dep_ids.clone(),
            build_env_hash: "env_aaa".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            build_options: BTreeMap::new(),
        };
        DerivationId::compute(&inputs).unwrap()
    };

    let id1 = compute_id(&recipe);
    let id2 = compute_id(&recipe);
    assert_eq!(id1, id2, "same inputs must produce same derivation ID");
}

#[test]
fn different_deps_produce_different_ids() {
    let recipe = test_recipe("bash", "5.2");

    let mut deps1 = BTreeMap::new();
    deps1.insert(
        "glibc".to_owned(),
        DerivationId::compute(&DerivationInputs {
            source_hash: "src1".to_owned(),
            build_script_hash: "script1".to_owned(),
            dependency_ids: BTreeMap::new(),
            build_env_hash: "env1".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            build_options: BTreeMap::new(),
        })
        .unwrap(),
    );

    let mut deps2 = BTreeMap::new();
    deps2.insert(
        "glibc".to_owned(),
        DerivationId::compute(&DerivationInputs {
            source_hash: "src2_different".to_owned(),
            build_script_hash: "script1".to_owned(),
            dependency_ids: BTreeMap::new(),
            build_env_hash: "env1".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            build_options: BTreeMap::new(),
        })
        .unwrap(),
    );

    let make_id = |deps: &BTreeMap<String, DerivationId>| {
        let inputs = DerivationInputs {
            source_hash: recipe_hash::source_hash(&recipe),
            build_script_hash: recipe_hash::build_script_hash(&recipe),
            dependency_ids: deps.clone(),
            build_env_hash: "env_aaa".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            build_options: BTreeMap::new(),
        };
        DerivationId::compute(&inputs).unwrap()
    };

    let id1 = make_id(&deps1);
    let id2 = make_id(&deps2);
    assert_ne!(
        id1, id2,
        "different dependency IDs must produce different derivation IDs"
    );
}

#[test]
fn cas_accessor_returns_store() {
    let tmp = TempDir::new().unwrap();
    let cas = test_cas(tmp.path());
    let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), ExecutorConfig::default());
    // Just verify the accessor doesn't panic and returns a usable store.
    assert!(!executor.cas().exists("nonexistent"));
}

#[test]
fn cache_miss_with_no_kitchen_infra_returns_build_error() {
    // Without real source archives, Kitchen build will fail at prep.
    // This confirms the error path works correctly.
    let tmp = TempDir::new().unwrap();
    let cas = test_cas(tmp.path());
    let conn = setup_db();

    let recipe = test_recipe("coreutils", "9.5");
    let dep_ids = BTreeMap::new();
    let sysroot = tmp.path().join("sysroot");
    std::fs::create_dir_all(&sysroot).unwrap();

    let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), ExecutorConfig::default());
    let result = executor.execute(
        &recipe,
        "env_hash",
        &dep_ids,
        "x86_64-unknown-linux-gnu",
        &sysroot,
        &conn,
    );

    match result {
        Err(ExecutorError::Build(msg)) => {
            assert!(
                msg.contains("prep"),
                "error should mention the prep phase, got: {msg}",
            );
        }
        other => {
            panic!("expected Build error from prep phase, got: {other:?}",);
        }
    }
}

#[test]
fn destdir_cleaned_up_on_build_failure() {
    // Without real source archives the build will fail during prep.
    // The CleanupGuard should remove the DESTDIR on any error path.
    let tmp = TempDir::new().unwrap();
    let cas = test_cas(tmp.path());
    let conn = setup_db();

    let recipe = test_recipe("sed", "4.9");
    let dep_ids = BTreeMap::new();
    let sysroot = tmp.path().join("sysroot");
    std::fs::create_dir_all(&sysroot).unwrap();

    let cas_dir = tmp.path().join("cas");
    let executor = DerivationExecutor::new(cas, cas_dir.clone(), ExecutorConfig::default());

    let result = executor.execute(
        &recipe,
        "env_hash",
        &dep_ids,
        "x86_64-unknown-linux-gnu",
        &sysroot,
        &conn,
    );

    // The build must fail (no real sources).
    assert!(result.is_err(), "execute should fail without real sources");

    // Compute the expected DESTDIR path to verify it was cleaned up.
    let src_hash = recipe_hash::source_hash(&recipe);
    let script_hash = recipe_hash::build_script_hash(&recipe);
    let inputs = DerivationInputs {
        source_hash: src_hash,
        build_script_hash: script_hash,
        dependency_ids: dep_ids,
        build_env_hash: "env_hash".to_owned(),
        target_triple: "x86_64-unknown-linux-gnu".to_owned(),
        build_options: BTreeMap::new(),
    };
    let derivation_id = DerivationId::compute(&inputs).unwrap();
    let destdir = cas_dir.join(format!("build-{}", &derivation_id.as_str()[..16]));

    assert!(
        !destdir.exists(),
        "DESTDIR should have been cleaned up on build failure: {}",
        destdir.display(),
    );
}

#[test]
fn shell_on_failure_does_not_hang_without_tty() {
    static REPORTED_DESTDIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
    fn report(event: DebugShellEvent<'_>) {
        let DebugShellEvent::BuildFailed {
            package,
            version,
            log_path,
            sysroot,
            destdir,
        } = event
        else {
            panic!("non-TTY failure must not start a shell");
        };
        assert_eq!((package, version), ("sed", "4.9"));
        assert!(sysroot.is_dir());
        assert!(destdir.is_dir());
        assert!(log_path.unwrap().is_file());
        *REPORTED_DESTDIR.lock().unwrap() = Some(destdir.to_path_buf());
    }

    // Force the non-interactive path even when the test runner has a tty.
    // Verify that with shell_on_failure=true, execute() still returns
    // the build error without blocking.
    let _stdin_guard = StdinTerminalOverrideGuard::non_tty();
    let tmp = TempDir::new().unwrap();
    let cas = test_cas(tmp.path());
    let conn = setup_db();

    let config = ExecutorConfig {
        log_dir: Some(tmp.path().join("logs")),
        keep_logs: false,
        shell_on_failure: true, // enabled, but no tty in tests
    };

    let recipe = test_recipe("sed", "4.9");
    let sysroot = tmp.path().join("sysroot");
    std::fs::create_dir_all(&sysroot).unwrap();

    let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), config)
        .with_debug_shell_reporter(report);
    let result = executor.execute(
        &recipe,
        "env_hash",
        &BTreeMap::new(),
        "x86_64-unknown-linux-gnu",
        &sysroot,
        &conn,
    );

    // Should fail with Build error, not hang
    assert!(matches!(result, Err(ExecutorError::Build(_))));
    let destdir = REPORTED_DESTDIR
        .lock()
        .unwrap()
        .take()
        .expect("failure reported before cleanup");
    assert!(!destdir.exists());
}

#[test]
fn build_log_written_on_failure() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let cas = test_cas(tmp.path());
    let conn = setup_db();

    let config = ExecutorConfig {
        log_dir: Some(log_dir.clone()),
        keep_logs: false,
        shell_on_failure: false,
    };

    let recipe = test_recipe("sed", "4.9");
    let sysroot = tmp.path().join("sysroot");
    std::fs::create_dir_all(&sysroot).unwrap();

    let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), config);
    let result = executor.execute(
        &recipe,
        "env_hash",
        &BTreeMap::new(),
        "x86_64-unknown-linux-gnu",
        &sysroot,
        &conn,
    );

    assert!(result.is_err(), "execute should fail without real sources");

    // Log file should exist in the logs directory.
    let logs: Vec<_> = std::fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(logs.len(), 1, "should have one log file");

    let content = std::fs::read_to_string(logs[0].path()).unwrap();
    assert!(
        content.contains("package: sed"),
        "log should contain package name"
    );
    assert!(
        content.contains("status: FAILED"),
        "log should show FAILED status"
    );
}

#[test]
fn provenance_storage_failure_propagates() {
    let tmp = TempDir::new().unwrap();
    let cas = test_cas(tmp.path());
    // A regular file at the CAS root deterministically rejects writes even as root.
    std::fs::remove_dir_all(cas.objects_dir()).unwrap();
    std::fs::write(cas.objects_dir(), b"blocked").unwrap();
    let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), ExecutorConfig::default());
    let provenance = crate::provenance::Provenance::new(
        crate::provenance::SourceProvenance::from_tarball(
            "https://example.com/test.tar.gz",
            "sha256:abc123",
        ),
        crate::provenance::BuildProvenance::new("script_hash"),
        crate::provenance::SignatureProvenance::default(),
        crate::provenance::ContentProvenance::new("output_hash"),
    );
    assert!(matches!(
        executor.store_provenance(&provenance),
        Err(ExecutorError::Cas(_))
    ));
}
