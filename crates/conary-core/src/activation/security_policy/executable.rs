// conary-core/src/activation/security_policy/executable.rs

//! Exact executable identity captured from the selected root.

use super::{SecurityPolicyParseError, safe_absolute_path};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationExecutableIdentity {
    /// Guest path actually used by the source lifecycle program.
    pub invoked_path: String,
    /// Canonical guest path whose bytes were mounted under the proxy.
    pub canonical_path: String,
    /// Digest of the exact executable bytes in the selected root.
    pub sha256: String,
}

impl ActivationExecutableIdentity {
    pub fn validate(&self) -> Result<(), SecurityPolicyParseError> {
        if !safe_absolute_path(&self.invoked_path) {
            return Err(SecurityPolicyParseError::InvalidExecutableIdentity(
                format!(
                    "invoked path '{}' is not a safe absolute path",
                    self.invoked_path
                ),
            ));
        }
        if !safe_absolute_path(&self.canonical_path) {
            return Err(SecurityPolicyParseError::InvalidExecutableIdentity(
                format!(
                    "canonical path '{}' is not a safe absolute path",
                    self.canonical_path
                ),
            ));
        }
        let Some(hex) = self.sha256.strip_prefix("sha256:") else {
            return Err(SecurityPolicyParseError::InvalidExecutableIdentity(
                "digest is not sha256-prefixed".to_string(),
            ));
        };
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(SecurityPolicyParseError::InvalidExecutableIdentity(
                "digest is not a 32-byte hexadecimal SHA-256".to_string(),
            ));
        }
        Ok(())
    }
}
