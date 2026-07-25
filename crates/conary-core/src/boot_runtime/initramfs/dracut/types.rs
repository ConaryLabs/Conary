// crates/conary-core/src/boot_runtime/initramfs/dracut/types.rs

use crate::boot_runtime::{InvocationDisposition, UpstreamContract};

pub const DRACUT_CONTRACT: UpstreamContract = UpstreamContract {
    project: "dracut-ng",
    version: "111",
    revision: "78c043a90a837d2494c573a7cf82d083f3fda9fe",
    source_url: "https://github.com/dracut-ng/dracut-ng",
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DracutOption {
    KernelVersion(String),
    AddModules(String),
    ForceAddModules(String),
    AddDrivers(String),
    ForceDrivers(String),
    OmitDrivers(String),
    Modules(String),
    OmitModules(String),
    Drivers(String),
    Filesystems(String),
    Install(String),
    InstallOptional(String),
    FirmwareDirectory(String),
    LibraryDirectories(String),
    Fscks(String),
    AddFstab(String),
    Mount(String),
    AddDevice(String),
    NoFscks,
    ReadOnlyMount,
    KernelModuleDirectory(String),
    Config(String),
    ConfigDirectory(String),
    AddConfigDirectory(String),
    TemporaryDirectory(String),
    Sysroot(String),
    StandardLogLevel(u8),
    Compressor(String),
    CompressionLevel(String),
    SquashCompressor(String),
    Prefix(String),
    Force,
    KernelOnly,
    NoKernel,
    KernelCommandLine(String),
    Strip,
    AggressiveStrip,
    NoStrip,
    Hardlink,
    NoHardlink,
    NoPrefix,
    MdadmConfig,
    NoMdadmConfig,
    LvmConfig,
    NoLvmConfig,
    NvmfNbftMode(NvmfNbftMode),
    Debug,
    Profile,
    SshKey(String),
    LogFile(String),
    Verbose,
    Quiet,
    Local,
    HostOnly,
    NoHostOnly,
    HostOnlyMode(HostOnlyMode),
    HostOnlyCommandLine,
    NoHostOnlyCommandLine,
    NoHostOnlyDefaultDevice,
    HostOnlyNics(String),
    PersistentPolicy(String),
    Fstab,
    Bzip2,
    Lzma,
    Xz,
    Lzo,
    Lz4,
    Zstd,
    NoCompression,
    Gzip,
    EnhancedCpio,
    ShowModules,
    Keep,
    PrintSize,
    Parallel,
    NoImageIfNotNeeded,
    EarlyMicrocode,
    NoEarlyMicrocode,
    Reproducible,
    NoReproducible,
    LogInstall(String),
    Uefi,
    Ukify,
    NoUefi,
    NoUkify,
    UefiStub(String),
    UefiSplashImage(String),
    KernelImage(String),
    Sbat(String),
    NoHostOnlyI18n,
    HostOnlyI18n,
    NoMachineId,
    Remove(String),
    Include { source: String, target: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOnlyMode {
    Sloppy,
    Strict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvmfNbftMode {
    Match,
    Nbft,
    Static,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DracutAction {
    Help,
    Version,
    ListModules,
    PrintCommandLine,
    PrintConfiguration,
    RegenerateAll,
    Rebuild {
        input: String,
        output: Option<String>,
        kernel_version: Option<String>,
    },
    Build {
        output: Option<String>,
        kernel_version: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DracutInvocation {
    pub contract: UpstreamContract,
    pub action: DracutAction,
    pub options: Vec<DracutOption>,
}

impl DracutInvocation {
    pub fn disposition(&self) -> InvocationDisposition {
        match self.action {
            DracutAction::Help | DracutAction::Version => InvocationDisposition::Information,
            DracutAction::ListModules
            | DracutAction::PrintCommandLine
            | DracutAction::PrintConfiguration => InvocationDisposition::Status,
            DracutAction::RegenerateAll
            | DracutAction::Rebuild { .. }
            | DracutAction::Build { .. } => InvocationDisposition::Mutation,
        }
    }
}
