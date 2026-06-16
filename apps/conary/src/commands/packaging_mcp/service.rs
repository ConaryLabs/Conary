// apps/conary/src/commands/packaging_mcp/service.rs
//! Read-only packaging agent service methods.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conary_agent_contract::{
    AgentError, AgentErrorKind, EvidenceItem, EvidenceKind, ExplainResult, InspectResult,
    OperationEnvelope, OperationStatus, RiskLevel, resource,
};
use conary_core::recipe::inference::{InferenceOptions, infer_recipe_from_path};
use conary_core::recipe::{parse_recipe_file, validate_recipe};

use super::records;
use super::types::{
    DiagnoseLatestFailureInput, ExplainInferenceInput, InspectProjectData, InspectProjectInput,
    OperationRecordData, OperationRecordsListInput, OperationRecordsReadInput,
};

#[derive(Debug, Clone)]
pub(crate) struct PackagingAgentService {
    operations_dir: PathBuf,
}

impl Default for PackagingAgentService {
    fn default() -> Self {
        let operations_dir = super::super::operation_records::default_packaging_operations_dir()
            .unwrap_or_else(|_| PathBuf::from(".conary-packaging-operations"));
        Self { operations_dir }
    }
}

impl PackagingAgentService {
    #[cfg(test)]
    pub(crate) fn with_operations_dir(operations_dir: PathBuf) -> Self {
        Self { operations_dir }
    }

    #[cfg(test)]
    pub(crate) fn operations_dir(&self) -> &Path {
        &self.operations_dir
    }

    pub(crate) fn inspect_project(&self, input: InspectProjectInput) -> Result<InspectResult> {
        let target = Path::new(&input.target);
        let recipe_path = input
            .recipe
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| explicit_recipe_path(target));

        if let Some(recipe_path) = recipe_path {
            return self.inspect_recipe_file(&recipe_path);
        }

        if target.is_dir() {
            return self.inspect_inferred_source_tree(target);
        }

        Ok(unavailable_inspect_result(
            "conary.packaging.inspect_project",
            "Project inspection needs a local recipe file or source directory",
            AgentErrorKind::MissingPrerequisite,
            format!(
                "target {} is not a local recipe file or source directory",
                input.target
            ),
        ))
    }

    pub(crate) fn explain_inference(&self, input: ExplainInferenceInput) -> Result<ExplainResult> {
        let target = Path::new(&input.target);
        if looks_remote_or_archive_target(&input.target) || !target.is_dir() {
            return Ok(ExplainResult::new(failed_envelope(
                "conary.packaging.explain_inference",
                "Inference explanation is only available for local source directories",
                AgentErrorKind::NotSupported,
                "remote, archive, and non-directory targets are not inspected by the read-only MCP surface",
            )));
        }

        let source_root = target.canonicalize()?;
        let inference = infer_recipe_from_path(
            &source_root,
            InferenceOptions::for_source_root(&source_root),
        )?;
        let subject = resource::packaging_project(&inference.recipe.package.name);
        let mut envelope = OperationEnvelope::new(
            "conary.packaging.explain_inference",
            OperationStatus::Ok,
            RiskLevel::ReadOnly,
            "Explained local recipe inference",
        );
        envelope.subject = Some(subject);
        envelope.evidence.push(EvidenceItem {
            kind: EvidenceKind::Check,
            summary: "local source tree inspected without building".to_string(),
            uri: None,
            path: Some(source_root.display().to_string()),
            id: None,
            command: None,
            exit_code: None,
            metadata: Default::default(),
            redactions: Vec::new(),
        });

        Ok(ExplainResult::new(envelope).with_data(serde_json::json!({
            "package_name": inference.recipe.package.name,
            "package_version": inference.recipe.package.version,
            "trace": inference.trace,
            "trace_text": inference.trace.render_human(),
        })))
    }

    pub(crate) fn diagnose_latest_failure(
        &self,
        input: DiagnoseLatestFailureInput,
    ) -> Result<InspectResult> {
        let Some(mut record) = records::latest_failed_record(&self.operations_dir)? else {
            return Ok(unavailable_inspect_result(
                "conary.packaging.diagnose_latest_failure",
                "No failed packaging operation record found",
                AgentErrorKind::MissingPrerequisite,
                "no failed packaging operation record exists in the local private record store",
            ));
        };
        if let Some(limit) = input.limit_events {
            record.events.truncate(limit);
        }
        let operation_id = record.operation_id.clone();
        let mut envelope = OperationEnvelope::new(
            "conary.packaging.diagnose_latest_failure",
            OperationStatus::Ok,
            RiskLevel::ReadOnly,
            "Loaded newest failed packaging operation record",
        );
        envelope.subject = Some(resource::packaging_operation(&operation_id));
        Ok(InspectResult::new(envelope).with_data(serde_json::json!({
            "operation_id": operation_id,
            "record": record,
        })))
    }

    pub(crate) fn list_operation_records(
        &self,
        input: OperationRecordsListInput,
    ) -> Result<InspectResult> {
        let limit = input.limit.unwrap_or(20).min(50);
        let records = records::list_recent_records(&self.operations_dir, limit)?;
        let mut envelope = OperationEnvelope::new(
            "conary.packaging.operation_records.list",
            OperationStatus::Ok,
            RiskLevel::ReadOnly,
            "Listed packaging operation records",
        );
        envelope.subject = Some(resource::packaging_operations_recent());
        Ok(InspectResult::new(envelope).with_data(serde_json::json!({ "records": records })))
    }

    pub(crate) fn read_operation_record(
        &self,
        input: OperationRecordsReadInput,
    ) -> Result<InspectResult> {
        let Some(record) = records::read_record(&self.operations_dir, &input.operation_id)? else {
            return Ok(unavailable_inspect_result(
                "conary.packaging.operation_records.read",
                "Packaging operation record was not found",
                AgentErrorKind::MissingPrerequisite,
                format!("operation record {} does not exist", input.operation_id),
            ));
        };
        let operation_id = record.operation_id.clone();
        let mut value = serde_json::to_value(record)?;
        if !input.include_events
            && let Some(object) = value.as_object_mut()
        {
            object.remove("events");
        }
        let mut envelope = OperationEnvelope::new(
            "conary.packaging.operation_records.read",
            OperationStatus::Ok,
            RiskLevel::ReadOnly,
            "Read packaging operation record",
        );
        envelope.subject = Some(resource::packaging_operation(&operation_id));
        let data = OperationRecordData {
            operation_id,
            record: value,
        };
        Ok(InspectResult::new(envelope).with_data(serde_json::to_value(data)?))
    }

    fn inspect_recipe_file(&self, recipe_path: &Path) -> Result<InspectResult> {
        let recipe_path = recipe_path
            .canonicalize()
            .with_context(|| format!("canonicalize recipe {}", recipe_path.display()))?;
        let recipe = parse_recipe_file(&recipe_path)
            .with_context(|| format!("parse recipe {}", recipe_path.display()))?;
        let warnings = validate_recipe(&recipe).context("validate recipe")?;
        let subject = resource::packaging_project(&recipe.package.name);
        let mut envelope = OperationEnvelope::new(
            "conary.packaging.inspect_project",
            OperationStatus::Ok,
            RiskLevel::ReadOnly,
            "Inspected local packaging recipe",
        );
        envelope.subject = Some(subject.clone());
        envelope.warnings = warnings;
        envelope.evidence.push(EvidenceItem {
            kind: EvidenceKind::Check,
            summary: "recipe parsed and validated without building".to_string(),
            uri: None,
            path: Some(recipe_path.display().to_string()),
            id: None,
            command: None,
            exit_code: None,
            metadata: Default::default(),
            redactions: Vec::new(),
        });
        let data = InspectProjectData {
            target_kind: "recipe".to_string(),
            subject,
            recipe_path: Some(recipe_path.display().to_string()),
            package_name: Some(recipe.package.name),
            package_version: Some(recipe.package.version),
        };
        Ok(InspectResult::new(envelope).with_data(serde_json::to_value(data)?))
    }

    fn inspect_inferred_source_tree(&self, source_root: &Path) -> Result<InspectResult> {
        let source_root = source_root.canonicalize()?;
        let inference = infer_recipe_from_path(
            &source_root,
            InferenceOptions::for_source_root(&source_root),
        )?;
        let subject = resource::packaging_project(&inference.recipe.package.name);
        let mut envelope = OperationEnvelope::new(
            "conary.packaging.inspect_project",
            OperationStatus::Ok,
            RiskLevel::ReadOnly,
            "Inspected local source tree recipe inference",
        );
        envelope.subject = Some(subject.clone());
        envelope.evidence.push(EvidenceItem {
            kind: EvidenceKind::Check,
            summary: "source tree inferred without building".to_string(),
            uri: None,
            path: Some(source_root.display().to_string()),
            id: None,
            command: None,
            exit_code: None,
            metadata: Default::default(),
            redactions: Vec::new(),
        });
        let data = InspectProjectData {
            target_kind: "source_tree".to_string(),
            subject,
            recipe_path: None,
            package_name: Some(inference.recipe.package.name),
            package_version: Some(inference.recipe.package.version),
        };
        Ok(InspectResult::new(envelope).with_data(serde_json::to_value(data)?))
    }
}

fn explicit_recipe_path(target: &Path) -> Option<PathBuf> {
    if target.is_file() {
        return Some(target.to_path_buf());
    }
    let recipe = target.join("recipe.toml");
    recipe.is_file().then_some(recipe)
}

fn looks_remote_or_archive_target(target: &str) -> bool {
    target.contains("://")
        || target.ends_with(".tar")
        || target.ends_with(".tar.gz")
        || target.ends_with(".tgz")
}

fn unavailable_inspect_result(
    operation: &str,
    summary: impl Into<String>,
    kind: AgentErrorKind,
    message: impl Into<String>,
) -> InspectResult {
    InspectResult::new(failed_envelope(operation, summary, kind, message))
}

fn failed_envelope(
    operation: &str,
    summary: impl Into<String>,
    kind: AgentErrorKind,
    message: impl Into<String>,
) -> OperationEnvelope {
    let mut envelope = OperationEnvelope::new(
        operation,
        OperationStatus::Failed,
        RiskLevel::ReadOnly,
        summary,
    );
    envelope.error = Some(AgentError {
        kind,
        message: message.into(),
        remediation: None,
    });
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_agent_contract::{OperationStatus, RiskLevel};
    use conary_core::diagnostics::PackagingCommandOutput;

    #[test]
    fn inspect_project_reads_recipe_without_building() {
        let temp = tempfile::TempDir::new().unwrap();
        let recipe = temp.path().join("recipe.toml");
        std::fs::write(
            &recipe,
            r#"
[package]
name = "demo"
version = "0.1.0"
description = "demo"
license = "MIT"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/bin && touch %(destdir)s/usr/bin/demo"
"#,
        )
        .unwrap();

        let service = PackagingAgentService::default();
        let result = service
            .inspect_project(crate::commands::packaging_mcp::types::InspectProjectInput {
                target: recipe.display().to_string(),
                recipe: None,
            })
            .unwrap();

        assert_eq!(result.envelope.status, OperationStatus::Ok);
        assert_eq!(result.envelope.risk, RiskLevel::ReadOnly);
        assert_eq!(result.data["package_name"], "demo");
    }

    #[test]
    fn list_operation_records_reads_private_store() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));
        let output = PackagingCommandOutput::failed(
            "publish-1",
            "conary publish",
            vec![conary_core::diagnostics::PackagingDiagnostic::error(
                conary_core::diagnostics::PackagingPhase::Publish,
                conary_core::diagnostics::PackagingDiagnosticCode::PublishGateFailed,
                "gate failed",
            )],
        );
        crate::commands::operation_records::write_packaging_record_unchecked(
            service.operations_dir(),
            "publish-1",
            &output,
        )
        .unwrap();

        let result = service
            .list_operation_records(
                crate::commands::packaging_mcp::types::OperationRecordsListInput {
                    limit: Some(10),
                },
            )
            .unwrap();

        assert_eq!(result.envelope.status, OperationStatus::Ok);
        assert_eq!(result.data["records"][0]["operation_id"], "publish-1");
    }

    #[test]
    fn diagnose_latest_failure_reads_newest_failed_record_without_stdout() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));
        let ok = PackagingCommandOutput::succeeded("cook-1", "conary cook");
        let failed = PackagingCommandOutput::failed(
            "publish-2",
            "conary publish",
            vec![conary_core::diagnostics::PackagingDiagnostic::error(
                conary_core::diagnostics::PackagingPhase::Publish,
                conary_core::diagnostics::PackagingDiagnosticCode::PublishGateFailed,
                "gate failed",
            )],
        );
        crate::commands::operation_records::write_packaging_record_unchecked(
            service.operations_dir(),
            "cook-1",
            &ok,
        )
        .unwrap();
        crate::commands::operation_records::write_packaging_record_unchecked(
            service.operations_dir(),
            "publish-2",
            &failed,
        )
        .unwrap();

        let result = service
            .diagnose_latest_failure(
                crate::commands::packaging_mcp::types::DiagnoseLatestFailureInput {
                    limit_events: Some(20),
                },
            )
            .unwrap();

        assert_eq!(result.envelope.status, OperationStatus::Ok);
        assert_eq!(result.data["operation_id"], "publish-2");
    }
}
