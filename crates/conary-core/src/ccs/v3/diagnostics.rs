// conary-core/src/ccs/v3/diagnostics.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum V3DiagnosticCode {
    MissingAuthority,
    UnsupportedFormatVersion,
    TomlOnlyAuthority,
    KindContractViolation,
    ComponentAuthorityMismatch,
    IdentityUnstable,
    ConversionNotNative,
    StructuralBudgetExceeded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum V3DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct V3Diagnostic {
    pub code: V3DiagnosticCode,
    pub severity: V3DiagnosticSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub invalid: bool,
    pub suggestion: String,
}

impl V3Diagnostic {
    pub fn error(
        code: V3DiagnosticCode,
        message: impl Into<String>,
        field: impl Into<Option<String>>,
        suggestion: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: V3DiagnosticSeverity::Error,
            message: message.into(),
            field: field.into(),
            path: None,
            invalid: true,
            suggestion: suggestion.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V3ValidationError {
    pub diagnostics: Vec<V3Diagnostic>,
}

impl std::fmt::Display for V3ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(first) = self.diagnostics.first() {
            write!(f, "{}", first.message)
        } else {
            write!(f, "v3 validation failed")
        }
    }
}

impl std::error::Error for V3ValidationError {}
