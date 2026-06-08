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
//! - Arch .INSTALL function wrapper generation
//! - Timeout protection (60 seconds)
//! - stdin nullification to prevent hangs
//! - Target root support: scriptlets can run inside a target filesystem
//! - Optional container isolation for untrusted scripts
//!
//! ## Target Root Support
//!
//! When installing to a target root (root != "/"), scriptlets are executed
//! inside a chroot or container rooted at the target path. This allows:
//! - Bootstrap: Running package scripts during system construction
//! - Container images: Populating rootfs without affecting host
//! - Offline installations: Installing packages into mounted filesystems
//!
//! The target root must have a working shell and interpreter for scriptlets
//! to execute successfully.

mod arguments;
mod executor;
mod legacy;
mod outcome;
mod phases;
mod process;
mod runtime;
mod sandbox;
mod types;

pub use executor::ScriptletExecutor;
pub use legacy::{LegacyInvocationRuntime, LegacyScriptletExecution};
pub use outcome::{ScriptletFailureKind, ScriptletFailureOutcome, ScriptletOutcome};
pub use phases::{phase_from_string, phase_to_string};
pub use runtime::set_seccomp_warn_override;
pub use sandbox::{EffectiveSandbox, SandboxMode};
pub use types::{ExecutionMode, PackageFormat};
