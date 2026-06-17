// conary-core/src/recipe/recording/report.rs

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectedBackend {
    FanotifyInotify,
    Fanotify,
    Inotify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TraceScope {
    Source,
    Work,
    Install,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScopeRootLabel {
    Source,
    Work,
    Install,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TraceOperation {
    SourceRead,
    SourceWrite,
    WorkRead,
    WorkWrite,
    InstallCreate,
    InstallModify,
    InstallDelete,
    OutOfScope,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedPath {
    pub scope: TraceScope,
    pub operation: TraceOperation,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledFileEvidence {
    pub path: String,
    pub file_type: String,
    pub executable: bool,
    pub size: u64,
    pub link_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySuggestion {
    pub capability: String,
    pub confidence: String,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredEvent {
    pub reason: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordingLimitation {
    IncompleteReadEvidence,
    EventLoss,
    NetworkNotObserved,
    CommandFailed,
    ValidationSkipped,
    ValidationFailed,
    UnsafeHost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingReport {
    pub schema_version: u16,
    pub operation_id: String,
    pub backend: SelectedBackend,
    pub scope_roots: Vec<ScopeRootLabel>,
    pub command_summary: Vec<String>,
    pub command_exit: Option<i32>,
    pub observed_paths: Vec<ObservedPath>,
    pub installed_files: Vec<InstalledFileEvidence>,
    pub inferred_build_steps: Vec<String>,
    pub inferred_install_steps: Vec<String>,
    pub capability_suggestions: Vec<CapabilitySuggestion>,
    pub ignored_events: Vec<IgnoredEvent>,
    pub redactions: Vec<String>,
    pub limitations: Vec<RecordingLimitation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeRoot {
    pub scope: TraceScope,
    pub root: PathBuf,
}

impl ScopeRoot {
    pub fn new(scope: TraceScope, root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let root = root
            .canonicalize()
            .with_context(|| format!("failed to canonicalize trace root {}", root.display()))?;
        Ok(Self { scope, root })
    }

    pub fn scope_path(
        &self,
        path: impl AsRef<Path>,
        operation: TraceOperation,
    ) -> Result<ObservedPath> {
        let path = path.as_ref();
        let relative = path
            .strip_prefix(&self.root)
            .with_context(|| format!("path {} is outside trace scope", path.display()))?;
        if relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            bail!("path {} is outside trace scope", path.display());
        }
        Ok(ObservedPath {
            scope: self.scope,
            operation,
            path: relative
                .to_string_lossy()
                .trim_start_matches('/')
                .to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_path_rejects_private_prefix_leaks() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        let other_root = temp.path().join("other");
        std::fs::create_dir_all(source_root.join("src")).unwrap();
        std::fs::create_dir_all(&other_root).unwrap();
        let source = ScopeRoot::new(TraceScope::Source, &source_root).unwrap();
        let scoped = source
            .scope_path(source_root.join("src/main.rs"), TraceOperation::SourceRead)
            .unwrap();
        assert_eq!(scoped.scope, TraceScope::Source);
        assert_eq!(scoped.operation, TraceOperation::SourceRead);
        assert_eq!(scoped.path, "src/main.rs");

        let error = source
            .scope_path(other_root.join("secret"), TraceOperation::SourceRead)
            .unwrap_err();
        assert!(error.to_string().contains("outside trace scope"));
    }

    #[test]
    fn report_serializes_backend_limitations_and_scope_relative_paths() {
        let report = RecordingReport {
            schema_version: 1,
            operation_id: "record-1".to_string(),
            backend: SelectedBackend::Inotify,
            scope_roots: vec![ScopeRootLabel::Source],
            command_summary: vec!["make".to_string(), "install".to_string()],
            command_exit: Some(0),
            observed_paths: vec![ObservedPath {
                scope: TraceScope::Install,
                operation: TraceOperation::InstallCreate,
                path: "usr/bin/demo".to_string(),
            }],
            installed_files: vec![InstalledFileEvidence {
                path: "usr/bin/demo".to_string(),
                file_type: "file".to_string(),
                executable: true,
                size: 12,
                link_target: None,
            }],
            inferred_build_steps: Vec::new(),
            inferred_install_steps: vec!["make install DESTDIR=%(destdir)s".to_string()],
            capability_suggestions: Vec::new(),
            ignored_events: vec![IgnoredEvent {
                reason: "out-of-scope".to_string(),
                count: 2,
            }],
            redactions: Vec::new(),
            limitations: vec![RecordingLimitation::IncompleteReadEvidence],
        };

        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"backend\":\"inotify\""));
        assert!(json.contains("\"incomplete-read-evidence\""));
        assert!(!json.contains("/tmp/conary-record"));
    }
}
