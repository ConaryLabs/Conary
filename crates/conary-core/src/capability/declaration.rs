// conary-core/src/capability/declaration.rs
//! Capability declaration types for CCS packages
//!
//! This module defines the structures for declaring what system resources
//! a package needs (network, filesystem, syscalls). These declarations enable:
//! - Documentation of package requirements
//! - Audit mode to compare declared vs observed behavior
//! - Enforcement via landlock/seccomp

use serde::{Deserialize, Serialize};

use crate::repository::versioning::VersionScheme;

/// The only capability-declaration schema accepted by this build.
pub const CAPABILITY_SCHEMA_VERSION: u32 = 1;

/// Complete capability declaration for a package
///
/// This declares what system resources a package requires to function.
/// Used for audit mode and enforcement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDeclaration {
    /// Exact version of the capability schema.
    pub version: u32,

    /// Human-readable rationale explaining why these capabilities are needed
    #[serde(default)]
    pub rationale: Option<String>,

    /// Network access requirements
    #[serde(default)]
    pub network: NetworkCapabilities,

    /// Filesystem access requirements
    #[serde(default)]
    pub filesystem: FilesystemCapabilities,

    /// Syscall requirements
    #[serde(default)]
    pub syscalls: SyscallCapabilities,

    /// Exact Linux process capabilities required by the package
    #[serde(default)]
    pub linux: LinuxCapabilities,
}

impl Default for CapabilityDeclaration {
    fn default() -> Self {
        Self {
            version: CAPABILITY_SCHEMA_VERSION,
            rationale: None,
            network: NetworkCapabilities::default(),
            filesystem: FilesystemCapabilities::default(),
            syscalls: SyscallCapabilities::default(),
            linux: LinuxCapabilities::default(),
        }
    }
}

impl CapabilityDeclaration {
    /// Create a new capability declaration with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if any capabilities are declared
    pub fn is_empty(&self) -> bool {
        self.network.is_empty()
            && self.filesystem.is_empty()
            && self.syscalls.is_empty()
            && self.linux.is_empty()
    }

    /// Validate the declaration for consistency
    pub fn validate(&self) -> Result<(), CapabilityValidationError> {
        if self.version != CAPABILITY_SCHEMA_VERSION {
            return Err(CapabilityValidationError::UnsupportedVersion {
                got: self.version,
                expected: CAPABILITY_SCHEMA_VERSION,
            });
        }

        // Validate network
        self.network.validate()?;

        // Validate filesystem
        self.filesystem.validate()?;

        // Validate syscalls
        self.syscalls.validate()?;

        // Validate exact Linux process capabilities
        self.linux.validate()?;

        Ok(())
    }

    /// Validate exact syscall names against a declared package architecture.
    ///
    /// Architecture-independent packages defer ABI resolution until install
    /// selects a concrete target.
    pub fn validate_for_target_arch(
        &self,
        version_scheme: VersionScheme,
        architecture: Option<&str>,
    ) -> Result<(), CapabilityValidationError> {
        self.validate()?;
        self.syscalls
            .validate_for_target_arch(version_scheme, architecture)
    }

    /// Validate target-dependent requirements against the running system.
    pub fn validate_for_current_target(&self) -> Result<(), CapabilityValidationError> {
        self.validate()?;
        self.syscalls.validate_for_current_target()?;
        self.linux.validate_current_kernel()
    }
}

/// Network capability declarations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct NetworkCapabilities {
    /// Exact remote TCP ports this package may connect to.
    #[serde(default)]
    pub connect_tcp: Vec<u16>,

    /// Exact local TCP ports this package may bind.
    #[serde(default)]
    pub bind_tcp: Vec<u16>,

    /// If true, no network access is required
    #[serde(default)]
    pub none: bool,
}

impl NetworkCapabilities {
    /// Check if no network capabilities are declared
    pub fn is_empty(&self) -> bool {
        self.connect_tcp.is_empty() && self.bind_tcp.is_empty() && !self.none
    }

    /// Validate network capability configuration
    pub fn validate(&self) -> Result<(), CapabilityValidationError> {
        if self.none && (!self.connect_tcp.is_empty() || !self.bind_tcp.is_empty()) {
            return Err(CapabilityValidationError::ConflictingNetwork {
                message: "network.none=true conflicts with connect_tcp/bind_tcp ports".to_string(),
            });
        }

        validate_exact_ports(&self.connect_tcp, "network.connect_tcp")?;
        validate_exact_ports(&self.bind_tcp, "network.bind_tcp")?;

        Ok(())
    }
}

fn validate_exact_ports(ports: &[u16], context: &str) -> Result<(), CapabilityValidationError> {
    let mut seen = std::collections::BTreeSet::new();
    for &port in ports {
        if port == 0 {
            return Err(CapabilityValidationError::InvalidPort {
                port,
                context: context.to_string(),
            });
        }
        if !seen.insert(port) {
            return Err(CapabilityValidationError::DuplicatePort {
                port,
                context: context.to_string(),
            });
        }
    }
    Ok(())
}

/// Filesystem capability declarations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct FilesystemCapabilities {
    /// Paths that can be read (e.g., ["/etc/ssl/certs", "/usr/share"])
    #[serde(default)]
    pub read: Vec<String>,

    /// Paths that can be written (e.g., ["/var/log/nginx", "/var/cache/nginx"])
    #[serde(default)]
    pub write: Vec<String>,

    /// Paths that can be executed (e.g., ["/usr/bin", "/usr/lib"])
    #[serde(default)]
    pub execute: Vec<String>,

    /// Paths that should be denied even if parent allows (e.g., ["/etc/shadow"])
    #[serde(default)]
    pub deny: Vec<String>,
}

impl FilesystemCapabilities {
    /// Check if no filesystem capabilities are declared
    pub fn is_empty(&self) -> bool {
        self.read.is_empty()
            && self.write.is_empty()
            && self.execute.is_empty()
            && self.deny.is_empty()
    }

    /// Validate filesystem capability configuration
    pub fn validate(&self) -> Result<(), CapabilityValidationError> {
        validate_absolute_paths(&self.read, "filesystem.read")?;
        validate_absolute_paths(&self.write, "filesystem.write")?;
        validate_absolute_paths(&self.execute, "filesystem.execute")?;
        validate_absolute_paths(&self.deny, "filesystem.deny")?;
        Ok(())
    }
}

/// Syscall capability declarations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SyscallCapabilities {
    /// Explicitly allowed target-ABI syscall names
    #[serde(default)]
    pub allow: Vec<String>,

    /// Explicitly denied syscalls
    #[serde(default)]
    pub deny: Vec<String>,
}

impl SyscallCapabilities {
    /// Check if no syscall capabilities are declared
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// Validate syscall capability configuration
    pub fn validate(&self) -> Result<(), CapabilityValidationError> {
        // Syscall declarations are exact target-ABI names. Wildcards would
        // delegate security authority to a manually curated expansion list.
        let mut allowed = std::collections::BTreeSet::new();
        for syscall in &self.allow {
            if syscall.contains('*') {
                return Err(CapabilityValidationError::WildcardSyscall {
                    syscall: syscall.clone(),
                    context: "syscalls.allow".to_string(),
                });
            }
            if !is_syscall_name_grammar(syscall) {
                return Err(CapabilityValidationError::InvalidSyscallGrammar {
                    syscall: syscall.clone(),
                    context: "syscalls.allow".to_string(),
                });
            }
            if !allowed.insert(syscall) {
                return Err(CapabilityValidationError::DuplicateSyscall {
                    syscall: syscall.clone(),
                    context: "syscalls.allow".to_string(),
                });
            }
        }

        let mut denied = std::collections::BTreeSet::new();
        for syscall in &self.deny {
            if syscall.contains('*') {
                return Err(CapabilityValidationError::WildcardSyscall {
                    syscall: syscall.clone(),
                    context: "syscalls.deny".to_string(),
                });
            }
            if !is_syscall_name_grammar(syscall) {
                return Err(CapabilityValidationError::InvalidSyscallGrammar {
                    syscall: syscall.clone(),
                    context: "syscalls.deny".to_string(),
                });
            }
            if !denied.insert(syscall) {
                return Err(CapabilityValidationError::DuplicateSyscall {
                    syscall: syscall.clone(),
                    context: "syscalls.deny".to_string(),
                });
            }
            if allowed.contains(syscall) {
                return Err(CapabilityValidationError::ConflictingSyscall {
                    syscall: syscall.clone(),
                });
            }
        }

        Ok(())
    }

    /// Resolve the schema's exact syscall names against a declared target
    /// architecture. Architecture-independent packages defer this check until
    /// installation selects a concrete target.
    pub fn validate_for_target_arch(
        &self,
        version_scheme: VersionScheme,
        architecture: Option<&str>,
    ) -> Result<(), CapabilityValidationError> {
        self.validate()?;
        if self.is_empty() {
            return Ok(());
        }
        let Some(architecture) = architecture else {
            return Ok(());
        };
        let Some(arch) = seccomp_architecture(version_scheme, architecture)? else {
            return Ok(());
        };
        validate_syscalls_for_arch(&self.allow, architecture, arch)?;
        validate_syscalls_for_arch(&self.deny, architecture, arch)
    }

    /// Resolve exact syscall names against the running target architecture.
    pub fn validate_for_current_target(&self) -> Result<(), CapabilityValidationError> {
        self.validate()?;
        for syscall in self.allow.iter().chain(&self.deny) {
            if resolve_native_syscall_number(syscall).is_none() {
                return Err(CapabilityValidationError::InvalidSyscall {
                    syscall: syscall.clone(),
                    architecture: std::env::consts::ARCH.to_string(),
                });
            }
        }
        Ok(())
    }
}

fn is_syscall_name_grammar(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn seccomp_architecture(
    version_scheme: VersionScheme,
    architecture: &str,
) -> Result<Option<libseccomp::ScmpArch>, CapabilityValidationError> {
    use libseccomp::ScmpArch;

    Ok(match (version_scheme, architecture) {
        (VersionScheme::Conary | VersionScheme::Rpm, "noarch")
        | (VersionScheme::Debian, "all")
        | (VersionScheme::Arch, "any") => None,
        (VersionScheme::Debian, "amd64")
        | (VersionScheme::Conary | VersionScheme::Rpm | VersionScheme::Arch, "x86_64") => {
            Some(ScmpArch::X8664)
        }
        (VersionScheme::Debian, "i386")
        | (VersionScheme::Rpm, "i386" | "i486" | "i586" | "i686")
        | (VersionScheme::Conary | VersionScheme::Arch, "i686") => Some(ScmpArch::X86),
        (VersionScheme::Debian, "arm64")
        | (VersionScheme::Conary | VersionScheme::Rpm | VersionScheme::Arch, "aarch64") => {
            Some(ScmpArch::Aarch64)
        }
        (VersionScheme::Debian, "armhf")
        | (VersionScheme::Rpm, "armv7hl")
        | (VersionScheme::Arch, "armv7h")
        | (VersionScheme::Conary, "armv7") => Some(ScmpArch::Arm),
        (_, "loongarch64") => Some(ScmpArch::Loongarch64),
        (_, "m68k") => Some(ScmpArch::M68k),
        (_, "mips") => Some(ScmpArch::Mips),
        (_, "mips64") => Some(ScmpArch::Mips64),
        (_, "mips64n32") => Some(ScmpArch::Mips64N32),
        (_, "mipsel") => Some(ScmpArch::Mipsel),
        (_, "mipsel64") => Some(ScmpArch::Mipsel64),
        (_, "mipsel64n32") => Some(ScmpArch::Mipsel64N32),
        (_, "ppc") => Some(ScmpArch::Ppc),
        (_, "ppc64") => Some(ScmpArch::Ppc64),
        (VersionScheme::Debian, "ppc64el")
        | (VersionScheme::Conary | VersionScheme::Rpm | VersionScheme::Arch, "ppc64le") => {
            Some(ScmpArch::Ppc64Le)
        }
        (_, "s390") => Some(ScmpArch::S390),
        (_, "s390x") => Some(ScmpArch::S390X),
        (_, "parisc" | "hppa") => Some(ScmpArch::Parisc),
        (_, "parisc64" | "hppa64") => Some(ScmpArch::Parisc64),
        (_, "riscv64") => Some(ScmpArch::Riscv64),
        (_, "sheb") => Some(ScmpArch::Sheb),
        (_, "sh") => Some(ScmpArch::Sh),
        (_, _) => {
            return Err(CapabilityValidationError::UnsupportedSyscallArchitecture {
                architecture: architecture.to_string(),
            });
        }
    })
}

fn validate_syscalls_for_arch(
    syscalls: &[String],
    architecture: &str,
    arch: libseccomp::ScmpArch,
) -> Result<(), CapabilityValidationError> {
    for syscall in syscalls {
        let number = libseccomp::ScmpSyscall::from_name_by_arch(syscall, arch)
            .ok()
            .map(|resolved| resolved.as_raw_syscall())
            .filter(|number| *number >= 0);
        if number.is_none() {
            return Err(CapabilityValidationError::InvalidSyscall {
                syscall: syscall.clone(),
                architecture: architecture.to_string(),
            });
        }
    }
    Ok(())
}

/// Resolve an exact syscall name through libseccomp's native-architecture
/// table. Negative pseudo-syscalls are not directly enforceable by the BPF
/// rule map and are rejected.
pub(crate) fn resolve_native_syscall_number(name: &str) -> Option<i64> {
    libseccomp::ScmpSyscall::from_name(name)
        .ok()
        .map(|syscall| i64::from(syscall.as_raw_syscall()))
        .filter(|number| *number >= 0)
}

/// Exact Linux process-capability requirements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct LinuxCapabilities {
    /// Canonical lower-case capability names (for example,
    /// `cap-net-bind-service`)
    #[serde(default)]
    pub required: Vec<String>,
}

impl LinuxCapabilities {
    /// Check if no Linux process capabilities are declared.
    pub fn is_empty(&self) -> bool {
        self.required.is_empty()
    }

    /// Validate canonical capability-name grammar and uniqueness.
    pub fn validate(&self) -> Result<(), CapabilityValidationError> {
        use std::str::FromStr;

        let mut seen = std::collections::BTreeSet::new();
        for capability in &self.required {
            let kernel_name = capability.replace('-', "_").to_ascii_uppercase();
            let parsed = caps::Capability::from_str(&kernel_name).map_err(|_| {
                CapabilityValidationError::InvalidLinuxCapability {
                    capability: capability.clone(),
                }
            })?;
            let canonical = parsed.to_string().to_ascii_lowercase().replace('_', "-");
            if canonical != *capability || !seen.insert(capability) {
                return Err(CapabilityValidationError::InvalidLinuxCapability {
                    capability: capability.clone(),
                });
            }
        }
        Ok(())
    }

    /// Validate that every declared capability exists in the running kernel's
    /// exact capability inventory.
    pub fn validate_current_kernel(&self) -> Result<(), CapabilityValidationError> {
        use std::str::FromStr;

        let supported = caps::runtime::procfs_all_supported(None).map_err(|error| {
            CapabilityValidationError::LinuxCapabilityInventory {
                reason: error.to_string(),
            }
        })?;
        for capability in &self.required {
            let kernel_name = capability.replace('-', "_").to_ascii_uppercase();
            let parsed = caps::Capability::from_str(&kernel_name).map_err(|_| {
                CapabilityValidationError::InvalidLinuxCapability {
                    capability: capability.clone(),
                }
            })?;
            if !supported.contains(&parsed) {
                return Err(CapabilityValidationError::UnsupportedLinuxCapability {
                    capability: capability.clone(),
                });
            }
        }
        Ok(())
    }
}

/// Validate that all paths in a list are absolute and do not contain traversal
fn validate_absolute_paths(
    paths: &[String],
    context: &str,
) -> Result<(), CapabilityValidationError> {
    use std::path::Component;

    for path in paths {
        if path.contains('*') {
            return Err(CapabilityValidationError::WildcardPath {
                path: path.clone(),
                context: context.to_string(),
            });
        }
        if !path.starts_with('/') {
            return Err(CapabilityValidationError::RelativePath {
                path: path.clone(),
                context: context.to_string(),
            });
        }

        // Reject path traversal components (. and ..)
        let p = std::path::Path::new(path);
        for component in p.components() {
            match component {
                Component::ParentDir => {
                    return Err(CapabilityValidationError::PathTraversal {
                        path: path.clone(),
                        context: context.to_string(),
                    });
                }
                Component::CurDir => {
                    return Err(CapabilityValidationError::PathTraversal {
                        path: path.clone(),
                        context: context.to_string(),
                    });
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Validation errors for capability declarations
#[derive(Debug, Clone, thiserror::Error)]
pub enum CapabilityValidationError {
    #[error("unsupported capability schema version {got}; expected exactly {expected}")]
    UnsupportedVersion { got: u32, expected: u32 },

    #[error("conflicting network configuration: {message}")]
    ConflictingNetwork { message: String },

    #[error("port 0 is not a valid exact TCP rule in {context}")]
    InvalidPort { port: u16, context: String },

    #[error("duplicate exact TCP port {port} in {context}")]
    DuplicatePort { port: u16, context: String },

    #[error("relative path not allowed in {context}: {path}")]
    RelativePath { path: String, context: String },

    #[error("path traversal not allowed in {context}: {path}")]
    PathTraversal { path: String, context: String },

    #[error("wildcard path not allowed in {context}: {path}")]
    WildcardPath { path: String, context: String },

    #[error("invalid exact syscall name '{syscall}' in {context}")]
    InvalidSyscallGrammar { syscall: String, context: String },

    #[error("wildcard syscall '{syscall}' is not allowed in {context}; use exact ABI names")]
    WildcardSyscall { syscall: String, context: String },

    #[error("duplicate syscall '{syscall}' in {context}")]
    DuplicateSyscall { syscall: String, context: String },

    #[error("syscall '{syscall}' cannot be both allowed and denied")]
    ConflictingSyscall { syscall: String },

    #[error("cannot resolve syscall declarations for unsupported architecture '{architecture}'")]
    UnsupportedSyscallArchitecture { architecture: String },

    #[error("syscall '{syscall}' is not an exact name in the {architecture} ABI")]
    InvalidSyscall {
        syscall: String,
        architecture: String,
    },

    #[error("invalid or duplicate Linux capability name: {capability}")]
    InvalidLinuxCapability { capability: String },

    #[error("Linux capability is not supported by the running kernel: {capability}")]
    UnsupportedLinuxCapability { capability: String },

    #[error("cannot read the running kernel capability inventory: {reason}")]
    LinuxCapabilityInventory { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_capability_declaration() {
        let cap = CapabilityDeclaration::default();
        assert_eq!(cap.version, 1);
        assert!(cap.is_empty());
    }

    #[test]
    fn test_network_none_conflict() {
        let mut cap = CapabilityDeclaration::default();
        cap.network.none = true;
        cap.network.connect_tcp.push(80);

        let result = cap.validate();
        assert!(result.is_err());
    }

    #[test]
    fn exact_tcp_ports_are_validated_without_string_grammar() {
        NetworkCapabilities {
            connect_tcp: vec![80, 443, 65535],
            bind_tcp: vec![8080],
            none: false,
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn port_zero_and_duplicates_are_rejected() {
        for capabilities in [
            NetworkCapabilities {
                connect_tcp: vec![0],
                ..Default::default()
            },
            NetworkCapabilities {
                bind_tcp: vec![443, 443],
                ..Default::default()
            },
        ] {
            assert!(capabilities.validate().is_err());
        }
    }

    #[test]
    fn string_ports_and_old_network_fields_are_not_in_the_schema() {
        for toml in [
            "[network]\nconnect_tcp = [\"443\"]",
            "[network]\noutbound = [443]",
            "[network]\nlisten = [443]",
        ] {
            let document = format!("version = 1\n{toml}\n");
            assert!(toml::from_str::<CapabilityDeclaration>(&document).is_err());
        }
    }

    #[test]
    fn unknown_filesystem_fields_are_not_silently_ignored() {
        let error = toml::from_str::<CapabilityDeclaration>(
            r#"
version = 1

[filesystem]
read_only = ["/etc"]
"#,
        )
        .expect_err("misspelled filesystem authority must fail closed");

        assert!(error.to_string().contains("unknown field `read_only`"));
    }

    #[test]
    fn test_relative_path_rejected() {
        let mut cap = CapabilityDeclaration::default();
        cap.filesystem.read.push("etc/config".to_string());

        let result = cap.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_absolute_path_accepted() {
        let mut cap = CapabilityDeclaration::default();
        cap.filesystem.read.push("/etc/config".to_string());
        cap.filesystem.write.push("/var/log".to_string());

        let result = cap.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn package_cannot_select_an_internal_syscall_profile() {
        let toml = r#"
            version = 1

            [syscalls]
            profile = "scriptlet-v1"
        "#;

        let error = toml::from_str::<CapabilityDeclaration>(toml)
            .expect_err("executor ABI selection is not package-controlled");
        assert!(error.to_string().contains("unknown field `profile`"));
    }

    #[test]
    fn test_full_capability_declaration_parse() {
        let toml = r#"
            version = 1
            rationale = "Web server requiring network listeners and cache access"

            [network]
            connect_tcp = [443, 80]
            bind_tcp = [80, 443]
            none = false

            [filesystem]
            read = ["/etc/nginx", "/etc/ssl/certs"]
            write = ["/var/cache/nginx", "/var/log/nginx"]
            execute = ["/usr/bin", "/usr/lib"]
            deny = ["/home", "/root", "/etc/shadow"]

            [syscalls]
            allow = ["read", "write", "epoll_ctl"]
            deny = ["ptrace"]
        "#;

        let cap: CapabilityDeclaration = toml::from_str(toml).unwrap();
        assert_eq!(cap.version, 1);
        assert!(cap.rationale.is_some());
        assert_eq!(cap.network.bind_tcp, [80, 443]);
        assert_eq!(cap.filesystem.read.len(), 2);
        assert_eq!(cap.syscalls.allow, ["read", "write", "epoll_ctl"]);
    }

    #[test]
    fn capability_schema_version_is_required() {
        let error = toml::from_str::<CapabilityDeclaration>(
            r#"
            rationale = "missing schema version"
            "#,
        )
        .expect_err("missing capability schema version must not select a default");
        assert!(error.to_string().contains("missing field `version`"));
    }

    #[test]
    fn unknown_capability_schema_version_is_rejected() {
        let cap = CapabilityDeclaration {
            version: CAPABILITY_SCHEMA_VERSION + 1,
            ..CapabilityDeclaration::default()
        };
        assert!(matches!(
            cap.validate(),
            Err(CapabilityValidationError::UnsupportedVersion {
                got: 2,
                expected: CAPABILITY_SCHEMA_VERSION
            })
        ));
    }

    #[test]
    fn canonical_but_unknown_linux_capability_is_rejected() {
        let capabilities = LinuxCapabilities {
            required: vec!["cap-conary-made-this-up".to_string()],
        };
        assert!(matches!(
            capabilities.validate(),
            Err(CapabilityValidationError::InvalidLinuxCapability { .. })
        ));
    }

    #[test]
    fn known_linux_capability_must_use_the_exact_public_spelling() {
        for invalid in ["CAP_NET_RAW", "cap_net_raw", "net-raw", "cap-net-raw-"] {
            let capabilities = LinuxCapabilities {
                required: vec![invalid.to_string()],
            };
            assert!(
                capabilities.validate().is_err(),
                "{invalid} must not be normalized into authority"
            );
        }
        LinuxCapabilities {
            required: vec!["cap-net-raw".to_string()],
        }
        .validate()
        .expect("exact public spelling should resolve through the caps vocabulary");
    }

    #[test]
    fn current_kernel_inventory_accepts_a_baseline_linux_capability() {
        LinuxCapabilities {
            required: vec!["cap-chown".to_string()],
        }
        .validate_current_kernel()
        .expect("Linux kernels supporting Conary must expose CAP_CHOWN");
    }

    #[test]
    fn wildcard_syscall_declaration_is_rejected() {
        let mut cap = CapabilityDeclaration::default();
        cap.syscalls.allow.push("epoll_*".to_string());

        let error = cap
            .validate()
            .expect_err("syscall wildcards must not control security authority");
        assert!(matches!(
            error,
            CapabilityValidationError::WildcardSyscall { .. }
        ));
    }

    #[test]
    fn unknown_syscall_declaration_is_rejected_against_native_abi() {
        let mut cap = CapabilityDeclaration::default();
        cap.syscalls
            .allow
            .push("definitely_not_a_syscall".to_string());
        assert!(matches!(
            cap.validate_for_current_target(),
            Err(CapabilityValidationError::InvalidSyscall { .. })
        ));
    }

    #[test]
    fn schema_validation_does_not_use_the_build_hosts_syscall_abi() {
        let mut cap = CapabilityDeclaration::default();
        cap.syscalls.allow.push("openat".to_string());
        cap.validate_for_target_arch(VersionScheme::Conary, Some("aarch64"))
            .expect("a target ABI must be resolved through libseccomp");
    }

    #[test]
    fn target_architecture_validation_rejects_a_host_specific_syscall() {
        let mut cap = CapabilityDeclaration::default();
        cap.syscalls.allow.push("arch_prctl".to_string());
        assert!(matches!(
            cap.validate_for_target_arch(VersionScheme::Conary, Some("aarch64")),
            Err(CapabilityValidationError::InvalidSyscall { .. })
        ));
    }

    #[test]
    fn architecture_independent_packages_defer_syscall_resolution() {
        let mut cap = CapabilityDeclaration::default();
        cap.syscalls
            .allow
            .push("syntactically_valid_but_unknown".to_string());
        cap.validate_for_target_arch(VersionScheme::Conary, Some("noarch"))
            .expect("noarch does not select a concrete syscall ABI");
    }
}
