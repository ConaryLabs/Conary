// conary-test/src/config/tests.rs

use super::*;
use std::path::PathBuf;

fn remi_manifest_path(file_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../conary/tests/integration/remi/manifests")
        .join(file_name)
}

fn conary_fixture_path(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../conary/tests/fixtures")
        .join(path)
}

fn manifest_step_commands(step: &manifest::TestStep) -> Vec<&str> {
    let mut commands = Vec::new();
    commands.extend(step.conary.iter().map(String::as_str));
    commands.extend(step.run.iter().map(String::as_str));
    commands.extend(
        step.kill_after_log
            .iter()
            .map(|config| config.conary.as_str()),
    );
    if let Some(config) = &step.qemu_boot {
        commands.extend(config.commands.iter().map(String::as_str));
    }
    commands
}

fn minimal_manifest_with_id(id: &str) -> TestManifest {
    TestManifest {
        suite: manifest::SuiteDef {
            name: format!("suite-{id}"),
            phase: 1,
            setup: Vec::new(),
            mock_server: None,
            timeout: None,
        },
        test: vec![manifest::TestDef {
            id: id.to_string(),
            name: format!("test-{id}"),
            description: "duplicate id test".to_string(),
            timeout: 10,
            flaky: None,
            retries: None,
            retry_delay_ms: None,
            step: Vec::new(),
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
            requires: Vec::new(),
        }],
        distro_overrides: Default::default(),
    }
}

#[test]
fn active_manifest_ccs_commands_require_fixture_signing_authority() {
    let manifest_dir = remi_manifest_path("");
    if !manifest_dir.exists() {
        return;
    }

    let mut removed_bypass_tests = Vec::new();
    for entry in std::fs::read_dir(&manifest_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }

        let manifest = load_manifest(&path).unwrap();
        for (index, step) in manifest.suite.setup.iter().enumerate() {
            for command in manifest_step_commands(step) {
                if command.contains("ccs install ") || command.contains("ccs verify ") {
                    assert!(
                        command.contains("--policy"),
                        "{}:suite setup step {} CCS install/verify must name a trust policy: {}",
                        path.display(),
                        index + 1,
                        command
                    );
                }
                if command.contains("ccs build ") {
                    assert!(
                        command.contains("--key"),
                        "{}:suite setup step {} CCS build must use the fixture signing key: {}",
                        path.display(),
                        index + 1,
                        command
                    );
                }
                assert!(
                    !command.contains("--allow-unsigned"),
                    "{}:suite setup step {} must not use the removed unsigned bypass: {}",
                    path.display(),
                    index + 1,
                    command
                );
            }
        }

        for test in &manifest.test {
            for (index, step) in test.step.iter().enumerate() {
                for command in manifest_step_commands(step) {
                    if command.contains("ccs install ") || command.contains("ccs verify ") {
                        assert!(
                            command.contains("--policy"),
                            "{}:{} step {} CCS install/verify must name a trust policy: {}",
                            path.display(),
                            test.id,
                            index + 1,
                            command
                        );
                    }
                    if command.contains("ccs build ") {
                        assert!(
                            command.contains("--key"),
                            "{}:{} step {} CCS build must use the fixture signing key: {}",
                            path.display(),
                            test.id,
                            index + 1,
                            command
                        );
                    }
                    if command.contains("--allow-unsigned") {
                        assert_eq!(
                            test.id,
                            "T86",
                            "{}:{} step {} must not use the removed unsigned bypass: {}",
                            path.display(),
                            test.id,
                            index + 1,
                            command
                        );
                        assert_eq!(
                            step.assert
                                .as_ref()
                                .and_then(|assertion| assertion.exit_code_not),
                            Some(0),
                            "T86 must prove the removed --allow-unsigned flag is rejected"
                        );
                        removed_bypass_tests.push(format!("{}:{}", path.display(), test.id));
                    }
                }
            }
        }
    }

    assert_eq!(
        removed_bypass_tests.len(),
        1,
        "exactly one focused removed --allow-unsigned rejection test should remain: {removed_bypass_tests:?}"
    );
}

#[test]
fn test_validate_unique_test_ids_reports_duplicate_locations() {
    let manifests = vec![
        (
            PathBuf::from("phase-a.toml"),
            minimal_manifest_with_id("T01"),
        ),
        (
            PathBuf::from("phase-b.toml"),
            minimal_manifest_with_id("T01"),
        ),
    ];

    let err = validate_unique_test_ids(&manifests).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("duplicate test id `T01`"));
    assert!(msg.contains("phase-a.toml"));
    assert!(msg.contains("phase-b.toml"));
}

fn is_package_remove_segment(segment: &str) -> bool {
    segment.starts_with("remove ")
        || segment.contains("${CONARY_BIN} remove ")
        || (segment.starts_with("env ") && segment.contains(" remove "))
}

fn segment_matches_command(segment: &str, command: &str) -> bool {
    segment.starts_with(command)
        || segment.contains(&format!(" {command} "))
        || segment.contains(&format!("${{CONARY_BIN}} {command}"))
}

fn is_package_apply_segment(segment: &str) -> bool {
    segment.starts_with("install ")
        || segment.starts_with("update ")
        || segment.starts_with("ccs install ")
        || segment.contains("${CONARY_BIN} install ")
        || segment.contains("${CONARY_BIN} update ")
        || segment.contains("${CONARY_BIN} ccs install ")
        || (segment.starts_with("env ")
            && (segment.contains(" install ") || segment.contains(" ccs install ")))
}

fn is_system_mutation_segment(segment: &str) -> bool {
    if segment_matches_command(segment, "system adopt") {
        return segment.split_whitespace().any(|arg| arg == "--sync-hook");
    }

    [
        "system restore",
        "system native-handoff",
        "system state revert",
        "system state rollback",
        "system db-backup recover",
        "system generation build",
        "system generation publish",
        "system generation switch",
        "system generation gc",
        "system generation rollback",
        "system generation recover",
        "system generation recover-db",
        "system takeover",
        "system unadopt",
        "model apply",
        "automation apply",
    ]
    .iter()
    .any(|command| segment_matches_command(segment, command))
}

fn live_mutation_segments(command: &str) -> impl Iterator<Item = &str> {
    command.split(';').filter_map(|segment| {
        let segment = segment.trim();
        (is_package_apply_segment(segment)
            || is_package_remove_segment(segment)
            || is_system_mutation_segment(segment))
        .then_some(segment)
    })
}

fn is_dry_run_segment(segment: &str) -> bool {
    segment.split_whitespace().any(|arg| arg == "--dry-run")
}

fn has_apply_intent(segment: &str) -> bool {
    segment.contains("--yes")
}

#[test]
fn test_parse_minimal_manifest() {
    let toml = r#"
[suite]
name = "smoke"
phase = 1

[[test]]
id = "T01"
name = "health_check"
description = "Verify Remi is reachable"
timeout = 10

[[test.step]]
run = "curl -sf http://localhost:8081/health"

[test.step.assert]
exit_code = 0
stdout_contains = "ok"
"#;
    let manifest: TestManifest = toml::from_str(toml).unwrap();
    assert_eq!(manifest.suite.name, "smoke");
    assert_eq!(manifest.suite.phase, 1);
    assert_eq!(manifest.test.len(), 1);

    let test = &manifest.test[0];
    assert_eq!(test.id, "T01");
    assert_eq!(test.name, "health_check");
    assert_eq!(test.timeout, 10);
    assert_eq!(test.step.len(), 1);

    let step = &test.step[0];
    assert_eq!(
        step.run.as_deref(),
        Some("curl -sf http://localhost:8081/health")
    );
    let assertion = step.assert.as_ref().unwrap();
    assert_eq!(assertion.exit_code, Some(0));
    assert_eq!(assertion.stdout_contains.as_deref(), Some("ok"));
}

#[test]
fn test_parse_multi_step_test() {
    let toml = r#"
[suite]
name = "install"
phase = 1

[[test]]
id = "T05"
name = "install_package"
description = "Install and verify a package"
timeout = 60
fatal = true

[[test.step]]
conary = "install tree"

[test.step.assert]
exit_code = 0

[[test.step]]
file_exists = "/usr/bin/tree"
"#;
    let manifest: TestManifest = toml::from_str(toml).unwrap();
    let test = &manifest.test[0];
    assert_eq!(test.fatal, Some(true));
    assert_eq!(test.step.len(), 2);

    let step0 = &test.step[0];
    assert_eq!(step0.conary.as_deref(), Some("install tree"));
    assert!(matches!(step0.step_type(), Some(StepType::Conary(_))));

    let step1 = &test.step[1];
    assert_eq!(step1.file_exists.as_deref(), Some("/usr/bin/tree"));
    assert!(matches!(step1.step_type(), Some(StepType::FileExists(_))));
}

#[test]
fn test_parse_depends_on() {
    let toml = r#"
[suite]
name = "deps"
phase = 1

[[test]]
id = "T02"
name = "repo_sync"
description = "Sync after adding repo"
timeout = 30
depends_on = ["T01"]

[[test.step]]
conary = "repo sync"
"#;
    let manifest: TestManifest = toml::from_str(toml).unwrap();
    let test = &manifest.test[0];
    let deps = test.depends_on.as_ref().unwrap();
    assert_eq!(deps, &["T01"]);
}

#[test]
fn test_parse_global_config() {
    let toml = r#"
[remi]
endpoint = "https://remi.conary.io"

[paths]
db = "/tmp/conary-test.db"
conary_bin = "/usr/local/bin/conary"
results_dir = "/tmp/results"

[setup]
remove_default_repos = ["fedora", "updates"]

[distros.fedora44]
remi_distro = "fedora-44"
repo_name = "remi-fedora-44"
build_context = "binary"
containerfile = "Containerfile.fedora44"
test_packages = [{ package = "tree", binary = "/usr/bin/tree" }]

[distros."ubuntu-26.04"]
remi_distro = "ubuntu-26.04"
repo_name = "remi-ubuntu-26.04"
build_context = "workspace-source"
test_packages = [{ package = "tree", binary = "/usr/bin/tree" }]

[fixtures]
package = "conary-test-fixture"
file = "/usr/share/conary-test/hello.txt"

[fixtures.v1]
version = "1.0.0"
ccs_file = "conary-test-fixture-1.0.0-1.ccs"
"#;
    let config: GlobalConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.remi.endpoint, "https://remi.conary.io");
    assert_eq!(config.paths.db, "/tmp/conary-test.db");
    assert_eq!(config.setup.remove_default_repos, &["fedora", "updates"]);
    assert_eq!(config.distros.len(), 2);

    let fedora = &config.distros["fedora44"];
    assert_eq!(fedora.remi_distro, "fedora-44");
    assert_eq!(fedora.repo_name, "remi-fedora-44");
    assert_eq!(fedora.build_context, DistroBuildContext::Binary);
    assert_eq!(fedora.test_packages.len(), 1);
    assert_eq!(fedora.test_packages[0].package, "tree");
    assert_eq!(fedora.test_packages[0].binary, "/usr/bin/tree");
    assert_eq!(
        fedora.containerfile.as_deref(),
        Some("Containerfile.fedora44")
    );
    assert_eq!(
        config.distros["ubuntu-26.04"].build_context,
        DistroBuildContext::WorkspaceSource
    );

    let fixtures = config.fixtures.as_ref().unwrap();
    assert_eq!(fixtures.package.as_deref(), Some("conary-test-fixture"));
    assert_eq!(
        fixtures.file.as_deref(),
        Some("/usr/share/conary-test/hello.txt")
    );
    assert_eq!(fixtures.v1_version.as_deref(), Some("1.0.0"));
    assert_eq!(
        fixtures.v1_ccs_file.as_deref(),
        Some("conary-test-fixture-1.0.0-1.ccs")
    );
}

#[test]
fn distro_config_rejects_numbered_package_fields() {
    for (field, value) in [
        ("test_package", "\"tree\""),
        ("test_binary", "\"/usr/bin/tree\""),
    ] {
        let toml = format!(
            r#"
[remi]
endpoint = "https://remi.conary.io"

[paths]
db = "/tmp/conary-test.db"
conary_bin = "/usr/bin/conary"
results_dir = "/tmp/results"

[distros.fedora44]
remi_distro = "fedora-44"
repo_name = "remi-fedora-44"
build_context = "binary"
{field} = {value}
"#
        );

        let error = toml::from_str::<GlobalConfig>(&toml)
            .expect_err("legacy singular package fields must not deserialize");
        let message = error.to_string();
        assert!(
            message.contains("unknown field") && message.contains(field),
            "strict schema rejection should identify legacy field {field}: {message}"
        );
        assert!(
            message.contains("test_packages"),
            "strict schema rejection should direct callers to test_packages: {message}"
        );
    }
}

#[test]
fn distro_config_requires_a_typed_build_context() {
    let missing = r#"
[remi]
endpoint = "https://remi.conary.io"

[paths]
db = "/tmp/conary-test.db"
conary_bin = "/usr/bin/conary"
results_dir = "/tmp/results"

[distros.custom]
remi_distro = "custom"
repo_name = "custom"
"#;
    let missing_error = toml::from_str::<GlobalConfig>(missing)
        .expect_err("build context must be explicit")
        .to_string();
    assert!(
        missing_error.contains("build_context"),
        "missing capability should identify build_context: {missing_error}"
    );

    let invalid = format!("{missing}\nbuild_context = \"ubuntu-name-heuristic\"\n");
    let invalid_error = toml::from_str::<GlobalConfig>(&invalid)
        .expect_err("build context must use the typed contract")
        .to_string();
    assert!(
        invalid_error.contains("binary") && invalid_error.contains("workspace-source"),
        "invalid capability should list typed values: {invalid_error}"
    );
}

#[test]
fn test_load_phase1_advanced_manifest() {
    let path = remi_manifest_path("phase1-advanced.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert!(
            manifest.test.len() >= 27,
            "Expected at least 27 tests (T11-T37), got {}",
            manifest.test.len()
        );

        // Verify suite metadata
        assert_eq!(manifest.suite.phase, 1);

        // Verify T11 remove_package
        let t11 = manifest.test.iter().find(|t| t.id == "T11").unwrap();
        assert_eq!(t11.name, "remove_package");
        assert_eq!(t11.timeout, 60);
        let t11_remove = t11.step[0].conary.as_deref().unwrap();
        assert!(
            has_apply_intent(t11_remove),
            "T11 remove must include apply intent"
        );

        // Verify T15 uses stdout_contains_all
        let t15 = manifest.test.iter().find(|t| t.id == "T15").unwrap();
        let a15 = t15.step[0].assert.as_ref().unwrap();
        assert!(
            a15.stdout_contains_all.is_some(),
            "T15 should use stdout_contains_all"
        );
        let all = a15.stdout_contains_all.as_ref().unwrap();
        assert_eq!(all.len(), 2);

        // Verify T27 uses stdout_contains_all with 3 entries
        let t27 = manifest.test.iter().find(|t| t.id == "T27").unwrap();
        let a27 = t27.step[0].assert.as_ref().unwrap();
        let all27 = a27.stdout_contains_all.as_ref().unwrap();
        assert_eq!(all27.len(), 3);

        // Verify ownership-mode install tests are independent of earlier installs.
        for (id, package, ownership) in [
            ("T28", "${TEST_PACKAGE_1}", "--ownership preserve"),
            ("T30", "${TEST_PACKAGE_3}", "--ownership takeover"),
        ] {
            let test = manifest.test.iter().find(|t| t.id == id).unwrap();
            let pre_remove = test.step[0].run.as_deref().unwrap_or("");
            assert!(
                pre_remove.contains(" remove ") && pre_remove.contains(package),
                "{id} should remove {package} before reinstalling"
            );
            assert!(
                has_apply_intent(pre_remove)
                    && pre_remove.contains("--db-path ${DB_PATH}")
                    && pre_remove.contains("|| true"),
                "{id} pre-remove should be best-effort, use the test DB, and include apply intent"
            );

            let install = test.step[1].conary.as_deref().unwrap_or("");
            assert!(
                install.contains(ownership),
                "{id} should keep its ownership install assertion"
            );
        }

        // Verify T33 uses run (no_db generation command)
        let t33 = manifest.test.iter().find(|t| t.id == "T33").unwrap();
        assert!(
            t33.step[0].run.is_some(),
            "T33 should use run (no_db generation command)"
        );

        // Verify T35 uses stdout_contains_any
        let t35 = manifest.test.iter().find(|t| t.id == "T35").unwrap();
        let a35 = t35.step[0].assert.as_ref().unwrap();
        assert!(
            a35.stdout_contains_any.is_some(),
            "T35 should use stdout_contains_any"
        );
        let any35 = a35.stdout_contains_any.as_ref().unwrap();
        assert_eq!(any35.len(), 3);

        // Verify T34 uses stdout_contains_if_success
        let t34 = manifest.test.iter().find(|t| t.id == "T34").unwrap();
        let a34 = t34.step[0].assert.as_ref().unwrap();
        assert!(
            a34.stdout_contains_if_success.is_some(),
            "T34 should use stdout_contains_if_success"
        );

        // Verify T37 uses stdout_contains_any_if_success
        let t37 = manifest.test.iter().find(|t| t.id == "T37").unwrap();
        let a37 = t37.step[0].assert.as_ref().unwrap();
        assert!(
            a37.stdout_contains_any_if_success.is_some(),
            "T37 should use stdout_contains_any_if_success"
        );
    }
}

#[test]
fn phase4_native_pm_parity_manifest_carries_cross_source_and_daily_driver_contract() {
    let path = remi_manifest_path("phase4-native-pm-parity.toml");
    if !path.exists() {
        return;
    }

    let manifest = load_manifest(&path).unwrap();
    let cross_source = manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM02X")
        .expect("Phase 4 native PM parity must include the cross-source-format matrix");
    let cross_source_rendered = format!("{cross_source:?}");
    for required in [
        "run-cross-source-lifecycle-matrix.sh",
        "install",
        "rollback",
        "remove",
    ] {
        assert!(
            cross_source_rendered.contains(required),
            "cross-source-format matrix should enforce {required}"
        );
    }
    let matrix_script = std::fs::read_to_string(conary_fixture_path(
        "native/run-cross-source-lifecycle-matrix.sh",
    ))
    .expect("read cross-source lifecycle helper");
    for required in [
        "all_source_formats=(rpm deb arch)",
        "for source_format in \"${source_formats[@]}\"",
        "capture-native-lifecycle-oracle.sh",
        "native-lifecycle-parity",
        "source_profile=\"fedora-44\"",
        "source_profile=\"ubuntu-26.04\"",
        "source_profile=\"arch\"",
        "--from \"${source_profile}\"",
        "expected_trace_digest",
        "assert_trace",
        "forbid-native-pm",
        "native-manager-backups",
        "restore_native_managers",
        "rpmdb",
        "dpkg",
        "pacman",
        "system state rollback",
        "--purge",
        "assert-selected-generation.py",
    ] {
        assert!(
            matrix_script.contains(required),
            "cross-source lifecycle helper should enforce {required}"
        );
    }
    let oracle_script = std::fs::read_to_string(conary_fixture_path(
        "native/capture-native-lifecycle-oracle.sh",
    ))
    .expect("read native lifecycle oracle helper");
    for required in [
        "rpm --install",
        "rpm --upgrade",
        "dpkg --install",
        "pacman --upgrade",
        "capture_and_compare install",
        "capture_and_compare upgrade",
        "capture_and_compare remove",
        "cmp --silent",
        "diff --unified",
    ] {
        assert!(
            oracle_script.contains(required),
            "native lifecycle oracle should enforce {required}"
        );
    }
    for source_format in ["rpm", "deb", "arch"] {
        for operation in ["install", "upgrade", "remove"] {
            let trace_path =
                format!("native-lifecycle-parity/expected/{source_format}/{operation}.trace");
            let trace = std::fs::read_to_string(conary_fixture_path(&trace_path))
                .unwrap_or_else(|error| panic!("read {trace_path}: {error}"));
            assert!(!trace.is_empty(), "{trace_path} must not be empty");
            assert!(
                trace.lines().all(|line| {
                    line.contains("|argc=")
                        && line.contains("|argv=")
                        && line.contains("|stdin=")
                        && line.contains("|payload=")
                }),
                "{trace_path} must contain only normalized lifecycle records"
            );
        }
    }

    let corpus_tests: Vec<_> = manifest
        .test
        .iter()
        .filter(|test| test.group.as_deref() == Some("daily-driver-corpus"))
        .collect();

    assert!(
        corpus_tests.len() >= 5,
        "Phase 4 native PM parity should include a daily-driver corpus group"
    );
    assert!(
        corpus_tests.iter().all(|test| test.skip.is_none()),
        "daily-driver corpus tests must not be manifest-skipped"
    );
    assert!(
        corpus_tests.iter().all(|test| test.flaky != Some(true)),
        "daily-driver corpus tests must not rely on flaky majority voting"
    );

    let rendered = corpus_tests
        .iter()
        .map(|test| format!("{test:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    for required in [
        "phase4-corpus.service",
        "/etc/phase4-corpus/app.conf",
        "phase4-corpus-alt",
        "phase4-corpus-user",
        "phase4-corpus-group",
        "${native_corpus_dependency_probe}",
        "/var/lib/phase4-corpus/scriptlet.marker",
        "/var/lib/phase4-corpus/remove.marker",
        "phase4-corpus-conflict",
        "large-payload.bin",
        "/usr/lib/kernel/install.d/95-phase4-corpus.install",
    ] {
        assert!(
            rendered.contains(required),
            "daily-driver corpus should cover {required}"
        );
    }

    for distro in ["fedora44", "ubuntu-26.04", "arch"] {
        let overrides = manifest
            .distro_overrides
            .get(distro)
            .unwrap_or_else(|| panic!("missing {distro} overrides"));
        for key in [
            "native_corpus_fixture_version",
            "native_corpus_dependency_count",
            "native_corpus_dependency_probe",
            "native_corpus_lifecycle_fidelity",
        ] {
            assert!(
                overrides.contains_key(key),
                "{distro} overrides should define {key}"
            );
        }
    }
}

#[test]
fn focused_native_cross_source_manifest_runs_the_shared_lifecycle_contract() {
    let path = remi_manifest_path("native-cross-source-lifecycle.toml");
    if !path.exists() {
        return;
    }

    let manifest = load_manifest(&path).expect("load focused native lifecycle manifest");
    assert_eq!(manifest.suite.phase, 4);
    assert_eq!(manifest.test.len(), 1);
    let test = &manifest.test[0];
    assert_eq!(test.id, "TNPMX01");
    assert_eq!(test.fatal, Some(true));
    assert_eq!(test.skip, None);
    assert_eq!(test.flaky, None);

    let rendered = format!("{test:?}");
    assert!(
        rendered.contains("run-cross-source-lifecycle-matrix.sh"),
        "focused manifest must execute the shared lifecycle contract"
    );
    assert!(
        rendered.contains("${native_lifecycle_oracle_format}"),
        "focused manifest must pass an explicit typed native-oracle format"
    );
    for (distro, expected_format) in [
        ("fedora44", "rpm"),
        ("ubuntu-26.04", "deb"),
        ("arch", "arch"),
    ] {
        let overrides = manifest
            .distro_overrides
            .get(distro)
            .unwrap_or_else(|| panic!("missing {distro} overrides"));
        assert_eq!(
            overrides
                .get("native_lifecycle_oracle_format")
                .map(String::as_str),
            Some(expected_format),
            "{distro} must select its native lifecycle oracle explicitly"
        );
    }
    for operation in ["install", "update", "rollback", "remove"] {
        assert!(
            test.description.contains(operation),
            "focused manifest should name {operation} in its contract"
        );
    }
}

#[test]
fn phase4_security_advisory_pipeline_manifest_carries_trusted_update_contract() {
    let path = remi_manifest_path("phase4-security-advisory-pipeline.toml");
    let manifest = load_manifest(&path).unwrap();

    assert_eq!(manifest.suite.phase, 4);
    assert!(
        manifest.test.iter().all(|test| test.skip.is_none()),
        "security advisory pipeline tests must not be manifest-skipped"
    );
    assert!(
        manifest.test.iter().all(|test| test.flaky != Some(true)),
        "security advisory pipeline tests must not rely on flaky majority voting"
    );

    let rendered = manifest
        .test
        .iter()
        .map(|test| format!("{test:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    for required in [
        "security_advisory_source",
        "source_trust",
        "fixed_version",
        "CVE-2026-0001",
        "TEST-2026-0001",
        "--security-advisories unknown",
        "--security-advisories supported",
        "trusted source: conary-json",
    ] {
        assert!(
            rendered.contains(required),
            "security advisory pipeline should cover {required}"
        );
    }
}

#[test]
fn active_manifest_live_mutation_commands_acknowledge_mutation_and_protect_lifecycle() {
    let manifest_dir = remi_manifest_path("");
    if !manifest_dir.exists() {
        return;
    }

    for entry in std::fs::read_dir(&manifest_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }

        let manifest = load_manifest(&path).unwrap();
        for test in &manifest.test {
            for (index, step) in test.step.iter().enumerate() {
                for command in step
                    .conary
                    .iter()
                    .chain(step.run.iter())
                    .chain(step.kill_after_log.iter().map(|kill| &kill.conary))
                {
                    for segment in live_mutation_segments(command) {
                        if is_dry_run_segment(segment) {
                            continue;
                        }
                        assert!(
                            has_apply_intent(segment),
                            "{}:{} step {} mutation command must include apply intent: {}",
                            path.display(),
                            test.id,
                            index + 1,
                            segment
                        );
                        if is_package_remove_segment(segment) {
                            assert!(
                                segment.contains("--sandbox always"),
                                "{}:{} step {} scripted remove command must require protected lifecycle execution: {}",
                                path.display(),
                                test.id,
                                index + 1,
                                segment
                            );
                        }
                    }
                }
            }
        }
    }
}

mod dependabot;
mod manifests;
mod workflow;
