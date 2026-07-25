// conary-core/src/activation/security_policy.rs

//! Exact SELinux and AppArmor policy operations split at the live-kernel edge.
//!
//! The parsers consume argv observed at execution. Filesystem-only work stays
//! in the selected root, while live policy operations become durable runtime
//! requests for the exact generation built from that root.

mod apparmor;
mod executable;
mod selinux;

pub use apparmor::{ApparmorActivationAction, ApparmorActivationInvocation, ApparmorProgram};
pub use executable::ActivationExecutableIdentity;
pub use selinux::{SelinuxActivationAction, SelinuxActivationInvocation, SelinuxBooleanAssignment};

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", content = "invocation", rename_all = "kebab-case")]
pub enum SecurityPolicyActivationInvocation {
    Selinux(SelinuxActivationInvocation),
    Apparmor(ApparmorActivationInvocation),
}

impl SecurityPolicyActivationInvocation {
    pub fn validate(&self) -> Result<(), SecurityPolicyParseError> {
        match self {
            Self::Selinux(invocation) => invocation.validate(),
            Self::Apparmor(invocation) => invocation.validate(),
        }
    }

    pub const fn provider(&self) -> &'static str {
        match self {
            Self::Selinux(_) => "selinux",
            Self::Apparmor(_) => "apparmor",
        }
    }

    pub fn executable(&self) -> &ActivationExecutableIdentity {
        match self {
            Self::Selinux(invocation) => &invocation.executable,
            Self::Apparmor(invocation) => &invocation.executable,
        }
    }

    pub fn arguments(&self) -> &[String] {
        match self {
            Self::Selinux(invocation) => &invocation.arguments,
            Self::Apparmor(invocation) => &invocation.arguments,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityPolicyInvocationDisposition {
    SelectedRoot,
    Generation(SecurityPolicyActivationInvocation),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecurityPolicyParseError {
    #[error("{program} argv is outside the typed security-policy contract: {reason}")]
    UnsupportedInvocation { program: String, reason: String },
    #[error("{program} live operation cannot be represented durably: {reason}")]
    UnsupportedLiveOperation { program: String, reason: String },
    #[error("security-policy executable identity is invalid: {0}")]
    InvalidExecutableIdentity(String),
}

pub fn parse_security_policy_invocation(
    invoked_program: &str,
    arguments: &[String],
    executable: ActivationExecutableIdentity,
) -> Result<Option<SecurityPolicyInvocationDisposition>, SecurityPolicyParseError> {
    let program = Path::new(invoked_program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(invoked_program);
    match program {
        "restorecon" | "semanage" | "setsebool" | "semodule" => {
            selinux::parse(program, arguments, executable).map(Some)
        }
        "apparmor_parser" | "aa-enforce" | "aa-complain" | "aa-disable" => {
            apparmor::parse(program, arguments, executable).map(Some)
        }
        _ => Ok(None),
    }
}

fn unsupported(program: &str, reason: impl Into<String>) -> SecurityPolicyParseError {
    SecurityPolicyParseError::UnsupportedInvocation {
        program: program.to_string(),
        reason: reason.into(),
    }
}

fn safe_absolute_path(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute()
        && !value.contains('\0')
        && path.components().all(|component| {
            !matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
}

fn nonempty_value(value: &str) -> bool {
    !value.is_empty() && !value.contains('\0')
}
