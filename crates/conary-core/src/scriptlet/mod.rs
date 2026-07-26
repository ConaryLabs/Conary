// conary-core/src/scriptlet/mod.rs

//! Scriptlet execution for package install/remove hooks
//!
//! This module handles executing package scriptlets with cross-distro support
//! for RPM, DEB, and Arch packages. Key features:
//!
//! - Distro-specific argument handling:
//!   - RPM: Integer count ($1=1 install, $1=2 upgrade, $1=0 remove)
//!   - DEB: Action words per Debian Policy ($1=install/configure/remove/upgrade)
//!   - Arch: Version strings ($1=new_version, $2=old_version for upgrades)
//! - Exact libalpm-style `.INSTALL` function invocation
//! - Timeout protection (60 seconds)
//! - stdin nullification to prevent hangs
//! - One mandatory selected-root execution boundary
//!
//! ## Selected Root Boundary
//!
//! Scriptlets execute inside a private mount namespace chrooted into the
//! materialized selected root. The host root is rejected. This supports:
//! - Bootstrap: Running package scripts during system construction
//! - Container images: Populating rootfs without affecting host
//! - Offline installations: Installing packages into mounted filesystems
//!
//! The target root must have a working shell and interpreter for scriptlets
//! to execute successfully.

mod activation_capture;
mod arguments;
mod boot_runtime_capture;
mod executor;
mod lifecycle_bridge;
mod native_command;
mod native_lifecycle;
mod outcome;
mod process;
mod rpm_runtime;
mod runtime;
mod sandbox;
#[cfg(test)]
mod test_support;
mod types;

pub use crate::activation::SystemdActivationInvocation;
pub use executor::ScriptletExecutor;
pub use lifecycle_bridge::{
    LifecycleBridgeConfig, LifecycleBridgeEndpoint, LifecycleBridgeHandler,
    LifecycleBridgeHandlerError, LifecycleBridgeRequest, LifecycleBridgeResponse,
    executable_bridge_shim, lifecycle_bridge_shell_library,
};
pub use native_lifecycle::{
    NativeInterpreterAvailability, NativeInvocationRuntime, NativeLifecycleExecution,
};
pub use outcome::{ScriptletFailureKind, ScriptletFailureOutcome, ScriptletOutcome};
pub(crate) use process::configure_target_command_boundary;
pub use sandbox::{EffectiveSandbox, SandboxMode};
pub use types::{ExecutionMode, PackageFormat};
