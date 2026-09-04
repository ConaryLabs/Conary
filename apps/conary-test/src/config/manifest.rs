// apps/conary-test/src/config/manifest.rs

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Top-level test manifest (one TOML file = one suite).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestManifest {
    pub suite: SuiteDef,
    pub test: Vec<TestDef>,
    #[serde(default)]
    pub distro_overrides: HashMap<String, HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Required typed semantic coverage for corpus suites.
    #[serde(default)]
    pub corpus: Option<super::corpus::CorpusSuiteDef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// When set, the test is skipped with this reason string.
    #[serde(default)]
    pub skip: Option<String>,
    /// Runtime capabilities required inside the test container.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Attributable just-works corpus authority for this test.
    #[serde(default)]
    pub corpus: Option<super::corpus::CorpusCaseDef>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
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

impl TestManifest {
    /// Returns true if every step in the manifest is a `qemu_boot` step.
    ///
    /// QEMU-only suites do not need a container runtime — they boot their
    /// own VMs. The CLI uses this to skip container image build/start.
    pub fn is_qemu_only(&self) -> bool {
        let has_tests = !self.test.is_empty();
        has_tests
            && self
                .test
                .iter()
                .all(|t| !t.step.is_empty() && t.step.iter().all(|s| s.qemu_boot.is_some()))
    }
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
#[serde(deny_unknown_fields)]
pub struct FileChecksum {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct QemuGuestCopy {
    pub source: String,
    pub dest: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QemuImageFormat {
    #[default]
    Qcow2,
    Raw,
    Iso,
}

impl QemuImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Qcow2 => "qcow2",
            Self::Raw => "raw",
            Self::Iso => "iso",
        }
    }

    pub fn qemu_drive_format(self) -> Option<&'static str> {
        match self {
            Self::Qcow2 => Some("qcow2"),
            Self::Raw => Some("raw"),
            Self::Iso => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QemuBoot {
    pub image: String,
    #[serde(default)]
    pub local_image_path: Option<String>,
    #[serde(default)]
    pub image_format: QemuImageFormat,
    #[serde(default)]
    pub stage_conary: bool,
    #[serde(default)]
    pub scratch_disk_mb: Option<u64>,
    #[serde(default)]
    pub copy_to_guest: Vec<QemuGuestCopy>,
    #[serde(default)]
    pub copy_from_guest: Vec<QemuGuestCopy>,
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
#[serde(deny_unknown_fields)]
pub struct MockServerConfig {
    pub port: u16,
    pub routes: Vec<MockRoute>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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

#[cfg(test)]
#[path = "manifest/tests.rs"]
mod tests;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
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
    pub stderr_not_contains: Option<String>,
    #[serde(default)]
    pub file_exists: Option<String>,
    #[serde(default)]
    pub file_not_exists: Option<String>,
    #[serde(default)]
    pub file_checksum: Option<FileChecksum>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
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
        let corpus_tests = self
            .test
            .iter()
            .filter_map(|test| test.corpus.as_ref())
            .collect::<Vec<_>>();
        match (&self.suite.corpus, corpus_tests.is_empty()) {
            (None, true) => {}
            (None, false) => bail!("corpus tests require suite-level semantic coverage"),
            (Some(_), true) => bail!("suite corpus coverage requires at least one corpus test"),
            (Some(corpus), false) => {
                corpus.validate()?;
                let required = corpus.required.iter().copied().collect::<HashSet<_>>();
                let claimed = corpus_tests
                    .iter()
                    .flat_map(|case| case.coverage.iter().map(|claim| claim.semantic))
                    .collect::<HashSet<_>>();
                if claimed != required {
                    let mut missing = required.difference(&claimed).copied().collect::<Vec<_>>();
                    let mut undeclared = claimed.difference(&required).copied().collect::<Vec<_>>();
                    missing.sort();
                    undeclared.sort();
                    bail!(
                        "suite corpus coverage and case claims disagree: missing={missing:?}, undeclared={undeclared:?}"
                    );
                }
            }
        }
        for test in &self.test {
            if let Some(corpus) = &test.corpus {
                corpus.validate(&test.id)?;
                if test.resources.is_some() {
                    bail!(
                        "test {}: corpus evidence cannot use a resource-scoped disposable container",
                        test.id
                    );
                }
            }
            for requirement in &test.requires {
                if requirement != "composefs_runtime" {
                    bail!(
                        "test {} has unknown runtime requirement `{}`",
                        test.id,
                        requirement
                    );
                }
            }
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
#[path = "manifest/tests/validation_tests.rs"]
mod validation_tests;
