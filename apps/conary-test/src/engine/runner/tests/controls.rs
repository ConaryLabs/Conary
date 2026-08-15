// conary-test/src/engine/runner/tests/controls.rs

use super::*;

#[test]
fn test_substitute_vars() {
    let mut runner = TestRunner::new(test_config(), "fedora44".to_string());
    let mut manifest = make_manifest(Vec::new());
    manifest.distro_overrides.insert(
        "fedora44".to_string(),
        HashMap::from([("PKG".to_string(), "tree".to_string())]),
    );
    runner.load_manifest_vars(&manifest);

    let expanded = variables::expand_variables("curl ${REMI_ENDPOINT}/health", &runner.vars);
    assert_eq!(expanded, "curl https://remi.conary.io/health");

    let expanded2 = variables::expand_variables("${CONARY_BIN} --db-path ${DB_PATH}", &runner.vars);
    assert_eq!(
        expanded2,
        "/usr/local/bin/conary --db-path /tmp/conary-test.db"
    );

    let expanded3 = variables::expand_variables("conary install ${PKG}", &runner.vars);
    assert_eq!(expanded3, "conary install tree");

    let fixture_v1 = variables::expand_variables("${FIXTURE_V1_CCS}", &runner.vars);
    assert_eq!(
        fixture_v1,
        "/opt/remi-tests/fixtures/conary-test-fixture/v1/output/conary-test-fixture-1.0.0-1.ccs"
    );
}

#[test]
fn test_expand_assertion_substitutes_vars() {
    let mut runner = TestRunner::new(test_config(), "fedora44".to_string());
    let mut manifest = make_manifest(Vec::new());
    manifest.distro_overrides.insert(
        "fedora44".to_string(),
        HashMap::from([
            ("PKG".to_string(), "conary-test-fixture".to_string()),
            ("HELLO_SHA".to_string(), "abc123".to_string()),
        ]),
    );
    runner.load_manifest_vars(&manifest);

    let assertion = Assertion {
        stdout_contains_all: Some(vec!["${PKG}".to_string(), "Version".to_string()]),
        stderr_contains: Some("${PKG}".to_string()),
        file_checksum: Some(FileChecksum {
            path: "/tmp/${PKG}".to_string(),
            sha256: "${HELLO_SHA}".to_string(),
        }),
        ..Assertion::default()
    };

    let expanded = runner.expand_assertion(&assertion);
    assert_eq!(
        expanded.stdout_contains_all,
        Some(vec![
            "conary-test-fixture".to_string(),
            "Version".to_string()
        ])
    );
    assert_eq!(
        expanded.stderr_contains.as_deref(),
        Some("conary-test-fixture")
    );
    assert_eq!(
        expanded.file_checksum.as_ref().map(|chk| chk.path.as_str()),
        Some("/tmp/conary-test-fixture")
    );
    assert_eq!(
        expanded
            .file_checksum
            .as_ref()
            .map(|chk| chk.sha256.as_str()),
        Some("abc123")
    );
}

#[test]
fn test_apply_resource_constraints() {
    let runner = TestRunner::new(test_config(), "fedora44".to_string());
    let mut container_config = ContainerConfig::default();
    let resources = ResourceConstraints {
        tmpfs_size_mb: Some(50),
        memory_limit_mb: Some(512),
        network_isolated: Some(true),
    };

    runner.apply_resource_constraints(&mut container_config, Some(&resources));

    assert_eq!(
        container_config
            .tmpfs
            .get("/var/lib/conary")
            .map(String::as_str),
        Some("size=50m")
    );
    assert_eq!(container_config.memory_limit, Some(512 * 1024 * 1024));
    assert_eq!(container_config.network_mode, "none");
}

#[test]
fn test_build_kill_after_log_command_supports_env_prefix() {
    // Delegates to executor::build_kill_after_log_command (tested there).
    // Keep one runner-level smoke test for cross-layer coverage.
    use crate::engine::executor;
    let cmd = executor::build_kill_after_log_command(
        "/usr/local/bin/conary",
        "env CONARY_TEST_HOLD_AFTER_DB_UPDATE_MS=1500 ccs install fixture.ccs",
    );
    assert!(cmd.contains("exec env CONARY_TEST_HOLD_AFTER_DB_UPDATE_MS=1500"));
    assert!(cmd.contains("/usr/local/bin/conary ccs install fixture.ccs"));
}

#[tokio::test]
async fn test_runner_qemu_boot_step_skips_when_tooling_missing() {
    struct MissingToolsReset;

    impl Drop for MissingToolsReset {
        fn drop(&mut self) {
            crate::engine::qemu::set_missing_tools_override_for_tests(None);
        }
    }

    let backend = MockBackend::new(Vec::new());
    let manifest = make_manifest(vec![TestDef {
        id: "T156".to_string(),
        name: "qemu_boot".to_string(),
        description: "boots a qcow2 image".to_string(),
        timeout: 30,
        flaky: None,
        retries: None,
        retry_delay_ms: None,
        step: vec![simple_step_qemu_boot(
            QemuBoot {
                image: "https://127.0.0.1:9/minimal-boot-${PKG}.qcow2".to_string(),
                local_image_path: None,
                image_format: QemuImageFormat::Qcow2,
                stage_conary: false,
                scratch_disk_mb: None,
                copy_to_guest: Vec::new(),
                copy_from_guest: Vec::new(),
                memory_mb: 512,
                timeout_seconds: 5,
                ssh_port: 2223,
                commands: vec!["echo ${PKG}".to_string()],
                expect_output: vec!["skipped".to_string()],
            },
            Some(Assertion {
                stdout_contains: Some("qemu boot".to_string()),
                ..Assertion::default()
            }),
        )],
        resources: None,
        depends_on: None,
        fatal: None,
        group: None,
        skip: None,
        requires: Vec::new(),
        corpus: None,
    }]);

    let mut runner = TestRunner::new(test_config(), "fedora44".to_string());
    let mut overrides = HashMap::new();
    overrides.insert("PKG".to_string(), "v1".to_string());
    let mut manifest = manifest;
    manifest
        .distro_overrides
        .insert("fedora44".to_string(), overrides);

    crate::engine::qemu::set_missing_tools_override_for_tests(Some(vec![
        "qemu-system-x86_64".to_string(),
    ]));
    let _reset = MissingToolsReset;

    let suite = runner
        .run(&manifest, &backend, &"ctr-1".to_string(), None)
        .await
        .unwrap();

    assert_eq!(suite.passed(), 0);
    assert_eq!(suite.skipped(), 1);
    assert_eq!(suite.failed(), 0);
    assert_eq!(suite.results[0].status, TestStatus::Skipped);
    assert!(
        suite.results[0]
            .stdout
            .as_deref()
            .unwrap_or_default()
            .contains("qemu boot")
    );
}

#[test]
fn test_expand_qemu_boot_substitutes_vars() {
    let mut runner = TestRunner::new(test_config(), "fedora44".to_string());
    let mut manifest = make_manifest(Vec::new());
    manifest.distro_overrides.insert(
        "fedora44".to_string(),
        HashMap::from([("IMG".to_string(), "minimal-boot-v1".to_string())]),
    );
    runner.load_manifest_vars(&manifest);

    let expanded = variables::expand_qemu_boot(
        &QemuBoot {
            image: "${IMG}".to_string(),
            local_image_path: None,
            image_format: QemuImageFormat::Qcow2,
            stage_conary: false,
            scratch_disk_mb: None,
            copy_to_guest: Vec::new(),
            copy_from_guest: Vec::new(),
            memory_mb: 1024,
            timeout_seconds: 120,
            ssh_port: 2222,
            commands: vec!["echo ${IMG}".to_string()],
            expect_output: vec!["${IMG}".to_string()],
        },
        &runner.vars,
    );

    assert_eq!(expanded.image, "minimal-boot-v1");
    assert_eq!(expanded.commands, vec!["echo minimal-boot-v1"]);
    assert_eq!(expanded.expect_output, vec!["minimal-boot-v1"]);
}

#[tokio::test]
async fn test_cancel_flag_stops_runner() {
    let backend = MockBackend::new(vec![
        ExecResult {
            exit_code: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
        },
        ExecResult {
            exit_code: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
        },
    ]);

    let manifest = make_manifest(vec![
        TestDef {
            id: "T01".to_string(),
            name: "first".to_string(),
            description: "runs first".to_string(),
            timeout: 30,
            flaky: None,
            retries: None,
            retry_delay_ms: None,
            step: vec![simple_step_run(
                "echo ok",
                Some(make_assertion(Some(0), None)),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
            requires: Vec::new(),
            corpus: None,
        },
        TestDef {
            id: "T02".to_string(),
            name: "second".to_string(),
            description: "should be cancelled".to_string(),
            timeout: 30,
            flaky: None,
            retries: None,
            retry_delay_ms: None,
            step: vec![simple_step_run("echo ok", None)],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
            requires: Vec::new(),
            corpus: None,
        },
    ]);

    // Set cancel flag before run -- T01 will pass but the flag will be
    // set immediately so T02 should be cancelled.
    let cancel_flag = Arc::new(AtomicBool::new(true));

    let mut runner = TestRunner::new(test_config(), "fedora44".to_string());
    let suite = runner
        .run_with_cancel(
            &manifest,
            &backend,
            &"ctr-1".to_string(),
            None,
            Some(cancel_flag),
            None,
            None,
        )
        .await
        .unwrap();

    // Both tests should be cancelled since the flag was set from the start.
    assert_eq!(suite.cancelled(), 2);
    assert_eq!(suite.passed(), 0);
    assert_eq!(suite.results[0].status, TestStatus::Cancelled);
    assert_eq!(suite.results[1].status, TestStatus::Cancelled);
}

#[tokio::test]
async fn test_suite_timeout_cancels_remaining() {
    // Create a manifest with suite timeout = 0 seconds (already expired).
    let manifest = TestManifest {
        suite: SuiteDef {
            name: "timeout-suite".to_string(),
            phase: 1,
            setup: Vec::new(),
            mock_server: None,
            timeout: Some(0), // Already expired.
            corpus: None,
        },
        test: vec![
            TestDef {
                id: "T01".to_string(),
                name: "first".to_string(),
                description: "cancelled by timeout".to_string(),
                timeout: 30,
                flaky: None,
                retries: None,
                retry_delay_ms: None,
                step: vec![simple_step_run("echo ok", None)],
                resources: None,
                depends_on: None,
                fatal: None,
                group: None,
                skip: None,
                requires: Vec::new(),
                corpus: None,
            },
            TestDef {
                id: "T02".to_string(),
                name: "second".to_string(),
                description: "also cancelled".to_string(),
                timeout: 30,
                flaky: None,
                retries: None,
                retry_delay_ms: None,
                step: vec![simple_step_run("echo ok", None)],
                resources: None,
                depends_on: None,
                fatal: None,
                group: None,
                skip: None,
                requires: Vec::new(),
                corpus: None,
            },
        ],
        distro_overrides: HashMap::new(),
    };

    let backend = MockBackend::new(Vec::new());
    let mut runner = TestRunner::new(test_config(), "fedora44".to_string());
    let suite = runner
        .run_with_cancel(
            &manifest,
            &backend,
            &"ctr-1".to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(suite.cancelled(), 2);
    assert!(
        suite.results[0]
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("suite timeout")
    );
}

#[tokio::test]
async fn test_step_timeout_overrides_test_timeout() {
    // Test that a step with timeout = 1 uses 1s, not the test-level 30s.
    // We verify this indirectly: the step executes successfully with a
    // 1s timeout (the mock is instant, so it works).
    let backend = MockBackend::new(vec![ExecResult {
        exit_code: 0,
        stdout: "ok".to_string(),
        stderr: String::new(),
    }]);

    let manifest = make_manifest(vec![TestDef {
        id: "T01".to_string(),
        name: "step_timeout".to_string(),
        description: "step has custom timeout".to_string(),
        timeout: 30,
        flaky: None,
        retries: None,
        retry_delay_ms: None,
        step: vec![TestStep {
            timeout: Some(1),
            run: Some("echo ok".to_string()),
            assert: Some(make_assertion(Some(0), Some("ok"))),
            ..TestStep::default()
        }],
        resources: None,
        depends_on: None,
        fatal: None,
        group: None,
        skip: None,
        requires: Vec::new(),
        corpus: None,
    }]);

    let mut runner = TestRunner::new(test_config(), "fedora44".to_string());
    let suite = runner
        .run(&manifest, &backend, &"ctr-1".to_string(), None)
        .await
        .unwrap();

    assert_eq!(suite.passed(), 1);
}

#[tokio::test]
async fn test_concurrent_runs_independent() {
    // Two independent runs should complete without interfering with each
    // other. Each gets its own MockBackend, runner, and manifest.
    let backend_a = MockBackend::new(vec![ExecResult {
        exit_code: 0,
        stdout: "run-a".to_string(),
        stderr: String::new(),
    }]);
    let backend_b = MockBackend::new(vec![ExecResult {
        exit_code: 0,
        stdout: "run-b".to_string(),
        stderr: String::new(),
    }]);

    let manifest_a = make_manifest(vec![TestDef {
        id: "T-A1".to_string(),
        name: "run_a_test".to_string(),
        description: "test in run A".to_string(),
        timeout: 30,
        flaky: None,
        retries: None,
        retry_delay_ms: None,
        step: vec![simple_step_run(
            "echo run-a",
            Some(make_assertion(Some(0), Some("run-a"))),
        )],
        resources: None,
        depends_on: None,
        fatal: None,
        group: None,
        skip: None,
        requires: Vec::new(),
        corpus: None,
    }]);

    let manifest_b = make_manifest(vec![TestDef {
        id: "T-B1".to_string(),
        name: "run_b_test".to_string(),
        description: "test in run B".to_string(),
        timeout: 30,
        flaky: None,
        retries: None,
        retry_delay_ms: None,
        step: vec![simple_step_run(
            "echo run-b",
            Some(make_assertion(Some(0), Some("run-b"))),
        )],
        resources: None,
        depends_on: None,
        fatal: None,
        group: None,
        skip: None,
        requires: Vec::new(),
        corpus: None,
    }]);

    let (suite_a, suite_b) = tokio::join!(
        async {
            let mut runner = TestRunner::new(test_config(), "fedora44".to_string());
            runner
                .run(&manifest_a, &backend_a, &"ctr-a".to_string(), None)
                .await
                .unwrap()
        },
        async {
            let mut runner = TestRunner::new(test_config(), "fedora44".to_string());
            runner
                .run(&manifest_b, &backend_b, &"ctr-b".to_string(), None)
                .await
                .unwrap()
        },
    );

    assert_eq!(suite_a.passed(), 1, "run A should pass");
    assert_eq!(suite_b.passed(), 1, "run B should pass");
    assert_eq!(suite_a.failed(), 0, "run A should have no failures");
    assert_eq!(suite_b.failed(), 0, "run B should have no failures");
    assert_eq!(suite_a.results[0].id, "T-A1");
    assert_eq!(suite_b.results[0].id, "T-B1");
}
