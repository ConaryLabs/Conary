// conary-test/src/config/manifest.rs

use anyhow::{bail, Result};
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

#[derive(Debug, Clone, Default, Deserialize)]
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

impl Assertion {
    /// Validate that the assertion has no conflicting fields.
    ///
    /// Detects cases like setting both `exit_code` and `exit_code_not` to the
    /// same value, or `stdout_contains` and `stdout_not_contains` with the
    /// same string, which would make the assertion impossible to satisfy.
    pub fn validate(&self, test_id: &str, step_index: usize) -> Result<()> {
        let ctx = || format!("test {test_id}, step {step_index}");

        // exit_code vs exit_code_not
        if let (Some(code), Some(not_code)) = (self.exit_code, self.exit_code_not) {
            if code == not_code {
                bail!(
                    "{}: conflicting assertion: exit_code={code} and exit_code_not={not_code}",
                    ctx()
                );
            }
        }

        // stdout_contains vs stdout_not_contains
        if let (Some(contains), Some(not_contains)) =
            (&self.stdout_contains, &self.stdout_not_contains)
        {
            if contains == not_contains {
                bail!(
                    "{}: conflicting assertion: stdout_contains and stdout_not_contains \
                     both set to {:?}",
                    ctx(),
                    contains
                );
            }
        }

        // stdout_contains_all vs stdout_not_contains
        if let (Some(all), Some(not_contains)) =
            (&self.stdout_contains_all, &self.stdout_not_contains)
        {
            if all.iter().any(|s| s == not_contains) {
                bail!(
                    "{}: conflicting assertion: stdout_contains_all includes {:?} \
                     which is also set in stdout_not_contains",
                    ctx(),
                    not_contains
                );
            }
        }

        // file_exists vs file_not_exists
        if let (Some(exists), Some(not_exists)) =
            (&self.file_exists, &self.file_not_exists)
        {
            if exists == not_exists {
                bail!(
                    "{}: conflicting assertion: file_exists and file_not_exists \
                     both set to {:?}",
                    ctx(),
                    exists
                );
            }
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
}
