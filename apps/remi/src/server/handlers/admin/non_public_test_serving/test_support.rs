// apps/remi/src/server/handlers/admin/non_public_test_serving/test_support.rs

use conary_core::ccs::legacy_scriptlets::{
    BootSecurityIntentEvidence, CommandArgumentProvenance, CommandEvidenceSource,
    CommandExecutionContext,
};

pub(super) fn boot_security_intent(
    class_id: &str,
    reason_code: &str,
    command: &str,
    argv: Vec<String>,
    lifecycle_paths: Vec<String>,
) -> BootSecurityIntentEvidence {
    BootSecurityIntentEvidence {
        class_id: class_id.to_string(),
        reason_code: reason_code.to_string(),
        command: command.to_string(),
        command_provenance: CommandArgumentProvenance::Literal,
        argument_provenance: vec![CommandArgumentProvenance::Literal; argv.len()],
        argv,
        execution_context: CommandExecutionContext::Unconditional,
        phase: Some("postinstall".to_string()),
        lifecycle_paths,
        source: CommandEvidenceSource::ShellAst,
        environment: Vec::new(),
        pipeline_id: None,
    }
}
