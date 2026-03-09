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
    pub file_checksum: Option<FileChecksum>,
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
    FileChecksum(FileChecksum),
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
        } else if let Some(chk) = &self.file_checksum {
            Some(StepType::FileChecksum(chk.clone()))
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
    pub stdout_contains: Option<String>,
    #[serde(default)]
    pub stdout_not_contains: Option<String>,
    #[serde(default)]
    pub stderr_contains: Option<String>,
    #[serde(default)]
    pub file_exists: Option<String>,
    #[serde(default)]
    pub file_not_exists: Option<String>,
    #[serde(default)]
    pub file_checksum: Option<FileChecksum>,
}
