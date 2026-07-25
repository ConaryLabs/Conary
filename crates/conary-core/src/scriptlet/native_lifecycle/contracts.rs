// conary-core/src/scriptlet/native_lifecycle/contracts.rs

//! Executor-facing native lifecycle invocation contracts.

use crate::scriptlet::ExecutionMode;
use anyhow::{Result, bail};

pub(super) const NATIVE_LIFECYCLE_SAFE_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
const DANGEROUS_NATIVE_ENV_KEYS: [&str; 6] = [
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "BASH_ENV",
    "ENV",
    "PYTHONPATH",
    "PATH",
];

/// Executor-facing view of a native lifecycle bundle entry.
pub struct NativeLifecycleExecution<'a> {
    pub entry_id: &'a str,
    pub phase: &'a str,
    pub interpreter: &'a str,
    pub interpreter_args: &'a [String],
    pub body: String,
    pub body_sha256: String,
    pub body_encoding: Option<&'a str>,
    pub shell_entrypoint: Option<&'a str>,
    pub native_args: &'a [String],
    pub native_environment: &'a [String],
    pub stdin_contract: Option<&'a str>,
    pub chroot_contract: Option<&'a str>,
    pub timeout_ms: u64,
}

/// Runtime values needed to resolve native package-manager invocation contracts.
pub struct NativeInvocationRuntime<'a> {
    pub mode: &'a ExecutionMode,
    pub old_version: Option<&'a str>,
    pub new_version: Option<&'a str>,
    pub package_instance_count: Option<u32>,
    /// Exact arguments already resolved by the transaction event planner.
    pub resolved_native_args: Option<&'a [String]>,
    /// Exact package-manager standard input, such as trigger paths.
    pub stdin: &'a [u8],
}

/// Filesystem fact used when preflighting a native lifecycle interpreter.
///
/// Transaction-wide preflight can run before an incoming payload exists on
/// disk. The transaction planner must therefore supply the exact projected
/// interpreter state for events that run after that payload boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeInterpreterAvailability {
    /// Validate the interpreter against the filesystem as it exists now.
    CurrentRoot,
    /// The transaction's final path projection contains the interpreter.
    ProjectedPresent,
    /// The transaction's final path projection does not contain the interpreter.
    ProjectedMissing,
}

pub(super) fn validate_stdin_contract(execution: &NativeLifecycleExecution<'_>) -> Result<()> {
    match execution.stdin_contract {
        None | Some("none") | Some("null") | Some("paths") | Some("debconf") => Ok(()),
        Some(other) => bail!(
            "NativeArgsContractUnsupported: native lifecycle entry '{}' stdin contract '{}' is unknown",
            execution.entry_id,
            other
        ),
    }
}

pub(super) fn validate_chroot_contract(execution: &NativeLifecycleExecution<'_>) -> Result<()> {
    match execution.chroot_contract {
        None | Some("install-root") | Some("package-manager-default") => Ok(()),
        Some(other) => bail!(
            "SandboxRequirementUnsupported: native lifecycle entry '{}' chroot contract '{}' is unsupported",
            execution.entry_id,
            other
        ),
    }
}

pub(super) fn validate_environment_key(key: &str) -> Result<()> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        || key.as_bytes()[0].is_ascii_digit()
    {
        bail!("NativeArgsContractUnsupported: malformed native environment key '{key}'");
    }

    if DANGEROUS_NATIVE_ENV_KEYS.contains(&key) {
        bail!("NativeArgsContractUnsupported: native environment key '{key}' is denied");
    }

    Ok(())
}
