// crates/conary-core/src/boot_runtime/kernel.rs

mod depmod;
mod dkms;
mod kernel_install;
mod modprobe;

pub use depmod::{DEPMOD_CONTRACT, DepmodAction, DepmodInvocation, DepmodOption};
pub use dkms::{DKMS_CONTRACT, DkmsAction, DkmsInvocation, DkmsOption, DkmsTarget};
pub use kernel_install::{
    BootEntryToken, BootEntryType, JsonMode, KERNEL_INSTALL_CONTRACT, KernelInstallAction,
    KernelInstallInvocation, KernelInstallOption, SystemdTriState,
};
pub use modprobe::{
    MODPROBE_CONTRACT, ModprobeAction, ModprobeInvocation, ModprobeOperation, ModprobeOption,
};

use super::BootRuntimeParseError;

pub(super) fn parse_depmod(argv: &[&str]) -> Result<DepmodInvocation, BootRuntimeParseError> {
    depmod::parse_depmod(argv)
}

pub(super) fn parse_dkms(argv: &[&str]) -> Result<DkmsInvocation, BootRuntimeParseError> {
    dkms::parse_dkms(argv)
}

pub(super) fn parse_kernel_install(
    argv: &[&str],
) -> Result<KernelInstallInvocation, BootRuntimeParseError> {
    kernel_install::parse_kernel_install(argv)
}

pub(super) fn parse_installkernel(
    argv: &[&str],
) -> Result<KernelInstallInvocation, BootRuntimeParseError> {
    kernel_install::parse_installkernel(argv)
}

pub(super) fn parse_modprobe(argv: &[&str]) -> Result<ModprobeInvocation, BootRuntimeParseError> {
    modprobe::parse_modprobe(argv)
}
