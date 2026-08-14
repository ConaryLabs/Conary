// conary-core/src/activation/invocation.rs

//! Persisted closed union of runtime work consumed by an exact generation.

use super::{
    OpenRcActivationInvocation, SecurityPolicyActivationInvocation, SystemdActivationInvocation,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "invocation", rename_all = "kebab-case")]
pub enum RuntimeActivationInvocation {
    Systemd(SystemdActivationInvocation),
    OpenRc(OpenRcActivationInvocation),
    SecurityPolicy(SecurityPolicyActivationInvocation),
}

impl RuntimeActivationInvocation {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Systemd(invocation) => invocation.validate().map_err(|error| error.to_string()),
            Self::OpenRc(invocation) => invocation.validate().map_err(|error| error.to_string()),
            Self::SecurityPolicy(invocation) => {
                invocation.validate().map_err(|error| error.to_string())
            }
        }
    }

    pub const fn source_kind(&self) -> &'static str {
        match self {
            Self::Systemd(_) => "systemd",
            Self::OpenRc(_) => "openrc",
            Self::SecurityPolicy(SecurityPolicyActivationInvocation::Selinux(_)) => "selinux",
            Self::SecurityPolicy(SecurityPolicyActivationInvocation::Apparmor(_)) => "apparmor",
        }
    }
}

impl From<OpenRcActivationInvocation> for RuntimeActivationInvocation {
    fn from(invocation: OpenRcActivationInvocation) -> Self {
        Self::OpenRc(invocation)
    }
}

impl From<SystemdActivationInvocation> for RuntimeActivationInvocation {
    fn from(invocation: SystemdActivationInvocation) -> Self {
        Self::Systemd(invocation)
    }
}

impl From<SecurityPolicyActivationInvocation> for RuntimeActivationInvocation {
    fn from(invocation: SecurityPolicyActivationInvocation) -> Self {
        Self::SecurityPolicy(invocation)
    }
}
