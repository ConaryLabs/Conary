// conary-core/src/packages/deb/lifecycle_helpers.rs

//! Version-pinned typed argv grammars for Debian lifecycle service helpers.
//!
//! This module describes command-line contracts only. It does not execute a
//! helper, inspect target state, resolve policy, or mutate package state.

mod common;
mod deb_systemd_helper;
mod deb_systemd_invoke;
mod invoke_rc_d;
mod service;
mod update_rc_d;

pub use common::{InitScriptAction, UnitInstance};
pub use deb_systemd_helper::{
    DebSystemdHelperAction, DebSystemdHelperInvocation, DebSystemdHelperOptions,
    DebSystemdHelperQueryStatus,
};
pub use deb_systemd_invoke::{
    DebSystemdInvokeAction, DebSystemdInvokeInvocation, DebSystemdInvokeOptions,
    DebSystemdInvokePolicyStatus,
};
pub use invoke_rc_d::{
    InvokeRcDInvocation, InvokeRcDOptions, InvokeRcDPolicyStatus, InvokeRcDStatus,
};
pub use service::{ServiceAction, ServiceInvocation, ServiceStatus};
pub use update_rc_d::{
    LegacySequenceClause, LegacySequenceKind, SysvRunlevel, UpdateRcDInvocation, UpdateRcDOperation,
};

/// Debian source package version whose executable grammars are modeled here.
pub const INIT_SYSTEM_HELPERS_VERSION: &str = "1.69~deb13u1";

/// Canonical Debian Sources directory for the pinned helper implementations.
pub const INIT_SYSTEM_HELPERS_SOURCE_BASE: &str =
    "https://sources.debian.org/src/init-system-helpers/1.69~deb13u1/script/";

/// Immutable source identity for one helper grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedHelperSource {
    pub command: &'static str,
    pub version: &'static str,
    pub source_url: &'static str,
    pub sha256: &'static str,
}

/// Exact sources used to derive the typed grammars.
pub const PINNED_HELPER_SOURCES: [PinnedHelperSource; 5] = [
    PinnedHelperSource {
        command: "deb-systemd-helper",
        version: INIT_SYSTEM_HELPERS_VERSION,
        source_url: "https://sources.debian.org/src/init-system-helpers/1.69~deb13u1/script/deb-systemd-helper/",
        sha256: "a895d5f077651960b6ca4ed9c53f8b36eae422ae170f61c972d0c6579e9f8732",
    },
    PinnedHelperSource {
        command: "deb-systemd-invoke",
        version: INIT_SYSTEM_HELPERS_VERSION,
        source_url: "https://sources.debian.org/src/init-system-helpers/1.69~deb13u1/script/deb-systemd-invoke/",
        sha256: "92eadae89f4df4cd6088f6316f5390b685faf8d0e312a4e5af3a507caaf81bdb",
    },
    PinnedHelperSource {
        command: "invoke-rc.d",
        version: INIT_SYSTEM_HELPERS_VERSION,
        source_url: "https://sources.debian.org/src/init-system-helpers/1.69~deb13u1/script/invoke-rc.d/",
        sha256: "1d389fa6f6297e77b2ba1d2df4a6ec60f4d4d37815b727889f38ba602e713f77",
    },
    PinnedHelperSource {
        command: "update-rc.d",
        version: INIT_SYSTEM_HELPERS_VERSION,
        source_url: "https://sources.debian.org/src/init-system-helpers/1.69~deb13u1/script/update-rc.d/",
        sha256: "9a85792c1ee2714d34ad2f0dac9becd987cbaedbfe25519dede7378e6c9ebbf1",
    },
    PinnedHelperSource {
        command: "service",
        version: INIT_SYSTEM_HELPERS_VERSION,
        source_url: "https://sources.debian.org/src/init-system-helpers/1.69~deb13u1/script/service/",
        sha256: "f2218f05d3309f75e122c787ba27a7951f8572e55034ee4911f6c6c5b8f18aae",
    },
];

/// One parsed invocation from the pinned Debian helper family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebianLifecycleHelperInvocation {
    DebSystemdHelper(DebSystemdHelperInvocation),
    DebSystemdInvoke(DebSystemdInvokeInvocation),
    InvokeRcD(InvokeRcDInvocation),
    UpdateRcD(UpdateRcDInvocation),
    Service(ServiceInvocation),
}

impl DebianLifecycleHelperInvocation {
    /// Parse an exact helper command name and its argv (excluding argv[0]).
    pub fn parse(command: &str, argv: &[String]) -> Result<Self, HelperGrammarError> {
        match command {
            "deb-systemd-helper" => {
                DebSystemdHelperInvocation::parse(argv).map(Self::DebSystemdHelper)
            }
            "deb-systemd-invoke" => {
                DebSystemdInvokeInvocation::parse(argv).map(Self::DebSystemdInvoke)
            }
            "invoke-rc.d" => InvokeRcDInvocation::parse(argv).map(Self::InvokeRcD),
            "update-rc.d" => UpdateRcDInvocation::parse(argv).map(Self::UpdateRcD),
            "service" => ServiceInvocation::parse(argv).map(Self::Service),
            _ => Err(HelperGrammarError::UnknownHelper(command.to_string())),
        }
    }

    pub const fn command(&self) -> &'static str {
        match self {
            Self::DebSystemdHelper(_) => "deb-systemd-helper",
            Self::DebSystemdInvoke(_) => "deb-systemd-invoke",
            Self::InvokeRcD(_) => "invoke-rc.d",
            Self::UpdateRcD(_) => "update-rc.d",
            Self::Service(_) => "service",
        }
    }

    /// Render a canonical argv accepted by the pinned helper grammar.
    pub fn argv(&self) -> Vec<String> {
        match self {
            Self::DebSystemdHelper(invocation) => invocation.argv(),
            Self::DebSystemdInvoke(invocation) => invocation.argv(),
            Self::InvokeRcD(invocation) => invocation.argv(),
            Self::UpdateRcD(invocation) => invocation.argv(),
            Self::Service(invocation) => invocation.argv(),
        }
    }
}

/// Exact failures from the pinned helper grammars.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HelperGrammarError {
    #[error("unknown Debian lifecycle helper {0:?}")]
    UnknownHelper(String),
    #[error("{helper}: unknown option {option:?}")]
    UnknownOption {
        helper: &'static str,
        option: String,
    },
    #[error("{helper}: missing required {operand}")]
    MissingOperand {
        helper: &'static str,
        operand: &'static str,
    },
    #[error("{helper}: unexpected operand {operand:?}")]
    UnexpectedOperand {
        helper: &'static str,
        operand: String,
    },
    #[error("{helper}: unknown action {action:?}")]
    UnknownAction {
        helper: &'static str,
        action: String,
    },
    #[error("{helper}: invalid {operand} {value:?}: {reason}")]
    InvalidOperand {
        helper: &'static str,
        operand: &'static str,
        value: String,
        reason: &'static str,
    },
    #[error("{helper}: option {option} is not valid with action {action}")]
    InvalidOptionForAction {
        helper: &'static str,
        option: &'static str,
        action: &'static str,
    },
    #[error("{helper}: invalid runlevel {value:?}")]
    InvalidRunlevel { helper: &'static str, value: String },
    #[error("{helper}: malformed legacy start/stop sequence: {reason}")]
    InvalidLegacySequence {
        helper: &'static str,
        reason: String,
    },
    #[error("{helper}: unsupported status {status}")]
    UnsupportedStatus { helper: &'static str, status: i32 },
}

#[cfg(test)]
#[path = "lifecycle_helpers/tests.rs"]
mod tests;
