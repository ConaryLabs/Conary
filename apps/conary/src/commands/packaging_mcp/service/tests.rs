// apps/conary/src/commands/packaging_mcp/service/tests.rs

use super::*;
use crate::commands::packaging_mcp::types::{
    PublishApplyInput, PublishModeInput, PublishPlanInput,
};
use conary_agent_contract::{AgentErrorKind, OperationStatus, RiskLevel};
use conary_core::ccs::builder::write_signed_current_ccs_package;
use conary_core::ccs::{CcsBuilder, CcsManifest, SigningKeyPair};
use conary_core::diagnostics::PackagingCommandOutput;
use conary_core::repository::static_repo::RepoLocation;
use conary_core::repository::static_repo::publish::{StaticPublishOptions, publish_static_repo};

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
            crate::commands::packaging_mcp::types::OperationRecordsListInput { limit: Some(10) },
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
fn publish_plan_does_not_route_unrelated_release_substring_to_remi() {
    let temp = tempfile::TempDir::new().unwrap();
    let artifact = temp.path().join("pkg.ccs");
    let key_dir = temp.path().join("keys");
    std::fs::write(&artifact, b"not-a-real-package").unwrap();
    let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));

    let plan = service
        .plan_publish(PublishPlanInput {
            artifact_or_project_path: artifact.display().to_string(),
            target: "https://remi.example.invalid/prefix/v1/admin/releases/fedora".to_string(),
            recipe: None,
            key_dir: Some(key_dir.display().to_string()),
            state_file: None,
            mode: PublishModeInput::ArtifactStatic,
        })
        .unwrap();

    assert_eq!(plan.envelope.status, OperationStatus::Failed);
    assert_eq!(
        plan.envelope.error.unwrap().kind,
        AgentErrorKind::NotSupported
    );
    assert_eq!(plan.data["route"], "unsupported_http");
    assert!(!key_dir.exists());
}

#[test]
fn publish_plan_rejects_non_path_url_authority_before_artifact_reads() {
    let temp = tempfile::TempDir::new().unwrap();
    let key_dir = temp.path().join("keys");
    let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));

    let plan = service
        .plan_publish(PublishPlanInput {
            artifact_or_project_path: temp.path().join("missing.ccs").display().to_string(),
            target: "https://remi.example.invalid/v1/admin/releases/fedora?dry-run=true"
                .to_string(),
            recipe: None,
            key_dir: Some(key_dir.display().to_string()),
            state_file: None,
            mode: PublishModeInput::ArtifactStatic,
        })
        .unwrap();

    assert_eq!(plan.envelope.status, OperationStatus::Failed);
    assert_eq!(
        plan.envelope.error.unwrap().kind,
        AgentErrorKind::ValidationFailed
    );
    assert_eq!(plan.data["route"], "invalid");
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
version_scheme = "conary"
release = "1"
kind = "package"
description = "M3b fixture package"
license = "MIT"

[package.platform]
os = "linux"
arch = "x86_64"
libc = "gnu"

[provenance]
origin_class = "native-built"
hardening_level = "hermetic"
"#
        ))
        .unwrap();
        let result = CcsBuilder::new(manifest, &source).unwrap().build().unwrap();
        write_signed_current_ccs_package(&result, &package, &self.key, false).unwrap();
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
