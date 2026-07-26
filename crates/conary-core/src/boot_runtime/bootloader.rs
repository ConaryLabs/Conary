// crates/conary-core/src/boot_runtime/bootloader.rs

mod bootctl;
mod grub;

pub use bootctl::{
    BOOTCTL_CONTRACT, BootctlAction, BootctlCertificateSource, BootctlEntryTarget,
    BootctlEntryToken, BootctlInstallSource, BootctlInvocation, BootctlJsonMode, BootctlKeySource,
    BootctlOption, BootctlPrintTarget, BootctlTimeout, BootctlTriState,
};
pub use grub::{
    GRUB_MKCONFIG_CONTRACT, GrubAction, GrubInvocation, GrubOption, UPDATE_GRUB_CONTRACT,
};

use super::BootRuntimeParseError;

pub(super) fn parse_bootctl(argv: &[&str]) -> Result<BootctlInvocation, BootRuntimeParseError> {
    bootctl::parse_bootctl(argv)
}

pub(super) fn parse_grub_mkconfig(
    command: &str,
    argv: &[&str],
) -> Result<GrubInvocation, BootRuntimeParseError> {
    grub::parse_grub_mkconfig(command, argv)
}

pub(super) fn parse_update_grub(argv: &[&str]) -> Result<GrubInvocation, BootRuntimeParseError> {
    grub::parse_update_grub(argv)
}
