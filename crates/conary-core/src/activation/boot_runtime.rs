// conary-core/src/activation/boot_runtime.rs

//! Persisted boot and kernel maintenance work for an exact generation.

use super::ActivationExecutableIdentity;
use crate::boot_runtime::{InvocationDisposition, parse_boot_runtime_invocation};
use serde::{Deserialize, Serialize};

pub const BOOT_RUNTIME_ACTIVATION_SCHEMA_VERSION: u32 = 1;

/// Closed executable identities accepted by the pinned boot-runtime grammars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BootRuntimeProgram {
    Depmod,
    Modprobe,
    Dracut,
    Mkinitcpio,
    UpdateInitramfs,
    KernelInstall,
    Installkernel,
    Dkms,
    GrubMkconfig,
    Grub2Mkconfig,
    UpdateGrub,
    UpdateGrub2,
    Bootctl,
}

impl BootRuntimeProgram {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Depmod => "depmod",
            Self::Modprobe => "modprobe",
            Self::Dracut => "dracut",
            Self::Mkinitcpio => "mkinitcpio",
            Self::UpdateInitramfs => "update-initramfs",
            Self::KernelInstall => "kernel-install",
            Self::Installkernel => "installkernel",
            Self::Dkms => "dkms",
            Self::GrubMkconfig => "grub-mkconfig",
            Self::Grub2Mkconfig => "grub2-mkconfig",
            Self::UpdateGrub => "update-grub",
            Self::UpdateGrub2 => "update-grub2",
            Self::Bootctl => "bootctl",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "depmod" => Ok(Self::Depmod),
            "modprobe" => Ok(Self::Modprobe),
            "dracut" => Ok(Self::Dracut),
            "mkinitcpio" => Ok(Self::Mkinitcpio),
            "update-initramfs" => Ok(Self::UpdateInitramfs),
            "kernel-install" => Ok(Self::KernelInstall),
            "installkernel" => Ok(Self::Installkernel),
            "dkms" => Ok(Self::Dkms),
            "grub-mkconfig" => Ok(Self::GrubMkconfig),
            "grub2-mkconfig" => Ok(Self::Grub2Mkconfig),
            "update-grub" => Ok(Self::UpdateGrub),
            "update-grub2" => Ok(Self::UpdateGrub2),
            "bootctl" => Ok(Self::Bootctl),
            _ => Err(format!("unsupported boot-runtime program '{value}'")),
        }
    }
}

/// Exact mutation deferred from a successful selected-root lifecycle event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootRuntimeActivationInvocation {
    pub schema_version: u32,
    pub program: BootRuntimeProgram,
    pub arguments: Vec<String>,
    pub executable: ActivationExecutableIdentity,
}

impl BootRuntimeActivationInvocation {
    pub fn new(
        program: &str,
        arguments: Vec<String>,
        executable: ActivationExecutableIdentity,
    ) -> Result<Self, String> {
        let invocation = Self {
            schema_version: BOOT_RUNTIME_ACTIVATION_SCHEMA_VERSION,
            program: BootRuntimeProgram::parse(program)?,
            arguments,
            executable,
        };
        invocation.validate()?;
        Ok(invocation)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != BOOT_RUNTIME_ACTIVATION_SCHEMA_VERSION {
            return Err(format!(
                "boot-runtime activation schema version {} is unsupported; expected {}",
                self.schema_version, BOOT_RUNTIME_ACTIVATION_SCHEMA_VERSION
            ));
        }
        self.executable
            .validate()
            .map_err(|error| error.to_string())?;
        let parsed = parse_boot_runtime_invocation(self.program.as_str(), &self.arguments)
            .map_err(|error| error.to_string())?;
        if parsed.disposition() != InvocationDisposition::Mutation {
            return Err(format!(
                "{} invocation is not durable mutation work",
                self.program.as_str()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(program: &str) -> ActivationExecutableIdentity {
        ActivationExecutableIdentity {
            invoked_path: format!("/usr/bin/{program}"),
            canonical_path: format!("/usr/bin/{program}"),
            sha256: format!("sha256:{}", "a".repeat(64)),
        }
    }

    #[test]
    fn accepts_only_mutation_argv_from_the_closed_grammar() {
        BootRuntimeActivationInvocation::new("depmod", vec!["-a".to_string()], identity("depmod"))
            .unwrap();
        assert!(
            BootRuntimeActivationInvocation::new(
                "depmod",
                vec!["--version".to_string()],
                identity("depmod")
            )
            .is_err()
        );
        assert!(
            BootRuntimeActivationInvocation::new(
                "future-tool",
                Vec::new(),
                identity("future-tool")
            )
            .is_err()
        );
    }

    #[test]
    fn persisted_schema_is_closed_and_revalidates() {
        let invocation = BootRuntimeActivationInvocation::new(
            "update-initramfs",
            vec!["-u".to_string(), "-k".to_string(), "all".to_string()],
            identity("update-initramfs"),
        )
        .unwrap();
        let json = serde_json::to_string(&invocation).unwrap();
        let decoded: BootRuntimeActivationInvocation = serde_json::from_str(&json).unwrap();
        decoded.validate().unwrap();
        assert!(
            serde_json::from_str::<BootRuntimeActivationInvocation>(&format!(
                "{{\"unknown\":true,{}}}",
                json.trim_start_matches('{')
            ))
            .is_err()
        );
    }
}
