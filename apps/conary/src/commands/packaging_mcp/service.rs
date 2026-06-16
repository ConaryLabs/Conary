// apps/conary/src/commands/packaging_mcp/service.rs
//! Read-only packaging agent service methods.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use conary_agent_contract::{
    AgentError, AgentErrorKind, ApplyResult, ConfirmationRequirement, EvidenceItem, EvidenceKind,
    ExplainResult, InspectResult, NextAction, OperationEnvelope, OperationStatus, PlanResult,
    RiskLevel, resource,
};
use conary_core::diagnostics::{
    PackagingCommandOutput, PackagingDiagnostic, PackagingDiagnosticCode, PackagingPhase,
};
use conary_core::recipe::inference::{InferenceOptions, infer_recipe_from_path};
use conary_core::recipe::{parse_recipe_file, validate_recipe};
use conary_core::repository::static_repo::RepoLocation;
use conary_core::repository::static_repo::publish_context::{
    StaticArtifactDestinationSnapshot, inspect_artifact_form_static_destination,
};

use super::super::publish::{
    StaticArtifactPublishServiceInput, publish_static_artifact_form_service,
};
use super::projection::{AgentProjectionMode, project_packaging_output};
use super::publish_plan::{PublishPlanMaterial, PublishPlanRegistry, stage_artifact_private};
use super::records;
use super::types::{
    DiagnoseLatestFailureInput, ExplainInferenceInput, InspectProjectData, InspectProjectInput,
    OperationRecordData, OperationRecordsListInput, OperationRecordsReadInput, PublishApplyInput,
    PublishModeInput, PublishPlanInput,
};

#[derive(Clone)]
pub(crate) struct PackagingAgentService {
    operations_dir: PathBuf,
    publish_plans: Arc<Mutex<PublishPlanRegistry>>,
}

impl Default for PackagingAgentService {
    fn default() -> Self {
        let operations_dir = super::super::operation_records::default_packaging_operations_dir()
            .unwrap_or_else(|_| PathBuf::from(".conary-packaging-operations"));
        Self::new(operations_dir)
    }
}

impl PackagingAgentService {
    fn new(operations_dir: PathBuf) -> Self {
        Self {
            operations_dir,
            publish_plans: Arc::new(Mutex::new(PublishPlanRegistry::new(16))),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_operations_dir(operations_dir: PathBuf) -> Self {
        Self::new(operations_dir)
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

    pub(crate) fn plan_publish(&self, input: PublishPlanInput) -> Result<PlanResult> {
        let operation = "conary.packaging.publish.plan";
        let target_route = classify_publish_target(&input.target);
        if matches!(target_route, PublishTargetRoute::RemiRelease) {
            return Ok(failed_plan_result(
                operation,
                OperationStatus::Unavailable,
                "Remi publish apply is not available through M3b packaging MCP",
                AgentErrorKind::RemoteUnavailable,
                "M3b does not resolve bearer tokens or apply Remi publishes",
            )
            .with_data(route_data(&input, "remi_release", "remi")));
        }
        if matches!(target_route, PublishTargetRoute::UnsupportedHttp) {
            return Ok(failed_plan_result(
                operation,
                OperationStatus::Failed,
                "HTTP static publish targets are not supported by M3b packaging MCP",
                AgentErrorKind::NotSupported,
                "M3b only plans local static artifact-form publish targets",
            )
            .with_data(route_data(&input, "unsupported_http", "artifact_static")));
        }

        let source = Path::new(&input.artifact_or_project_path);
        let mode = match input.mode {
            PublishModeInput::ProjectStatic => {
                return Ok(failed_plan_result(
                    operation,
                    OperationStatus::Failed,
                    "Project-form static publish is not supported by M3b packaging MCP",
                    AgentErrorKind::NotSupported,
                    "M3b v1 only plans static artifact-form publish for an existing CCS artifact",
                )
                .with_data(route_data(&input, "static_local", "project_static")));
            }
            PublishModeInput::Auto if source.is_dir() => {
                return Ok(failed_plan_result(
                    operation,
                    OperationStatus::Failed,
                    "Auto publish classified this input as project-form, which M3b does not apply",
                    AgentErrorKind::NotSupported,
                    "M3b v1 only plans static artifact-form publish for an existing CCS artifact",
                )
                .with_data(route_data(&input, "static_local", "project_static")));
            }
            PublishModeInput::Auto | PublishModeInput::ArtifactStatic => "artifact_static",
        };

        let artifact = match read_regular_file_identity(source) {
            Ok(identity) => identity,
            Err(error) => {
                return Ok(failed_plan_result(
                    operation,
                    OperationStatus::Failed,
                    "Artifact publish planning needs a regular local CCS artifact",
                    AgentErrorKind::ValidationFailed,
                    error.to_string(),
                )
                .with_data(route_data(&input, "static_local", mode)));
            }
        };
        let destination = match RepoLocation::parse(&input.target) {
            Ok(destination) => destination,
            Err(error) => {
                return Ok(failed_plan_result(
                    operation,
                    OperationStatus::Failed,
                    "Static publish target could not be parsed",
                    AgentErrorKind::ValidationFailed,
                    error.to_string(),
                )
                .with_data(route_data(&input, "static_local", mode)));
            }
        };
        let normalized_target = match normalize_static_target(&destination) {
            Ok(target) => target,
            Err(error) => {
                return Ok(failed_plan_result(
                    operation,
                    OperationStatus::Failed,
                    "Static publish target could not be normalized",
                    AgentErrorKind::ValidationFailed,
                    error.to_string(),
                )
                .with_data(route_data(&input, "static_local", mode)));
            }
        };
        let snapshot = match inspect_artifact_form_static_destination(&destination) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Ok(failed_plan_result(
                    operation,
                    OperationStatus::Failed,
                    "Static destination trust state could not be inspected",
                    AgentErrorKind::MissingPrerequisite,
                    error.to_string(),
                )
                .with_data(route_data(&input, "static_local", mode)));
            }
        };

        if snapshot.initial || snapshot.accepted_signer_set_hash.is_none() {
            return Ok(failed_plan_result(
                operation,
                OperationStatus::Failed,
                "Static destination is missing existing artifact-form trust state",
                AgentErrorKind::MissingPrerequisite,
                "artifact-form MCP publish requires an initialized static repository with active package keys",
            )
            .with_data(serde_json::json!({
                "mode": mode,
                "route": "static_local",
                "artifact_sha256": artifact.digest,
                "artifact_size": artifact.size,
                "destination": snapshot,
            })));
        }

        let material = PublishPlanMaterial {
            schema_version: 1,
            plan_kind: "static_artifact_publish".to_string(),
            mode: mode.to_string(),
            stored_route_enum: "StaticLocal".to_string(),
            normalized_artifact_or_project_path: artifact.path.display().to_string(),
            artifact_sha256: artifact.digest.clone(),
            artifact_size: artifact.size,
            artifact_manifest_identity_when_available: None,
            normalized_static_target: normalized_target,
            key_dir_path_when_supplied: normalize_optional_path(input.key_dir.as_deref())?,
            state_file_path_when_supplied: normalize_optional_path(input.state_file.as_deref())?,
            selected_options: mcp_safe_selected_options(),
            command_risk_projection: "high".to_string(),
            destination_root_key_fingerprint: snapshot.root_key_fingerprint.clone(),
            destination_package_key_hash: snapshot.package_keys_sha256.clone(),
            accepted_signer_set_hash: snapshot.accepted_signer_set_hash.clone(),
            publish_policy_digest: snapshot.publish_policy_digest.clone(),
            metadata_versions_or_watermark: snapshot_metadata_value(&snapshot)?,
            expires_at: (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
        };
        let receipt = self
            .publish_plans
            .lock()
            .map_err(|_| anyhow::anyhow!("publish plan registry lock poisoned"))?
            .insert(material)?;

        let mut envelope = OperationEnvelope::new(
            operation,
            OperationStatus::Planned,
            RiskLevel::High,
            "Planned static artifact publish",
        );
        envelope.subject = Some(resource::packaging_artifact(&artifact.digest));
        envelope.confirmation = Some(ConfirmationRequirement {
            plan_id: receipt.plan_id.clone(),
            level: RiskLevel::High,
            reason: "Publishing mutates a trusted static repository".to_string(),
            input_label: "Type the plan id to confirm".to_string(),
            fingerprint: Some(receipt.fingerprint.clone()),
            expires_at: Some(receipt.expires_at.clone()),
        });
        envelope.next_actions.push(NextAction {
            label: "Apply publish plan".to_string(),
            description: "Call conary.packaging.publish.apply with this plan id, fingerprint, and exact confirmation".to_string(),
            risk: RiskLevel::High,
            command: None,
            requires_confirmation: true,
        });

        Ok(PlanResult::new(envelope).with_data(serde_json::json!({
            "plan_id": receipt.plan_id,
            "fingerprint": receipt.fingerprint,
            "expires_at": receipt.expires_at,
            "mode": mode,
            "route": "static_local",
            "artifact_sha256": artifact.digest,
            "artifact_size": artifact.size,
            "selected_options": mcp_safe_selected_options(),
            "destination": snapshot,
        })))
    }

    pub(crate) async fn apply_publish(&self, input: PublishApplyInput) -> Result<ApplyResult> {
        let operation = "conary.packaging.publish.apply";
        let stored = match self
            .publish_plans
            .lock()
            .map_err(|_| anyhow::anyhow!("publish plan registry lock poisoned"))?
            .get_confirmed(&input.plan_id, &input.fingerprint, &input.confirmation)
        {
            Ok(stored) => stored,
            Err(error) => {
                return Ok(failed_apply_result(
                    operation,
                    "Publish plan confirmation failed",
                    AgentErrorKind::UnsafeWithoutConfirmation,
                    error.to_string(),
                ));
            }
        };
        let material = stored.material;

        if material.plan_kind != "static_artifact_publish"
            || material.mode != "artifact_static"
            || material.stored_route_enum != "StaticLocal"
        {
            return Ok(failed_apply_result(
                operation,
                "Publish plan kind is not supported by this apply path",
                AgentErrorKind::UnsafeWithoutConfirmation,
                "only confirmed static artifact-form publish plans can be applied",
            ));
        }
        if material.selected_options != mcp_safe_selected_options() {
            return Ok(failed_apply_result(
                operation,
                "Publish plan selected options changed",
                AgentErrorKind::UnsafeWithoutConfirmation,
                "MCP publish apply only permits the safe static artifact option set",
            ));
        }

        let artifact = match read_regular_file_identity(Path::new(
            &material.normalized_artifact_or_project_path,
        )) {
            Ok(identity) => identity,
            Err(error) => {
                return Ok(failed_apply_result(
                    operation,
                    "Planned artifact is no longer a regular local file",
                    AgentErrorKind::UnsafeWithoutConfirmation,
                    error.to_string(),
                ));
            }
        };
        if artifact.path.display().to_string() != material.normalized_artifact_or_project_path {
            return Ok(failed_apply_result(
                operation,
                "Planned artifact path changed before apply",
                AgentErrorKind::UnsafeWithoutConfirmation,
                "artifact path recheck did not match the confirmed plan",
            ));
        }
        if artifact.digest != material.artifact_sha256 || artifact.size != material.artifact_size {
            return Ok(failed_apply_result(
                operation,
                "Planned artifact bytes changed before apply",
                AgentErrorKind::UnsafeWithoutConfirmation,
                "artifact digest or size no longer matches the confirmed plan",
            ));
        }

        let destination = match RepoLocation::parse(&material.normalized_static_target) {
            Ok(destination) => destination,
            Err(error) => {
                return Ok(failed_apply_result(
                    operation,
                    "Static publish target could not be parsed during apply",
                    AgentErrorKind::UnsafeWithoutConfirmation,
                    error.to_string(),
                ));
            }
        };
        let normalized_target = match normalize_static_target(&destination) {
            Ok(target) => target,
            Err(error) => {
                return Ok(failed_apply_result(
                    operation,
                    "Static publish target could not be normalized during apply",
                    AgentErrorKind::UnsafeWithoutConfirmation,
                    error.to_string(),
                ));
            }
        };
        if normalized_target != material.normalized_static_target {
            return Ok(failed_apply_result(
                operation,
                "Static publish target changed before apply",
                AgentErrorKind::UnsafeWithoutConfirmation,
                "target route recheck did not match the confirmed plan",
            ));
        }

        let snapshot = match inspect_artifact_form_static_destination(&destination) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Ok(failed_apply_result(
                    operation,
                    "Static destination trust state could not be rechecked",
                    AgentErrorKind::UnsafeWithoutConfirmation,
                    error.to_string(),
                ));
            }
        };
        if snapshot.root_key_fingerprint != material.destination_root_key_fingerprint
            || snapshot.package_keys_sha256 != material.destination_package_key_hash
            || snapshot.accepted_signer_set_hash != material.accepted_signer_set_hash
            || snapshot.publish_policy_digest != material.publish_policy_digest
            || snapshot_metadata_value(&snapshot)? != material.metadata_versions_or_watermark
        {
            return Ok(failed_apply_result(
                operation,
                "Static destination trust state changed before apply",
                AgentErrorKind::UnsafeWithoutConfirmation,
                "destination trust snapshot no longer matches the confirmed plan",
            ));
        }

        let staged = match stage_artifact_private(&artifact.path) {
            Ok(staged) => staged,
            Err(error) => {
                return Ok(failed_apply_result(
                    operation,
                    "Artifact could not be staged for publish apply",
                    AgentErrorKind::UnsafeWithoutConfirmation,
                    error.to_string(),
                ));
            }
        };
        if staged.digest() != material.artifact_sha256 || staged.size() != material.artifact_size {
            return Ok(failed_apply_result(
                operation,
                "Staged artifact digest did not match the confirmed plan",
                AgentErrorKind::UnsafeWithoutConfirmation,
                "private staging changed the artifact bytes unexpectedly",
            ));
        }

        let operation_id = super::super::operation_records::new_operation_id("publish");
        let output = match publish_static_artifact_form_service(StaticArtifactPublishServiceInput {
            artifact_path: staged.path().to_path_buf(),
            target: material.normalized_static_target.clone(),
            key_dir: material
                .key_dir_path_when_supplied
                .as_deref()
                .map(PathBuf::from),
            state_file: material
                .state_file_path_when_supplied
                .as_deref()
                .map(PathBuf::from),
            refresh: false,
            force_reinit: false,
            accept_destination_state: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            operation_id: operation_id.clone(),
        })
        .await
        {
            Ok(output) => output,
            Err(error) => PackagingCommandOutput::failed(
                operation_id,
                "conary publish",
                vec![PackagingDiagnostic::error(
                    PackagingPhase::Publish,
                    PackagingDiagnosticCode::PublishGateFailed,
                    error.to_string(),
                )],
            ),
        };
        let redacted = super::super::diagnostics::redacted_packaging_output(&output);
        super::super::operation_records::write_packaging_record_unchecked(
            &self.operations_dir,
            &redacted.operation_id,
            &redacted,
        )?;

        let mut envelope = project_packaging_output(
            operation,
            &output,
            RiskLevel::High,
            AgentProjectionMode::Apply,
            Some(resource::packaging_artifact(&material.artifact_sha256)),
        );
        envelope
            .changed
            .push(resource::packaging_artifact(&material.artifact_sha256));
        Ok(ApplyResult::new(envelope).with_data(serde_json::json!({
            "plan_id": stored.plan_id,
            "fingerprint": stored.fingerprint,
            "operation_id": output.operation_id,
            "artifact_sha256": material.artifact_sha256,
            "staged_sha256": staged.digest(),
        })))
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

fn failed_envelope_with_status_and_risk(
    operation: &str,
    status: OperationStatus,
    risk: RiskLevel,
    summary: impl Into<String>,
    kind: AgentErrorKind,
    message: impl Into<String>,
) -> OperationEnvelope {
    let mut envelope = OperationEnvelope::new(operation, status, risk, summary);
    envelope.error = Some(AgentError {
        kind,
        message: message.into(),
        remediation: None,
    });
    envelope
}

fn failed_plan_result(
    operation: &str,
    status: OperationStatus,
    summary: impl Into<String>,
    kind: AgentErrorKind,
    message: impl Into<String>,
) -> PlanResult {
    PlanResult::new(failed_envelope_with_status_and_risk(
        operation,
        status,
        RiskLevel::High,
        summary,
        kind,
        message,
    ))
}

fn failed_apply_result(
    operation: &str,
    summary: impl Into<String>,
    kind: AgentErrorKind,
    message: impl Into<String>,
) -> ApplyResult {
    ApplyResult::new(failed_envelope_with_status_and_risk(
        operation,
        OperationStatus::Failed,
        RiskLevel::High,
        summary,
        kind,
        message,
    ))
}

#[derive(Debug, Clone)]
struct FileIdentity {
    path: PathBuf,
    digest: String,
    size: u64,
}

fn read_regular_file_identity(path: &Path) -> Result<FileIdentity> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect artifact {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!("artifact {} is not a regular file", path.display());
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalize artifact {}", path.display()))?;
    let mut file =
        File::open(&canonical).with_context(|| format!("open artifact {}", canonical.display()))?;
    let digest = format!(
        "sha256:{}",
        conary_core::hash::sha256_reader_hex(&mut file)?
    );
    Ok(FileIdentity {
        path: canonical,
        digest,
        size: metadata.len(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishTargetRoute {
    StaticLocal,
    RemiRelease,
    UnsupportedHttp,
}

fn classify_publish_target(target: &str) -> PublishTargetRoute {
    if target.starts_with("http://") || target.starts_with("https://") {
        if target.contains("/v1/admin/releases/") {
            PublishTargetRoute::RemiRelease
        } else {
            PublishTargetRoute::UnsupportedHttp
        }
    } else {
        PublishTargetRoute::StaticLocal
    }
}

fn route_data(
    input: &PublishPlanInput,
    route: &'static str,
    mode: &'static str,
) -> serde_json::Value {
    serde_json::json!({
        "mode": mode,
        "route": route,
        "target": input.target,
        "recipe": input.recipe,
    })
}

fn normalize_static_target(destination: &RepoLocation) -> Result<String> {
    match destination {
        RepoLocation::File { root } => Ok(normalize_path(root)?.display().to_string()),
        RepoLocation::Http { base } => Ok(base.clone()),
    }
}

fn normalize_optional_path(path: Option<&str>) -> Result<Option<String>> {
    path.map(|path| normalize_path(Path::new(path)).map(|path| path.display().to_string()))
        .transpose()
}

fn normalize_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .with_context(|| format!("canonicalize {}", path.display()));
    }
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(path)
}

fn mcp_safe_selected_options() -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        ("refresh".to_string(), serde_json::json!(false)),
        ("force_reinit".to_string(), serde_json::json!(false)),
        (
            "accept_destination_state".to_string(),
            serde_json::json!(false),
        ),
        ("rotate_publish_key".to_string(), serde_json::json!(false)),
        ("rotate_root_key".to_string(), serde_json::json!(false)),
    ])
}

fn snapshot_metadata_value(
    snapshot: &StaticArtifactDestinationSnapshot,
) -> Result<Option<serde_json::Value>> {
    snapshot
        .metadata_versions
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::packaging_mcp::types::{
        PublishApplyInput, PublishModeInput, PublishPlanInput,
    };
    use conary_agent_contract::{AgentErrorKind, OperationStatus, RiskLevel};
    use conary_core::ccs::builder::write_signed_ccs_package;
    use conary_core::ccs::{CcsBuilder, CcsManifest, SigningKeyPair};
    use conary_core::diagnostics::PackagingCommandOutput;
    use conary_core::repository::static_repo::RepoLocation;
    use conary_core::repository::static_repo::publish::{
        StaticPublishOptions, publish_static_repo,
    };

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

    #[test]
    fn publish_plan_for_missing_static_trust_state_returns_missing_prerequisite() {
        let temp = tempfile::TempDir::new().unwrap();
        let artifact = temp.path().join("pkg.ccs");
        std::fs::write(&artifact, b"not-a-real-package").unwrap();
        let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));

        let plan = service
            .plan_publish(PublishPlanInput {
                artifact_or_project_path: artifact.display().to_string(),
                target: temp.path().join("repo").display().to_string(),
                recipe: None,
                key_dir: Some(temp.path().join("keys").display().to_string()),
                state_file: None,
                mode: PublishModeInput::ArtifactStatic,
            })
            .expect("missing trust state is represented as an agent result");

        assert_eq!(plan.envelope.status, OperationStatus::Failed);
        assert_eq!(
            plan.envelope.error.unwrap().kind,
            AgentErrorKind::MissingPrerequisite
        );
        assert!(!temp.path().join("repo").exists());
        assert!(!temp.path().join("keys").exists());
    }

    #[test]
    fn publish_plan_auto_classifies_static_artifact_form() {
        let temp = tempfile::TempDir::new().unwrap();
        let artifact = temp.path().join("pkg.ccs");
        std::fs::write(&artifact, b"not-a-real-package").unwrap();
        let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));

        let plan = service
            .plan_publish(PublishPlanInput {
                artifact_or_project_path: artifact.display().to_string(),
                target: temp.path().join("repo").display().to_string(),
                recipe: None,
                key_dir: Some(temp.path().join("keys").display().to_string()),
                state_file: None,
                mode: PublishModeInput::Auto,
            })
            .unwrap();

        assert_eq!(plan.envelope.status, OperationStatus::Failed);
        assert_eq!(plan.data["mode"], "artifact_static");
        assert_eq!(plan.data["route"], "static_local");
    }

    #[test]
    fn publish_plan_project_static_is_explicitly_unsupported_in_m3b_v1() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));

        let plan = service
            .plan_publish(PublishPlanInput {
                artifact_or_project_path: project.display().to_string(),
                target: temp.path().join("repo").display().to_string(),
                recipe: None,
                key_dir: Some(temp.path().join("keys").display().to_string()),
                state_file: None,
                mode: PublishModeInput::ProjectStatic,
            })
            .unwrap();

        assert_eq!(plan.envelope.status, OperationStatus::Failed);
        assert_eq!(
            plan.envelope.error.unwrap().kind,
            AgentErrorKind::NotSupported
        );
        assert!(!temp.path().join("keys").exists());
    }

    #[test]
    fn publish_plan_remi_target_is_explicitly_unavailable_without_token_resolution() {
        let temp = tempfile::TempDir::new().unwrap();
        let artifact = temp.path().join("pkg.ccs");
        let key_dir = temp.path().join("keys");
        std::fs::write(&artifact, b"not-a-real-package").unwrap();
        let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));

        let plan = service
            .plan_publish(PublishPlanInput {
                artifact_or_project_path: artifact.display().to_string(),
                target: "https://remi.example.invalid/v1/admin/releases/test".to_string(),
                recipe: None,
                key_dir: Some(key_dir.display().to_string()),
                state_file: None,
                mode: PublishModeInput::ArtifactStatic,
            })
            .unwrap();

        assert_eq!(plan.envelope.status, OperationStatus::Unavailable);
        assert_eq!(
            plan.envelope.error.unwrap().kind,
            AgentErrorKind::RemoteUnavailable
        );
        assert!(!key_dir.exists());
    }

    #[test]
    fn publish_plan_rejects_symlink_and_non_regular_artifacts() {
        let temp = tempfile::TempDir::new().unwrap();
        let dir_artifact = temp.path().join("dir-artifact");
        std::fs::create_dir(&dir_artifact).unwrap();
        let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));

        let plan = service
            .plan_publish(PublishPlanInput {
                artifact_or_project_path: dir_artifact.display().to_string(),
                target: temp.path().join("repo").display().to_string(),
                recipe: None,
                key_dir: Some(temp.path().join("keys").display().to_string()),
                state_file: None,
                mode: PublishModeInput::ArtifactStatic,
            })
            .unwrap();
        assert_eq!(plan.envelope.status, OperationStatus::Failed);
        assert_eq!(
            plan.envelope.error.unwrap().kind,
            AgentErrorKind::ValidationFailed
        );

        #[cfg(unix)]
        {
            let artifact = temp.path().join("pkg.ccs");
            let link = temp.path().join("pkg-link.ccs");
            std::fs::write(&artifact, b"package bytes").unwrap();
            std::os::unix::fs::symlink(&artifact, &link).unwrap();

            let plan = service
                .plan_publish(PublishPlanInput {
                    artifact_or_project_path: link.display().to_string(),
                    target: temp.path().join("repo").display().to_string(),
                    recipe: None,
                    key_dir: Some(temp.path().join("keys").display().to_string()),
                    state_file: None,
                    mode: PublishModeInput::ArtifactStatic,
                })
                .unwrap();
            assert_eq!(plan.envelope.status, OperationStatus::Failed);
            assert_eq!(
                plan.envelope.error.unwrap().kind,
                AgentErrorKind::ValidationFailed
            );
        }
    }

    #[tokio::test]
    async fn publish_apply_rejects_missing_plan_without_confirmation() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));

        let result = service
            .apply_publish(PublishApplyInput {
                plan_id: "publish-missing".to_string(),
                fingerprint: "sha256:missing".to_string(),
                confirmation: "publish-missing".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(result.envelope.status, OperationStatus::Failed);
        assert_eq!(
            result.envelope.error.unwrap().kind,
            AgentErrorKind::UnsafeWithoutConfirmation
        );
    }

    #[test]
    fn publish_plan_for_existing_static_trust_state_returns_confirmation() {
        let fixture = StaticPlanFixture::new();
        let artifact = fixture.build_package("planned");
        let service = PackagingAgentService::with_operations_dir(fixture.temp.path().join("ops"));

        let plan = service
            .plan_publish(fixture.plan_input(&artifact, PublishModeInput::Auto))
            .unwrap();

        assert_eq!(plan.envelope.status, OperationStatus::Planned);
        assert_eq!(plan.envelope.risk, RiskLevel::High);
        assert_eq!(plan.data["mode"], "artifact_static");
        assert_eq!(plan.data["route"], "static_local");
        let confirmation = plan.envelope.confirmation.expect("confirmation");
        assert!(confirmation.plan_id.starts_with("publish-"));
        assert_eq!(confirmation.level, RiskLevel::High);
        assert!(
            confirmation
                .fingerprint
                .as_deref()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(!service.operations_dir().exists());
    }

    #[tokio::test]
    async fn publish_apply_rejects_changed_artifact_bytes_before_publish() {
        let fixture = StaticPlanFixture::new();
        let artifact = fixture.build_package("planned");
        let service = PackagingAgentService::with_operations_dir(fixture.temp.path().join("ops"));
        let plan = service
            .plan_publish(fixture.plan_input(&artifact, PublishModeInput::ArtifactStatic))
            .unwrap();
        let confirmation = plan.envelope.confirmation.unwrap();
        std::fs::write(&artifact, b"changed after planning").unwrap();

        let apply = service
            .apply_publish(PublishApplyInput {
                plan_id: confirmation.plan_id.clone(),
                fingerprint: confirmation.fingerprint.unwrap(),
                confirmation: confirmation.plan_id,
            })
            .await
            .unwrap();

        assert_eq!(apply.envelope.status, OperationStatus::Failed);
        assert_eq!(
            apply.envelope.error.unwrap().kind,
            AgentErrorKind::UnsafeWithoutConfirmation
        );
        assert!(!service.operations_dir().exists());
    }

    #[tokio::test]
    async fn publish_apply_rejects_changed_destination_trust_state() {
        let fixture = StaticPlanFixture::new();
        let artifact = fixture.build_package("planned");
        let service = PackagingAgentService::with_operations_dir(fixture.temp.path().join("ops"));
        let plan = service
            .plan_publish(fixture.plan_input(&artifact, PublishModeInput::ArtifactStatic))
            .unwrap();
        let confirmation = plan.envelope.confirmation.unwrap();
        std::fs::write(fixture.repo.join("keys/package-keys.json"), "{}").unwrap();

        let apply = service
            .apply_publish(PublishApplyInput {
                plan_id: confirmation.plan_id.clone(),
                fingerprint: confirmation.fingerprint.unwrap(),
                confirmation: confirmation.plan_id,
            })
            .await
            .unwrap();

        assert_eq!(apply.envelope.status, OperationStatus::Failed);
        assert_eq!(
            apply.envelope.error.unwrap().kind,
            AgentErrorKind::UnsafeWithoutConfirmation
        );
        assert!(!service.operations_dir().exists());
    }

    #[tokio::test]
    async fn publish_apply_projects_gate_failure_and_writes_redacted_record() {
        let fixture = StaticPlanFixture::new();
        let artifact = fixture.build_package("planned");
        let service = PackagingAgentService::with_operations_dir(fixture.temp.path().join("ops"));
        let plan = service
            .plan_publish(fixture.plan_input(&artifact, PublishModeInput::ArtifactStatic))
            .unwrap();
        let confirmation = plan.envelope.confirmation.unwrap();

        let apply = service
            .apply_publish(PublishApplyInput {
                plan_id: confirmation.plan_id.clone(),
                fingerprint: confirmation.fingerprint.unwrap(),
                confirmation: confirmation.plan_id,
            })
            .await
            .unwrap();

        assert_eq!(apply.envelope.status, OperationStatus::Failed);
        assert_eq!(
            apply.envelope.error.as_ref().unwrap().kind,
            AgentErrorKind::ValidationFailed
        );
        let operation_id = apply.data["operation_id"].as_str().unwrap();
        let record_path = service
            .operations_dir()
            .join(format!("{operation_id}.json"));
        assert!(record_path.is_file());
        let record = std::fs::read_to_string(record_path).unwrap();
        assert!(!record.contains("publish.private"));
        assert!(record.contains("publish_lint_report"));
    }

    struct StaticPlanFixture {
        temp: tempfile::TempDir,
        repo: PathBuf,
        key_dir: PathBuf,
        state_file: PathBuf,
        key: SigningKeyPair,
    }

    impl StaticPlanFixture {
        fn new() -> Self {
            let temp = tempfile::TempDir::new().unwrap();
            let repo = temp.path().join("repo");
            let key_dir = temp.path().join("keys");
            let state_file = temp.path().join("publish-state.toml");
            let key = SigningKeyPair::generate().with_key_id("publish");
            key.save_to_files(
                &key_dir.join("publish.private"),
                &key_dir.join("publish.public"),
            )
            .unwrap();
            let fixture = Self {
                temp,
                repo,
                key_dir,
                state_file,
                key,
            };
            let initial = fixture.build_package("initial");
            publish_static_repo(StaticPublishOptions {
                repo_name: "repo".to_string(),
                repo_description: None,
                destination: RepoLocation::File {
                    root: fixture.repo.clone(),
                },
                key_dir: fixture.key_dir.clone(),
                state_file: fixture.state_file.clone(),
                package_paths: vec![initial],
                refresh: false,
                force_reinit: false,
                accept_destination_state: false,
                rotate_publish_key: false,
                rotate_root_key: false,
                artifact_gate_context: None,
            })
            .unwrap();
            fixture
        }

        fn build_package(&self, name: &str) -> PathBuf {
            let source = self.temp.path().join(format!("source-{name}"));
            let package = self.temp.path().join(format!("dist/{name}-1.0.0.ccs"));
            std::fs::create_dir_all(source.join("usr/share/m3b")).unwrap();
            std::fs::create_dir_all(package.parent().unwrap()).unwrap();
            std::fs::write(source.join("usr/share/m3b/payload"), format!("{name}\n")).unwrap();
            let manifest = CcsManifest::parse(&format!(
                r#"
[package]
name = "{name}"
version = "1.0.0"
description = "M3b fixture package"
license = "MIT"

[provenance]
origin_class = "native-built"
hardening_level = "hermetic"
"#
            ))
            .unwrap();
            let result = CcsBuilder::new(manifest, &source).build().unwrap();
            write_signed_ccs_package(&result, &package, &self.key).unwrap();
            package
        }

        fn plan_input(&self, artifact: &Path, mode: PublishModeInput) -> PublishPlanInput {
            PublishPlanInput {
                artifact_or_project_path: artifact.display().to_string(),
                target: self.repo.display().to_string(),
                recipe: None,
                key_dir: Some(self.key_dir.display().to_string()),
                state_file: Some(self.state_file.display().to_string()),
                mode,
            }
        }
    }
}
