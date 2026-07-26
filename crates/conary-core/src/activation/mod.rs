// conary-core/src/activation/mod.rs

//! Typed runtime work that is deferred until an immutable generation boots.
//!
//! Package transactions materialize only a selected root. They never signal
//! live host service or security-policy managers. Exact runtime invocations
//! are captured as closed contracts and later projected onto the generation
//! built from the transaction.

mod invocation;
mod security_policy;
mod systemd;

pub use invocation::RuntimeActivationInvocation;
pub use security_policy::{
    ActivationExecutableIdentity, ApparmorActivationAction, ApparmorActivationInvocation,
    ApparmorProgram, SecurityPolicyActivationInvocation, SecurityPolicyInvocationDisposition,
    SecurityPolicyParseError, SelinuxActivationAction, SelinuxActivationInvocation,
    SelinuxBooleanAssignment, parse_security_policy_invocation,
};
pub(crate) use systemd::systemctl_proxy_shell_scan;
pub use systemd::{
    SystemdActivationAction, SystemdActivationInvocation, SystemdActivationParseError,
    SystemdJobMode, parse_systemctl_activation_invocation,
};
