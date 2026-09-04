// apps/conary-test/src/config/manifest/tests/validation_tests.rs

use super::*;

fn corpus_manifest(suite_corpus: &str, semantic: &str) -> TestManifest {
    toml::from_str(&format!(
        r#"
[suite]
name = "corpus"
phase = 4
{suite_corpus}

[[test]]
id = "TC01"
name = "corpus"
description = "typed corpus"
timeout = 30

[test.corpus]
evidence_path = "/tmp/corpus.json"
source_profile = "fedora-44"
source_format = "rpm"
digest_source = "fixture_build_manifest"
stages = ["installation"]

[test.corpus.target]
architecture = "x86_64"
init_system = "systemd"
capabilities = ["native_lifecycle"]

[[test.corpus.coverage]]
semantic = "{semantic}"
artifact_roles = ["install_request"]
"#
    ))
    .unwrap()
}

fn base_assertion() -> Assertion {
    Assertion::default()
}

#[test]
fn test_no_conflict_passes() {
    let mut a = base_assertion();
    a.exit_code = Some(0);
    a.stdout_contains = Some("ok".into());
    assert!(a.validate("T01", 0).is_ok());
}

#[test]
fn corpus_cases_require_exact_suite_coverage_authority() {
    let missing_suite = corpus_manifest("", "identity_exact_version");
    assert!(missing_suite.validate().is_err());

    let mismatched = corpus_manifest(
        "[suite.corpus]\nrequired = [\"payload_files\"]",
        "identity_exact_version",
    );
    let error = mismatched.validate().unwrap_err().to_string();
    assert!(error.contains("coverage and case claims disagree"));

    let exact = corpus_manifest(
        "[suite.corpus]\nrequired = [\"identity_exact_version\"]",
        "identity_exact_version",
    );
    assert!(exact.validate().is_ok());
}

#[test]
fn unknown_free_form_semantic_is_not_deserializable() {
    let source = r#"
[suite]
name = "corpus"
phase = 4

[suite.corpus]
required = ["package_seems_complex"]

[[test]]
id = "TC01"
name = "corpus"
description = "typed corpus"
timeout = 30
"#;
    assert!(toml::from_str::<TestManifest>(source).is_err());
}

#[test]
fn test_conflicting_exit_code() {
    let mut a = base_assertion();
    a.exit_code = Some(0);
    a.exit_code_not = Some(0);
    let err = a.validate("T01", 0).unwrap_err();
    assert!(err.to_string().contains("conflicting"));
}

#[test]
fn test_different_exit_codes_ok() {
    let mut a = base_assertion();
    a.exit_code = Some(0);
    a.exit_code_not = Some(1);
    assert!(a.validate("T01", 0).is_ok());
}

#[test]
fn test_conflicting_stdout_contains() {
    let mut a = base_assertion();
    a.stdout_contains = Some("hello".into());
    a.stdout_not_contains = Some("hello".into());
    let err = a.validate("T01", 0).unwrap_err();
    assert!(err.to_string().contains("conflicting"));
}

#[test]
fn test_different_stdout_contains_ok() {
    let mut a = base_assertion();
    a.stdout_contains = Some("hello".into());
    a.stdout_not_contains = Some("error".into());
    assert!(a.validate("T01", 0).is_ok());
}

#[test]
fn test_conflicting_stdout_contains_all_vs_not() {
    let mut a = base_assertion();
    a.stdout_contains_all = Some(vec!["foo".into(), "bar".into()]);
    a.stdout_not_contains = Some("bar".into());
    let err = a.validate("T01", 0).unwrap_err();
    assert!(err.to_string().contains("conflicting"));
}

#[test]
fn test_conflicting_file_exists() {
    let mut a = base_assertion();
    a.file_exists = Some("/tmp/test".into());
    a.file_not_exists = Some("/tmp/test".into());
    let err = a.validate("T01", 0).unwrap_err();
    assert!(err.to_string().contains("conflicting"));
}

#[test]
fn test_manifest_validate_catches_conflict() {
    let toml = r#"
[suite]
name = "bad"
phase = 1

[[test]]
id = "T01"
name = "conflicting"
description = "Has conflicting assertions"
timeout = 10

[[test.step]]
run = "echo hello"

[test.step.assert]
stdout_contains = "hello"
stdout_not_contains = "hello"
"#;
    let manifest: TestManifest = toml::from_str(toml).unwrap();
    let err = manifest.validate().unwrap_err();
    assert!(err.to_string().contains("conflicting"));
    assert!(err.to_string().contains("T01"));
}

#[test]
fn test_manifest_validate_rejects_unknown_requirement() {
    let toml = r#"
[suite]
name = "bad-requirement"
phase = 1

[[test]]
id = "T01"
name = "requires"
description = "Has an unknown requirement"
timeout = 10
requires = ["missing-magic"]

[[test.step]]
run = "echo hello"
"#;
    let manifest: TestManifest = toml::from_str(toml).unwrap();
    let err = manifest.validate().unwrap_err();
    assert!(err.to_string().contains("unknown runtime requirement"));
    assert!(err.to_string().contains("T01"));
}

#[test]
fn test_assertion_stderr_not_contains_parses() {
    let toml = r#"
[suite]
name = "stderr-not-contains"
phase = 4

[[test]]
id = "T01"
name = "stderr_guard"
description = "Rejects forbidden stderr text"
timeout = 10

[[test.step]]
run = "true"

[test.step.assert]
stderr_not_contains = "panic"
"#;
    let manifest: TestManifest = toml::from_str(toml).unwrap();
    assert_eq!(
        manifest.test[0].step[0]
            .assert
            .as_ref()
            .and_then(|assertion| assertion.stderr_not_contains.as_deref()),
        Some("panic")
    );
}

#[test]
fn test_unknown_assertion_field_is_rejected() {
    let toml = r#"
[suite]
name = "unknown-assertion"
phase = 4

[[test]]
id = "T01"
name = "bad_assertion"
description = "Has an unsupported assertion key"
timeout = 10

[[test.step]]
run = "true"

[test.step.assert]
stderr_never_contains = "panic"
"#;
    let err = toml::from_str::<TestManifest>(toml).unwrap_err();
    assert!(err.to_string().contains("unknown field"));
    assert!(err.to_string().contains("stderr_never_contains"));
}

#[test]
fn test_retry_delay_ms_parses() {
    let toml = r#"
[suite]
name = "retry-delay"
phase = 1

[[test]]
id = "T01"
name = "with_delay"
description = "Has retry delay"
timeout = 30
flaky = true
retries = 3
retry_delay_ms = 500

[[test.step]]
run = "echo ok"
"#;
    let manifest: TestManifest = toml::from_str(toml).unwrap();
    assert_eq!(manifest.test[0].retry_delay_ms, Some(500));
}

#[test]
fn test_retry_delay_ms_defaults_to_none() {
    let toml = r#"
[suite]
name = "no-delay"
phase = 1

[[test]]
id = "T01"
name = "no_delay"
description = "No retry delay"
timeout = 30

[[test.step]]
run = "echo ok"
"#;
    let manifest: TestManifest = toml::from_str(toml).unwrap();
    assert!(manifest.test[0].retry_delay_ms.is_none());
}

#[test]
fn test_suite_timeout_parses() {
    let toml = r#"
[suite]
name = "timed"
phase = 1
timeout = 300

[[test]]
id = "T01"
name = "test"
description = "A test"
timeout = 30

[[test.step]]
run = "echo ok"
"#;
    let manifest: TestManifest = toml::from_str(toml).unwrap();
    assert_eq!(manifest.suite.timeout, Some(300));
}

#[test]
fn test_step_timeout_override_parses() {
    let toml = r#"
[suite]
name = "step-timeout"
phase = 1

[[test]]
id = "T01"
name = "step_timeout"
description = "Step with timeout"
timeout = 30

[[test.step]]
timeout = 60
run = "long-running-command"
"#;
    let manifest: TestManifest = toml::from_str(toml).unwrap();
    assert_eq!(manifest.test[0].step[0].timeout, Some(60));
}

#[test]
fn test_step_timeout_defaults_to_none() {
    let toml = r#"
[suite]
name = "no-step-timeout"
phase = 1

[[test]]
id = "T01"
name = "default_step"
description = "Step without timeout"
timeout = 30

[[test.step]]
run = "echo ok"
"#;
    let manifest: TestManifest = toml::from_str(toml).unwrap();
    assert!(manifest.test[0].step[0].timeout.is_none());
}

#[test]
fn test_qemu_boot_step_type() {
    let toml = r#"
[suite]
name = "qemu"
phase = 3

[[test]]
id = "T156"
name = "qemu_boot"
description = "Boot a qcow2 image"
timeout = 30

[[test.step]]
[test.step.qemu_boot]
image = "minimal-boot-v1"
commands = ["uname -r"]
"#;

    let manifest: TestManifest = toml::from_str(toml).unwrap();
    let step = &manifest.test[0].step[0];
    match step.step_type() {
        Some(StepType::QemuBoot(cfg)) => {
            assert_eq!(cfg.image, "minimal-boot-v1");
            assert_eq!(cfg.memory_mb, 1024);
            assert_eq!(cfg.timeout_seconds, 300);
            assert_eq!(cfg.ssh_port, 2222);
            assert_eq!(cfg.commands, vec!["uname -r"]);
        }
        other => panic!("expected qemu_boot step, got {other:?}"),
    }
}

#[test]
fn test_qemu_boot_local_image_and_copy_fields_parse() {
    let toml = r#"
[suite]
name = "qemu-local"
phase = 3

[[test]]
id = "TGE01"
name = "qemu_boot_local"
description = "Boot a generated qcow2 image"
timeout = 30

[[test.step]]
[test.step.qemu_boot]
image = "minimal-boot-v2"
local_image_path = "/tmp/generated.qcow2"
stage_conary = true
scratch_disk_mb = 8192
copy_to_guest = [
  { source = "apps/conary/tests/fixtures/supported-host-generation-export", dest = "/var/lib/conary/bootstrap-inputs" },
]
copy_from_guest = [
  { source = "/tmp/out.qcow2", dest = "/tmp/conary-generation-export/host-out.qcow2" },
]
commands = ["true"]
"#;

    let manifest: TestManifest = toml::from_str(toml).unwrap();
    let step = &manifest.test[0].step[0];
    match step.step_type() {
        Some(StepType::QemuBoot(cfg)) => {
            assert_eq!(cfg.image, "minimal-boot-v2");
            assert_eq!(
                cfg.local_image_path.as_deref(),
                Some("/tmp/generated.qcow2")
            );
            assert!(cfg.stage_conary);
            assert_eq!(cfg.scratch_disk_mb, Some(8192));
            assert_eq!(cfg.copy_to_guest.len(), 1);
            assert_eq!(
                cfg.copy_to_guest[0].source,
                "apps/conary/tests/fixtures/supported-host-generation-export"
            );
            assert_eq!(
                cfg.copy_to_guest[0].dest,
                "/var/lib/conary/bootstrap-inputs"
            );
            assert_eq!(cfg.copy_from_guest.len(), 1);
            assert_eq!(cfg.copy_from_guest[0].source, "/tmp/out.qcow2");
            assert_eq!(
                cfg.copy_from_guest[0].dest,
                "/tmp/conary-generation-export/host-out.qcow2"
            );
        }
        other => panic!("expected qemu_boot step, got {other:?}"),
    }
}
