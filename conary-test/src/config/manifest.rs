// conary-test/src/config/manifest.rs

use serde::Deserialize;

/// Top-level test manifest (one TOML file = one suite).
#[derive(Debug, Clone, Deserialize)]
pub struct TestManifest {
    pub suite: SuiteDef,
    pub test: Vec<TestDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SuiteDef {
    pub name: String,
    pub phase: u32,
    #[serde(default)]
    pub setup: Vec<TestStep>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TestDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub timeout: u64,
    #[serde(default)]
    pub step: Vec<TestStep>,
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
    #[serde(default)]
    pub fatal: Option<bool>,
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TestStep {
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub conary: Option<String>,
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
