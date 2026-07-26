// crates/conary-core/src/boot_runtime/bootloader/bootctl/types.rs

use crate::boot_runtime::{InvocationDisposition, UpstreamContract};

pub const BOOTCTL_CONTRACT: UpstreamContract = UpstreamContract {
    project: "systemd",
    version: "262~devel",
    revision: "a8e93919c3fd4ff46ce7f1c7a0e73ea0a0a7eaeb",
    source_url: "https://github.com/systemd/systemd",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootctlTriState {
    Auto,
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootctlEntryToken {
    Auto,
    MachineId,
    OsId,
    OsImageId,
    Literal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootctlJsonMode {
    Pretty,
    Short,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootctlInstallSource {
    Auto,
    Image,
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootctlKeySource {
    File,
    Engine(String),
    Provider(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootctlCertificateSource {
    File,
    Provider(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootctlPrintTarget {
    EspPath,
    BootPath,
    RootDevice,
    RootDisk,
    LoaderPath,
    StubPath,
    EfiArchitecture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootctlOption {
    Print(BootctlPrintTarget),
    EspPath(String),
    BootPath(String),
    Root(String),
    Image(String),
    ImagePolicy(String),
    InstallSource(BootctlInstallSource),
    Variables(BootctlTriState),
    RandomSeed(bool),
    NoPager,
    Graceful,
    Quiet,
    EntryToken(BootctlEntryToken),
    MakeEntryDirectory(BootctlTriState),
    Json(BootctlJsonMode),
    AllArchitectures,
    EfiBootOptionDescription(String),
    EfiBootOptionDescriptionWithDevice(bool),
    DryRun,
    SecureBootAutoEnroll(bool),
    PrivateKey(String),
    PrivateKeySource(BootctlKeySource),
    Certificate(String),
    CertificateSource(BootctlCertificateSource),
    Oldest(bool),
    KeepFree(Option<u64>),
    EntryTitle(Option<String>),
    EntryVersion(Option<String>),
    EntryCommit(Option<u64>),
    Extra(Option<String>),
    TriesLeft(Option<u32>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootctlTimeout {
    Clear,
    MenuForce,
    MenuHidden,
    MenuDisabled,
    Duration(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootctlEntryTarget {
    Clear,
    Current,
    OneShot,
    Default,
    SystemFailure,
    Saved,
    Id(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootctlAction {
    Help,
    Version,
    JsonHelp,
    Print(BootctlPrintTarget),
    Status,
    RebootToFirmwareQuery,
    RebootToFirmwareSet(bool),
    List,
    Unlink(Option<String>),
    Link(String),
    LinkAuto,
    Cleanup,
    SetDefault(BootctlEntryTarget),
    SetOneShot(BootctlEntryTarget),
    SetSystemFailure(BootctlEntryTarget),
    SetTimeout(BootctlTimeout),
    SetTimeoutOneShot(BootctlTimeout),
    SetPreferred(BootctlEntryTarget),
    Install,
    Update,
    Remove,
    IsInstalled,
    RandomSeed,
    KernelIdentify(String),
    KernelInspect(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootctlInvocation {
    pub contract: UpstreamContract,
    pub action: BootctlAction,
    pub options: Vec<BootctlOption>,
}

impl BootctlInvocation {
    pub fn disposition(&self) -> InvocationDisposition {
        match self.action {
            BootctlAction::Help | BootctlAction::Version | BootctlAction::JsonHelp => {
                InvocationDisposition::Information
            }
            BootctlAction::Print(_)
            | BootctlAction::Status
            | BootctlAction::RebootToFirmwareQuery
            | BootctlAction::List
            | BootctlAction::IsInstalled
            | BootctlAction::KernelIdentify(_)
            | BootctlAction::KernelInspect(_) => InvocationDisposition::Status,
            BootctlAction::RebootToFirmwareSet(_)
            | BootctlAction::Unlink(_)
            | BootctlAction::Link(_)
            | BootctlAction::LinkAuto
            | BootctlAction::Cleanup
            | BootctlAction::SetDefault(_)
            | BootctlAction::SetOneShot(_)
            | BootctlAction::SetSystemFailure(_)
            | BootctlAction::SetTimeout(_)
            | BootctlAction::SetTimeoutOneShot(_)
            | BootctlAction::SetPreferred(_)
            | BootctlAction::Install
            | BootctlAction::Update
            | BootctlAction::Remove
            | BootctlAction::RandomSeed => InvocationDisposition::Mutation,
        }
    }
}
