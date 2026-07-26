// crates/conary-core/src/boot_runtime.rs

//! Version-pinned argv contracts for boot and kernel maintenance tools.
//!
//! This module only parses an already-tokenized command invocation. It does not
//! recognize shell syntax, execute a command, or infer authority from command
//! names embedded in free-form text.

mod argv;
mod bootloader;
mod initramfs;
mod kernel;

use std::fmt;

pub use bootloader::{
    BOOTCTL_CONTRACT, BootctlAction, BootctlCertificateSource, BootctlEntryTarget,
    BootctlEntryToken, BootctlInstallSource, BootctlInvocation, BootctlJsonMode, BootctlKeySource,
    BootctlOption, BootctlPrintTarget, BootctlTimeout, BootctlTriState, GRUB_MKCONFIG_CONTRACT,
    GrubAction, GrubInvocation, GrubOption, UPDATE_GRUB_CONTRACT,
};
pub use initramfs::{
    DRACUT_CONTRACT, DracutAction, DracutInvocation, DracutOption, HostOnlyMode, KernelSelection,
    MKINITCPIO_CONTRACT, MkinitcpioAction, MkinitcpioInvocation, MkinitcpioOption, NvmfNbftMode,
    UPDATE_INITRAMFS_CONTRACT, UpdateInitramfsAction, UpdateInitramfsInvocation,
    UpdateInitramfsOption,
};
pub use kernel::{
    BootEntryToken, BootEntryType, DEPMOD_CONTRACT, DKMS_CONTRACT, DepmodAction, DepmodInvocation,
    DepmodOption, DkmsAction, DkmsInvocation, DkmsOption, DkmsTarget, JsonMode,
    KERNEL_INSTALL_CONTRACT, KernelInstallAction, KernelInstallInvocation, KernelInstallOption,
    MODPROBE_CONTRACT, ModprobeAction, ModprobeInvocation, ModprobeOperation, ModprobeOption,
    SystemdTriState,
};

/// An upstream source snapshot used as argv grammar authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpstreamContract {
    pub project: &'static str,
    pub version: &'static str,
    pub revision: &'static str,
    pub source_url: &'static str,
}

/// Whether an invocation only reports information or requests durable mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationDisposition {
    Information,
    Status,
    Mutation,
}

/// A parsed invocation of one of the explicitly supported runtime tools.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BootRuntimeInvocation {
    Depmod(DepmodInvocation),
    Modprobe(ModprobeInvocation),
    Dracut(DracutInvocation),
    Mkinitcpio(MkinitcpioInvocation),
    UpdateInitramfs(UpdateInitramfsInvocation),
    KernelInstall(KernelInstallInvocation),
    InstallKernel(KernelInstallInvocation),
    Dkms(DkmsInvocation),
    GrubMkconfig(GrubInvocation),
    UpdateGrub(GrubInvocation),
    Bootctl(BootctlInvocation),
}

impl BootRuntimeInvocation {
    pub fn disposition(&self) -> InvocationDisposition {
        match self {
            Self::Depmod(invocation) => invocation.disposition(),
            Self::Modprobe(invocation) => invocation.disposition(),
            Self::Dracut(invocation) => invocation.disposition(),
            Self::Mkinitcpio(invocation) => invocation.disposition(),
            Self::UpdateInitramfs(invocation) => invocation.disposition(),
            Self::KernelInstall(invocation) | Self::InstallKernel(invocation) => {
                invocation.disposition()
            }
            Self::Dkms(invocation) => invocation.disposition(),
            Self::GrubMkconfig(invocation) | Self::UpdateGrub(invocation) => {
                invocation.disposition()
            }
            Self::Bootctl(invocation) => invocation.disposition(),
        }
    }
}

/// A closed parse failure. Unknown or malformed forms never become an intent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BootRuntimeParseError {
    UnsupportedCommand {
        command: String,
    },
    UnknownOption {
        command: &'static str,
        option: String,
    },
    MissingOptionValue {
        command: &'static str,
        option: String,
        values: usize,
    },
    UnexpectedOptionValue {
        command: &'static str,
        option: String,
    },
    InvalidOptionValue {
        command: &'static str,
        option: String,
        value: String,
        expected: &'static str,
    },
    MissingAction {
        command: &'static str,
        expected: &'static str,
    },
    UnknownAction {
        command: &'static str,
        action: String,
    },
    MissingOperand {
        command: &'static str,
        action: &'static str,
        expected: &'static str,
    },
    UnexpectedOperand {
        command: &'static str,
        action: &'static str,
        operand: String,
    },
    ConflictingArguments {
        command: &'static str,
        detail: &'static str,
    },
}

impl fmt::Display for BootRuntimeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCommand { command } => {
                write!(f, "unsupported boot runtime command {command:?}")
            }
            Self::UnknownOption { command, option } => {
                write!(f, "{command}: unknown option {option:?}")
            }
            Self::MissingOptionValue {
                command,
                option,
                values,
            } => write!(
                f,
                "{command}: option {option:?} requires {values} value{}",
                if *values == 1 { "" } else { "s" }
            ),
            Self::UnexpectedOptionValue { command, option } => {
                write!(f, "{command}: option {option:?} does not take a value")
            }
            Self::InvalidOptionValue {
                command,
                option,
                value,
                expected,
            } => write!(
                f,
                "{command}: invalid value {value:?} for {option:?}; expected {expected}"
            ),
            Self::MissingAction { command, expected } => {
                write!(f, "{command}: missing action; expected {expected}")
            }
            Self::UnknownAction { command, action } => {
                write!(f, "{command}: unknown action {action:?}")
            }
            Self::MissingOperand {
                command,
                action,
                expected,
            } => write!(f, "{command} {action}: missing {expected}"),
            Self::UnexpectedOperand {
                command,
                action,
                operand,
            } => write!(f, "{command} {action}: unexpected operand {operand:?}"),
            Self::ConflictingArguments { command, detail } => {
                write!(f, "{command}: {detail}")
            }
        }
    }
}

impl std::error::Error for BootRuntimeParseError {}

/// Parse an exact command name and argv vector (excluding argv[0]).
///
/// Executable-path normalization is deliberately outside this API. Callers
/// must resolve a structured executable identity before selecting this parser.
pub fn parse_boot_runtime_invocation<S: AsRef<str>>(
    command: &str,
    argv: &[S],
) -> Result<BootRuntimeInvocation, BootRuntimeParseError> {
    let argv: Vec<&str> = argv.iter().map(AsRef::as_ref).collect();
    match command {
        "depmod" => kernel::parse_depmod(&argv).map(BootRuntimeInvocation::Depmod),
        "modprobe" => kernel::parse_modprobe(&argv).map(BootRuntimeInvocation::Modprobe),
        "dracut" => initramfs::parse_dracut(&argv).map(BootRuntimeInvocation::Dracut),
        "mkinitcpio" => initramfs::parse_mkinitcpio(&argv).map(BootRuntimeInvocation::Mkinitcpio),
        "update-initramfs" => {
            initramfs::parse_update_initramfs(&argv).map(BootRuntimeInvocation::UpdateInitramfs)
        }
        "kernel-install" => {
            kernel::parse_kernel_install(&argv).map(BootRuntimeInvocation::KernelInstall)
        }
        "installkernel" => {
            kernel::parse_installkernel(&argv).map(BootRuntimeInvocation::InstallKernel)
        }
        "dkms" => kernel::parse_dkms(&argv).map(BootRuntimeInvocation::Dkms),
        "grub-mkconfig" | "grub2-mkconfig" => {
            bootloader::parse_grub_mkconfig(command, &argv).map(BootRuntimeInvocation::GrubMkconfig)
        }
        "update-grub" | "update-grub2" => {
            bootloader::parse_update_grub(&argv).map(BootRuntimeInvocation::UpdateGrub)
        }
        "bootctl" => bootloader::parse_bootctl(&argv).map(BootRuntimeInvocation::Bootctl),
        _ => Err(BootRuntimeParseError::UnsupportedCommand {
            command: command.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_dispatch_accepts_only_the_declared_basenames() {
        let cases: &[(&str, &[&str])] = &[
            ("depmod", &[]),
            ("modprobe", &["-n", "demo"]),
            ("dracut", &[]),
            ("mkinitcpio", &[]),
            ("update-initramfs", &["-u"]),
            ("kernel-install", &[]),
            ("installkernel", &["6.12", "/vmlinuz"]),
            ("dkms", &["status"]),
            ("grub-mkconfig", &[]),
            ("grub2-mkconfig", &[]),
            ("update-grub", &[]),
            ("update-grub2", &[]),
            ("bootctl", &[]),
        ];
        for (command, argv) in cases {
            assert!(
                parse_boot_runtime_invocation(command, argv).is_ok(),
                "{command}"
            );
        }

        assert!(parse_boot_runtime_invocation("/usr/sbin/depmod", &[] as &[&str]).is_err());
        assert!(parse_boot_runtime_invocation("dracut-install", &[] as &[&str]).is_err());
    }

    #[test]
    fn every_contract_is_revision_pinned() {
        for contract in [
            DEPMOD_CONTRACT,
            MODPROBE_CONTRACT,
            DRACUT_CONTRACT,
            MKINITCPIO_CONTRACT,
            UPDATE_INITRAMFS_CONTRACT,
            KERNEL_INSTALL_CONTRACT,
            DKMS_CONTRACT,
            GRUB_MKCONFIG_CONTRACT,
            UPDATE_GRUB_CONTRACT,
            BOOTCTL_CONTRACT,
        ] {
            assert!(!contract.project.is_empty());
            assert!(!contract.version.is_empty());
            assert_eq!(contract.revision.len(), 40, "{}", contract.project);
            assert!(
                contract
                    .revision
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            );
            assert!(contract.source_url.starts_with("https://"));
        }
    }

    #[test]
    fn update_grub2_is_the_debian_declared_symlink_alias() {
        let update = parse_boot_runtime_invocation("update-grub", &["--output=/custom"]).unwrap();
        let update2 = parse_boot_runtime_invocation("update-grub2", &["--output=/custom"]).unwrap();
        assert_eq!(update, update2);
    }
}
