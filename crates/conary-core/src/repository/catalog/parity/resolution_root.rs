// crates/conary-core/src/repository/catalog/parity/resolution_root.rs

//! Per-root native resolution failures and their private worker wire identity.

use serde::{Deserialize, Serialize};

use super::resolution_contract::NativeResolutionOutcomeV1;
use super::resolution_survey::{
    NativeResolutionSurveyErrorReasonV1, NativeResolutionSurveyErrorVariantV1,
    NativeResolutionSurveyNativeExplanationV1,
};
use crate::error::Error;

#[allow(dead_code)] // Shared by feature-gated native producer root loops.
pub(super) struct NativeRootResolutionSuccess {
    pub(super) outcome: NativeResolutionOutcomeV1,
    pub(super) explanation: Option<NativeResolutionSurveyNativeExplanationV1>,
}

#[allow(dead_code)]
impl NativeRootResolutionSuccess {
    pub(super) fn plain(outcome: NativeResolutionOutcomeV1) -> Self {
        Self {
            outcome,
            explanation: None,
        }
    }

    pub(super) fn explained(
        outcome: NativeResolutionOutcomeV1,
        explanation: NativeResolutionSurveyNativeExplanationV1,
    ) -> Self {
        Self {
            outcome,
            explanation: Some(explanation),
        }
    }
}

#[allow(dead_code)] // Shared by feature-gated native producer root loops.
pub(super) struct NativeRootResolutionError {
    pub(super) error: Error,
    pub(super) reason: NativeResolutionSurveyErrorReasonV1,
    pub(super) explanation: NativeResolutionSurveyNativeExplanationV1,
    pub(super) wire_identity: Option<(NativeResolutionSurveyErrorVariantV1, String)>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum NativeResolutionWireErrorV1 {
    ProviderSearchBudgetExceeded {
        root: String,
        checks: u32,
    },
    Display {
        variant: NativeResolutionSurveyErrorVariantV1,
        message: String,
    },
    UnknownArchitectureToken {
        scheme: String,
        token: String,
    },
}

#[allow(dead_code)]
impl NativeRootResolutionError {
    pub(super) fn new(
        error: Error,
        reason: NativeResolutionSurveyErrorReasonV1,
        explanation: NativeResolutionSurveyNativeExplanationV1,
    ) -> Box<Self> {
        Box::new(Self {
            error,
            reason,
            explanation,
            wire_identity: None,
        })
    }

    pub(super) fn into_wire(
        self,
    ) -> (
        NativeResolutionWireErrorV1,
        NativeResolutionSurveyErrorReasonV1,
        NativeResolutionSurveyNativeExplanationV1,
    ) {
        let error = match self.error {
            Error::ProviderSearchBudgetExceeded { root, checks } => {
                NativeResolutionWireErrorV1::ProviderSearchBudgetExceeded { root, checks }
            }
            Error::UnknownArchitectureToken { scheme, token } => {
                NativeResolutionWireErrorV1::UnknownArchitectureToken { scheme, token }
            }
            error => NativeResolutionWireErrorV1::Display {
                variant: NativeResolutionSurveyErrorVariantV1::from_error(&error),
                message: error.to_string(),
            },
        };
        (error, self.reason, self.explanation)
    }

    pub(super) fn from_wire(
        wire_error: NativeResolutionWireErrorV1,
        reason: NativeResolutionSurveyErrorReasonV1,
        explanation: NativeResolutionSurveyNativeExplanationV1,
    ) -> Box<Self> {
        let (error, variant, message) = match wire_error {
            NativeResolutionWireErrorV1::ProviderSearchBudgetExceeded { root, checks } => {
                let error = Error::ProviderSearchBudgetExceeded { root, checks };
                let message = error.to_string();
                (
                    error,
                    NativeResolutionSurveyErrorVariantV1::ProviderSearchBudgetExceeded,
                    message,
                )
            }
            NativeResolutionWireErrorV1::UnknownArchitectureToken { scheme, token } => {
                let error = Error::UnknownArchitectureToken { scheme, token };
                let message = error.to_string();
                (
                    error,
                    NativeResolutionSurveyErrorVariantV1::UnknownArchitectureToken,
                    message,
                )
            }
            NativeResolutionWireErrorV1::Display { variant, message } => {
                let error = error_from_display_wire(variant, &message);
                (error, variant, message)
            }
        };
        Box::new(Self {
            error,
            reason,
            explanation,
            wire_identity: Some((variant, message)),
        })
    }

    #[cfg(feature = "native-alpm-oracle")]
    pub(super) fn replace_error(
        &mut self,
        error: Error,
        reason: NativeResolutionSurveyErrorReasonV1,
    ) {
        self.error = error;
        self.reason = reason;
        self.wire_identity = None;
    }

    #[cfg(feature = "native-alpm-oracle")]
    pub(super) fn error_message(&self) -> String {
        self.error.to_string()
    }
}

fn error_from_display_wire(variant: NativeResolutionSurveyErrorVariantV1, message: &str) -> Error {
    match variant {
        NativeResolutionSurveyErrorVariantV1::IoError => {
            Error::IoError(strip_error_prefix(message, ""))
        }
        NativeResolutionSurveyErrorVariantV1::InitError => Error::InitError(strip_error_prefix(
            message,
            "Failed to initialize database: ",
        )),
        NativeResolutionSurveyErrorVariantV1::MissingId => {
            Error::MissingId(strip_error_prefix(message, "Missing ID: "))
        }
        NativeResolutionSurveyErrorVariantV1::VersionParse => {
            Error::VersionParse(strip_error_prefix(message, "Version parse error: "))
        }
        NativeResolutionSurveyErrorVariantV1::ConfigError => {
            Error::ConfigError(strip_error_prefix(message, "Configuration error: "))
        }
        NativeResolutionSurveyErrorVariantV1::DatabaseNotFound => {
            Error::DatabaseNotFound(strip_error_prefix(message, "Database not found at path: "))
        }
        NativeResolutionSurveyErrorVariantV1::DownloadError => {
            Error::DownloadError(strip_error_prefix(message, "Download failed: "))
        }
        NativeResolutionSurveyErrorVariantV1::ConflictError => {
            Error::ConflictError(strip_error_prefix(message, "Conflict: "))
        }
        NativeResolutionSurveyErrorVariantV1::ParseError => {
            Error::ParseError(strip_error_prefix(message, "Parse error: "))
        }
        NativeResolutionSurveyErrorVariantV1::InvalidPath => {
            Error::InvalidPath(strip_error_prefix(message, "Invalid path: "))
        }
        NativeResolutionSurveyErrorVariantV1::PathTraversal => {
            Error::PathTraversal(strip_error_prefix(message, "Path traversal detected: "))
        }
        NativeResolutionSurveyErrorVariantV1::NotFound => {
            Error::NotFound(strip_error_prefix(message, "Not found: "))
        }
        NativeResolutionSurveyErrorVariantV1::RecoveryFailed => {
            Error::RecoveryFailed(strip_error_prefix(message, "Recovery failed: "))
        }
        NativeResolutionSurveyErrorVariantV1::TimeoutError => {
            Error::TimeoutError(strip_error_prefix(message, "Timeout: "))
        }
        NativeResolutionSurveyErrorVariantV1::ResolutionError => {
            Error::ResolutionError(strip_error_prefix(message, "Resolution error: "))
        }
        NativeResolutionSurveyErrorVariantV1::NotImplemented => {
            Error::NotImplemented(strip_error_prefix(message, "Not implemented: "))
        }
        NativeResolutionSurveyErrorVariantV1::Capability => {
            Error::Capability(strip_error_prefix(message, "Capability error: "))
        }
        NativeResolutionSurveyErrorVariantV1::Federation => {
            Error::Federation(strip_error_prefix(message, "Federation error: "))
        }
        NativeResolutionSurveyErrorVariantV1::Cancelled => {
            Error::Cancelled(strip_error_prefix(message, "Operation cancelled: "))
        }
        NativeResolutionSurveyErrorVariantV1::InternalError => {
            Error::InternalError(strip_error_prefix(message, "Internal error: "))
        }
        NativeResolutionSurveyErrorVariantV1::TrustError => {
            Error::TrustError(strip_error_prefix(message, "Trust error: "))
        }
        NativeResolutionSurveyErrorVariantV1::PoolOverflow => {
            Error::PoolOverflow(strip_error_prefix(message, "Resolver pool overflow: "))
        }
        _ => Error::ResolutionError(message.to_string()),
    }
}

fn strip_error_prefix(message: &str, prefix: &str) -> String {
    message.strip_prefix(prefix).unwrap_or(message).to_string()
}

#[allow(dead_code)]
pub(super) type NativeRootResolutionResult =
    std::result::Result<NativeRootResolutionSuccess, Box<NativeRootResolutionError>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::catalog::parity::resolution_survey::{
        NativeResolutionSurveyDebianResultV1, NativeResolutionSurveyNativeExplanationV1,
    };

    #[test]
    fn worker_wire_preserves_provider_search_budget_fields() {
        let explanation = NativeResolutionSurveyNativeExplanationV1::Alpm {
            result: super::super::resolution_survey::NativeResolutionSurveyAlpmResultV1::ProviderSearchBudgetExceeded {
                root: "budget-root".to_string(),
                checks: 256,
            },
        };
        let failure = NativeRootResolutionError::new(
            Error::ProviderSearchBudgetExceeded {
                root: "budget-root".to_string(),
                checks: 256,
            },
            NativeResolutionSurveyErrorReasonV1::ProviderSearchBudgetExceeded,
            explanation.clone(),
        );
        let (wire, reason, evidence) = (*failure).into_wire();
        let wire: NativeResolutionWireErrorV1 =
            serde_json::from_slice(&serde_json::to_vec(&wire).unwrap()).unwrap();
        let restored = NativeRootResolutionError::from_wire(wire, reason, evidence);
        assert!(
            matches!(restored.error, Error::ProviderSearchBudgetExceeded { ref root, checks: 256 } if root == "budget-root")
        );
        assert_eq!(restored.explanation, explanation);
        assert_eq!(
            restored.reason,
            NativeResolutionSurveyErrorReasonV1::ProviderSearchBudgetExceeded
        );
        assert_eq!(
            restored.wire_identity.unwrap().0,
            NativeResolutionSurveyErrorVariantV1::ProviderSearchBudgetExceeded
        );
    }

    #[test]
    fn worker_wire_preserves_unknown_architecture_error_fields() {
        let failure = NativeRootResolutionError::new(
            Error::UnknownArchitectureToken {
                scheme: "deb".to_string(),
                token: "future-architecture".to_string(),
            },
            NativeResolutionSurveyErrorReasonV1::UnknownArchitectureToken,
            NativeResolutionSurveyNativeExplanationV1::Debian {
                result: NativeResolutionSurveyDebianResultV1::Unavailable {
                    reason: "not requested".to_string(),
                },
            },
        );

        let (wire_error, reason, explanation) = (*failure).into_wire();
        let restored = NativeRootResolutionError::from_wire(wire_error, reason, explanation);

        assert!(matches!(
            restored.error,
            Error::UnknownArchitectureToken { ref scheme, ref token }
                if scheme == "deb" && token == "future-architecture"
        ));
        assert_eq!(
            restored.wire_identity,
            Some((
                NativeResolutionSurveyErrorVariantV1::UnknownArchitectureToken,
                "Unknown architecture token for deb: 'future-architecture'".to_string(),
            ))
        );
    }
}
