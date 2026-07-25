// crates/conary-core/src/boot_runtime/initramfs.rs

mod dracut;
mod mkinitcpio;
mod update_initramfs;

pub use dracut::{
    DRACUT_CONTRACT, DracutAction, DracutInvocation, DracutOption, HostOnlyMode, NvmfNbftMode,
};
pub use mkinitcpio::{
    MKINITCPIO_CONTRACT, MkinitcpioAction, MkinitcpioInvocation, MkinitcpioOption,
};
pub use update_initramfs::{
    KernelSelection, UPDATE_INITRAMFS_CONTRACT, UpdateInitramfsAction, UpdateInitramfsInvocation,
    UpdateInitramfsOption,
};

use super::BootRuntimeParseError;

pub(super) fn parse_dracut(argv: &[&str]) -> Result<DracutInvocation, BootRuntimeParseError> {
    dracut::parse_dracut(argv)
}

pub(super) fn parse_mkinitcpio(
    argv: &[&str],
) -> Result<MkinitcpioInvocation, BootRuntimeParseError> {
    mkinitcpio::parse_mkinitcpio(argv)
}

pub(super) fn parse_update_initramfs(
    argv: &[&str],
) -> Result<UpdateInitramfsInvocation, BootRuntimeParseError> {
    update_initramfs::parse_update_initramfs(argv)
}
