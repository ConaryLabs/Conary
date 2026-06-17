// conary-core/src/ccs/v2/diagnostics.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum V2DiagnosticCode {
    MissingAuthority,
    LegacyV1Package,
    TomlOnlyAuthority,
    KindContractViolation,
    ComponentAuthorityMismatch,
    LifecycleUnsupported,
    IdentityUnstable,
    ConversionNotNative,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum V2DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct V2Diagnostic {
    pub code: V2DiagnosticCode,
    pub severity: V2DiagnosticSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub invalid: bool,
    pub suggestion: String,
}

impl V2Diagnostic {
    pub fn error(
        code: V2DiagnosticCode,
        message: impl Into<String>,
        field: impl Into<Option<String>>,
        suggestion: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: V2DiagnosticSeverity::Error,
            message: message.into(),
            field: field.into(),
            path: None,
            invalid: true,
            suggestion: suggestion.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2ValidationError {
    pub diagnostics: Vec<V2Diagnostic>,
}

impl std::fmt::Display for V2ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(first) = self.diagnostics.first() {
            write!(f, "{}", first.message)
        } else {
            write!(f, "v2 validation failed")
        }
    }
}

impl std::error::Error for V2ValidationError {}
