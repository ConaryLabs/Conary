// conary-test/src/config/manifest.rs

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level test manifest (one TOML file = one suite).
#[derive(Debug, Clone, Deserialize)]
pub struct TestManifest {
    pub suite: SuiteDef,
    pub test: Vec<TestDef>,
    #[serde(default)]
    pub distro_overrides: HashMap<String, HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SuiteDef {
    pub name: String,
    pub phase: u32,
    #[serde(default)]
    pub setup: Vec<TestStep>,
    #[serde(default)]
    pub mock_server: Option<MockServerConfig>,
    /// Suite-level timeout in seconds. If set, the entire suite must
    /// complete within this duration or remaining tests are cancelled.
    #[serde(default)]
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TestDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub timeout: u64,
    #[serde(default)]
    pub flaky: Option<bool>,
    #[serde(default)]
    pub retries: Option<u32>,
    /// Delay in milliseconds between retry attempts (default 0).
    #[serde(default)]
    pub retry_delay_ms: Option<u64>,
    #[serde(default)]
    pub step: Vec<TestStep>,
    #[serde(default)]
    pub resources: Option<ResourceConstraints>,
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
    #[serde(default)]
    pub fatal: Option<bool>,
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TestStep {
    /// Per-step timeout override in seconds. Falls back to the test-level
    /// timeout when absent.
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub conary: Option<String>,
    #[serde(default)]
    pub kill_after_log: Option<KillAfterLog>,
    #[serde(default)]
    pub qemu_boot: Option<QemuBoot>,
    #[serde(default)]
    pub file_exists: Option<String>,
    #[serde(default)]
    pub file_not_exists: Option<String>,
    #[serde(default)]
    pub file_executable: Option<String>,
    #[serde(default)]
    pub file_checksum: Option<FileChecksum>,
    #[serde(default)]
    pub dir_exists: Option<String>,
    #[serde(default)]
    pub sleep: Option<u64>,
    #[serde(default)]
    pub assert: Option<Assertion>,
}

/// Derive step type from which field is populated.
#[derive(Debug, Clone)]
pub enum StepType {
    Run(String),
    Conary(String),
    KillAfterLog(KillAfterLog),
    QemuBoot(QemuBoot),
    FileExists(String),
    FileNotExists(String),
    FileExecutable(String),
    FileChecksum(FileChecksum),
    DirExists(String),
    Sleep(u64),
}

impl TestStep {
    pub fn step_type(&self) -> Option<StepType> {
        if let Some(cmd) = &self.run {
            Some(StepType::Run(cmd.clone()))
        } else if let Some(cmd) = &self.conary {
            Some(StepType::Conary(cmd.clone()))
        } else if let Some(config) = &self.kill_after_log {
            Some(StepType::KillAfterLog(config.clone()))
        } else if let Some(config) = &self.qemu_boot {
            Some(StepType::QemuBoot(config.clone()))
        } else if let Some(path) = &self.file_exists {
            Some(StepType::FileExists(path.clone()))
        } else if let Some(path) = &self.file_not_exists {
            Some(StepType::FileNotExists(path.clone()))
        } else if let Some(path) = &self.file_executable {
            Some(StepType::FileExecutable(path.clone()))
        } else if let Some(chk) = &self.file_checksum {
            Some(StepType::FileChecksum(chk.clone()))
        } else if let Some(path) = &self.dir_exists {
            Some(StepType::DirExists(path.clone()))
        } else {
            self.sleep.map(StepType::Sleep)
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileChecksum {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KillAfterLog {
    pub conary: String,
    pub pattern: String,
    #[serde(default = "default_kill_timeout")]
    pub timeout_seconds: u64,
}

fn default_kill_timeout() -> u64 {
    60
}

#[derive(Debug, Clone, Deserialize)]
pub struct QemuBoot {
    pub image: String,
    #[serde(default = "default_qemu_memory")]
    pub memory_mb: u32,
    #[serde(default = "default_qemu_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    pub commands: Vec<String>,
    #[serde(default)]
    pub expect_output: Vec<String>,
}

fn default_qemu_memory() -> u32 {
    1024
}

fn default_qemu_timeout() -> u64 {
    300
}

fn default_ssh_port() -> u16 {
    2222
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MockServerConfig {
    pub port: u16,
    pub routes: Vec<MockRoute>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MockRoute {
    pub path: String,
    pub status: u16,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub body_file: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub delay_ms: Option<u64>,
    #[serde(default)]
    pub truncate_at_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Assertion {
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub exit_code_not: Option<i32>,
    #[serde(default)]
    pub stdout_contains: Option<String>,
    #[serde(default)]
    pub stdout_not_contains: Option<String>,
    /// All strings must appear in stdout.
    #[serde(default)]
    pub stdout_contains_all: Option<Vec<String>>,
    /// At least one string must appear in stdout.
    #[serde(default)]
    pub stdout_contains_any: Option<Vec<String>>,
    /// Check stdout contains this string only when exit code is 0.
    /// Non-zero exit is silently accepted (no assertion failure).
    #[serde(default)]
    pub stdout_contains_if_success: Option<String>,
    /// Check stdout contains any of these strings only when exit code is 0.
    /// Non-zero exit is silently accepted (no assertion failure).
    #[serde(default)]
    pub stdout_contains_any_if_success: Option<Vec<String>>,
    #[serde(default)]
    pub stderr_contains: Option<String>,
    #[serde(default)]
    pub file_exists: Option<String>,
    #[serde(default)]
    pub file_not_exists: Option<String>,
    #[serde(default)]
    pub file_checksum: Option<FileChecksum>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResourceConstraints {
    #[serde(default)]
    pub tmpfs_size_mb: Option<u64>,
    #[serde(default)]
    pub memory_limit_mb: Option<u64>,
    #[serde(default)]
    pub network_isolated: Option<bool>,
}

impl Assertion {
    /// Validate that the assertion has no conflicting fields.
    ///
    /// Detects cases like setting both `exit_code` and `exit_code_not` to the
    /// same value, or `stdout_contains` and `stdout_not_contains` with the
    /// same string, which would make the assertion impossible to satisfy.
    pub fn validate(&self, test_id: &str, step_index: usize) -> Result<()> {
        let ctx = || format!("test {test_id}, step {step_index}");

        // exit_code vs exit_code_not
        if let (Some(code), Some(not_code)) = (self.exit_code, self.exit_code_not)
            && code == not_code
        {
            bail!(
                "{}: conflicting assertion: exit_code={code} and exit_code_not={not_code}",
                ctx()
            );
        }

        // stdout_contains vs stdout_not_contains
        if let (Some(contains), Some(not_contains)) =
            (&self.stdout_contains, &self.stdout_not_contains)
            && contains == not_contains
        {
            bail!(
                "{}: conflicting assertion: stdout_contains and stdout_not_contains \
                 both set to {:?}",
                ctx(),
                contains
            );
        }

        // stdout_contains_all vs stdout_not_contains
        if let (Some(all), Some(not_contains)) =
            (&self.stdout_contains_all, &self.stdout_not_contains)
            && all.iter().any(|s| s == not_contains)
        {
            bail!(
                "{}: conflicting assertion: stdout_contains_all includes {:?} \
                 which is also set in stdout_not_contains",
                ctx(),
                not_contains
            );
        }

        // file_exists vs file_not_exists
        if let (Some(exists), Some(not_exists)) = (&self.file_exists, &self.file_not_exists)
            && exists == not_exists
        {
            bail!(
                "{}: conflicting assertion: file_exists and file_not_exists \
                 both set to {:?}",
                ctx(),
                exists
            );
        }

        Ok(())
    }
}

impl TestManifest {
    /// Validate all assertions in the manifest for conflicting fields.
    pub fn validate(&self) -> Result<()> {
        for test in &self.test {
            for (i, step) in test.step.iter().enumerate() {
                if let Some(ref assertion) = step.assert {
                    assertion.validate(&test.id, i)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

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
}
