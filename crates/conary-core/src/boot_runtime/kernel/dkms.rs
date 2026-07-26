// crates/conary-core/src/boot_runtime/kernel/dkms.rs

use crate::boot_runtime::{BootRuntimeParseError, InvocationDisposition, UpstreamContract};

pub const DKMS_CONTRACT: UpstreamContract = UpstreamContract {
    project: "DKMS",
    version: "3.4.2",
    revision: "43f222144e8b8a556335fc5cfa97758ff58a8442",
    source_url: "https://github.com/dell/dkms",
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DkmsOption {
    Module {
        name: String,
        version: Option<String>,
    },
    ModuleVersion(String),
    Kernel {
        version: String,
        architecture: Option<String>,
    },
    Architecture(String),
    Config(String),
    Quiet,
    NoInitrdDeprecated,
    NoCleanKernelDeprecated,
    NoPrepareKernelDeprecated,
    BinariesOnly,
    SourceOnly,
    Force,
    ForceVersionOverride,
    All,
    Verbose,
    RpmSafeUpgrade,
    DkmsTree(String),
    SourceTree(String),
    InstallTree(String),
    SymlinkModules,
    KernelConfig(String),
    Archive(String),
    KernelSourceDirectory(String),
    Directive {
        name: String,
        value: String,
    },
    NoDepmod,
    ModprobeOnInstall,
    Debug,
    ParallelJobs(u32),
    TemplateKernel {
        version: String,
        architecture: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DkmsTarget {
    /// Upstream resolves a bare operand against live filesystem state before
    /// deciding whether it is a module/version, source tree, archive, or RPM.
    FilesystemResolved(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DkmsAction {
    Help,
    Version,
    Add,
    Remove,
    Build,
    Unbuild,
    Install,
    Uninstall,
    Match,
    Autoinstall,
    MakeTarball,
    LoadTarball,
    Status,
    GenerateMok,
    KernelPreInstall,
    KernelPostInstall,
    KernelPreRemove,
}

impl DkmsAction {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "add" => Some(Self::Add),
            "remove" => Some(Self::Remove),
            "build" => Some(Self::Build),
            "unbuild" => Some(Self::Unbuild),
            "install" => Some(Self::Install),
            "uninstall" => Some(Self::Uninstall),
            "match" => Some(Self::Match),
            "autoinstall" => Some(Self::Autoinstall),
            "mktarball" => Some(Self::MakeTarball),
            "ldtarball" => Some(Self::LoadTarball),
            "status" => Some(Self::Status),
            "generate_mok" => Some(Self::GenerateMok),
            "kernel_preinst" => Some(Self::KernelPreInstall),
            "kernel_postinst" => Some(Self::KernelPostInstall),
            "kernel_prerm" => Some(Self::KernelPreRemove),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Version => "version",
            Self::Add => "add",
            Self::Remove => "remove",
            Self::Build => "build",
            Self::Unbuild => "unbuild",
            Self::Install => "install",
            Self::Uninstall => "uninstall",
            Self::Match => "match",
            Self::Autoinstall => "autoinstall",
            Self::MakeTarball => "mktarball",
            Self::LoadTarball => "ldtarball",
            Self::Status => "status",
            Self::GenerateMok => "generate_mok",
            Self::KernelPreInstall => "kernel_preinst",
            Self::KernelPostInstall => "kernel_postinst",
            Self::KernelPreRemove => "kernel_prerm",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DkmsInvocation {
    pub contract: UpstreamContract,
    pub action: DkmsAction,
    pub options: Vec<DkmsOption>,
    pub target: Option<DkmsTarget>,
}

impl DkmsInvocation {
    pub fn disposition(&self) -> InvocationDisposition {
        match self.action {
            DkmsAction::Help | DkmsAction::Version => InvocationDisposition::Information,
            DkmsAction::Status => InvocationDisposition::Status,
            _ => InvocationDisposition::Mutation,
        }
    }
}

#[derive(Default)]
struct ParseState {
    options: Vec<DkmsOption>,
    action: Option<DkmsAction>,
    information: Option<DkmsAction>,
    target: Option<DkmsTarget>,
    kernel_count: usize,
    architecture_count: usize,
    has_all: bool,
    has_module: bool,
    has_module_version: bool,
    has_archive: bool,
    has_template_kernel: bool,
    binaries_only: bool,
    source_only: bool,
}

pub(super) fn parse_dkms(argv: &[&str]) -> Result<DkmsInvocation, BootRuntimeParseError> {
    const COMMAND: &str = "dkms";
    let mut state = ParseState::default();
    let mut index = 0;

    while index < argv.len() {
        let token = argv[index];
        if token.starts_with('-') && token != "-" {
            let (name, joined) = split_long(token);
            if joined.is_some() && !takes_value(name) {
                return Err(BootRuntimeParseError::UnexpectedOptionValue {
                    command: COMMAND,
                    option: name.to_string(),
                });
            }
            match name {
                "-m" | "--module" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    let (module, version) = split_pair(&value);
                    if module.is_empty() {
                        return invalid(name, value, "a non-empty module or module/version");
                    }
                    if version.as_deref() == Some("") {
                        return invalid(name, value, "a non-empty module and module version");
                    }
                    state.has_module = true;
                    state.has_module_version |= version.is_some();
                    state.options.push(DkmsOption::Module {
                        name: module,
                        version,
                    });
                    index += consumed;
                }
                "-v" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    let value = nonempty(name, value, "a module version")?;
                    state.has_module_version = true;
                    state.options.push(DkmsOption::ModuleVersion(value));
                    index += consumed;
                }
                "-k" | "--kernelver" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    let (version, architecture) = split_pair(&value);
                    if version.is_empty() {
                        return invalid(name, value, "a kernel version or kernel/architecture");
                    }
                    if architecture.as_deref() == Some("") {
                        return invalid(name, value, "a kernel version or kernel/architecture");
                    }
                    state.kernel_count += 1;
                    if architecture.is_some() {
                        state.architecture_count += 1;
                    }
                    state.options.push(DkmsOption::Kernel {
                        version,
                        architecture,
                    });
                    index += consumed;
                }
                "-a" | "--arch" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    state.architecture_count += 1;
                    state.options.push(DkmsOption::Architecture(nonempty(
                        name,
                        value,
                        "an architecture",
                    )?));
                    index += consumed;
                }
                "-c" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    state.options.push(DkmsOption::Config(nonempty(
                        name,
                        value,
                        "a dkms.conf path",
                    )?));
                    index += consumed;
                }
                "-q" | "--quiet" => state.options.push(DkmsOption::Quiet),
                "-V" | "--version" => state.information = Some(DkmsAction::Version),
                "-h" | "--help" => state.information = Some(DkmsAction::Help),
                "--no-initrd" => state.options.push(DkmsOption::NoInitrdDeprecated),
                "--no-clean-kernel" => {
                    state.options.push(DkmsOption::NoCleanKernelDeprecated);
                }
                "--no-prepare-kernel" => {
                    state.options.push(DkmsOption::NoPrepareKernelDeprecated);
                }
                "--binaries-only" => {
                    state.binaries_only = true;
                    state.options.push(DkmsOption::BinariesOnly);
                }
                "--source-only" => {
                    state.source_only = true;
                    state.options.push(DkmsOption::SourceOnly);
                }
                "--force" => state.options.push(DkmsOption::Force),
                "--force-version-override" => {
                    state.options.push(DkmsOption::ForceVersionOverride);
                }
                "--all" => {
                    state.has_all = true;
                    state.options.push(DkmsOption::All);
                }
                "--verbose" => state.options.push(DkmsOption::Verbose),
                "--rpm_safe_upgrade" => state.options.push(DkmsOption::RpmSafeUpgrade),
                "--dkmstree" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    state.options.push(DkmsOption::DkmsTree(nonempty(
                        name,
                        value,
                        "a directory path",
                    )?));
                    index += consumed;
                }
                "--sourcetree" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    state.options.push(DkmsOption::SourceTree(nonempty(
                        name,
                        value,
                        "a directory path",
                    )?));
                    index += consumed;
                }
                "--installtree" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    state.options.push(DkmsOption::InstallTree(nonempty(
                        name,
                        value,
                        "a directory path",
                    )?));
                    index += consumed;
                }
                "--symlink-modules" => state.options.push(DkmsOption::SymlinkModules),
                "--config" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    state.options.push(DkmsOption::KernelConfig(nonempty(
                        name,
                        value,
                        "a kernel configuration path",
                    )?));
                    index += consumed;
                }
                "--archive" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    state.has_archive = true;
                    state.options.push(DkmsOption::Archive(nonempty(
                        name,
                        value,
                        "an archive path",
                    )?));
                    index += consumed;
                }
                "--kernelsourcedir" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    state
                        .options
                        .push(DkmsOption::KernelSourceDirectory(nonempty(
                            name,
                            value,
                            "a kernel source directory",
                        )?));
                    index += consumed;
                }
                "--directive" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    let (directive, directive_value) = value.split_once('=').ok_or_else(|| {
                        BootRuntimeParseError::InvalidOptionValue {
                            command: COMMAND,
                            option: name.to_string(),
                            value: value.clone(),
                            expected: "DIRECTIVE=VALUE",
                        }
                    })?;
                    if directive.is_empty() {
                        return invalid(name, value, "DIRECTIVE=VALUE");
                    }
                    state.options.push(DkmsOption::Directive {
                        name: directive.to_string(),
                        value: directive_value.to_string(),
                    });
                    index += consumed;
                }
                "--no-depmod" => state.options.push(DkmsOption::NoDepmod),
                "--modprobe-on-install" => {
                    state.options.push(DkmsOption::ModprobeOnInstall);
                }
                "--debug" => state.options.push(DkmsOption::Debug),
                "-j" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    let jobs = value.parse::<u32>().map_err(|_| {
                        BootRuntimeParseError::InvalidOptionValue {
                            command: COMMAND,
                            option: name.to_string(),
                            value,
                            expected: "an unsigned decimal job count",
                        }
                    })?;
                    state.options.push(DkmsOption::ParallelJobs(jobs));
                    index += consumed;
                }
                "--templatekernel" => {
                    let (value, consumed) = option_value(argv, index, name, joined)?;
                    let (version, architecture) = split_pair(&value);
                    if version.is_empty() {
                        return invalid(name, value, "a kernel version or kernel/architecture");
                    }
                    if architecture.as_deref() == Some("") {
                        return invalid(name, value, "a kernel version or kernel/architecture");
                    }
                    state.has_template_kernel = true;
                    state.options.push(DkmsOption::TemplateKernel {
                        version,
                        architecture,
                    });
                    index += consumed;
                }
                _ => {
                    return Err(BootRuntimeParseError::UnknownOption {
                        command: COMMAND,
                        option: token.to_string(),
                    });
                }
            }
        } else if let Some(action) = DkmsAction::parse(token) {
            if state.action.replace(action).is_some() {
                return Err(BootRuntimeParseError::ConflictingArguments {
                    command: COMMAND,
                    detail: "more than one action was specified",
                });
            }
        } else if state.target.is_none() {
            state.target = Some(DkmsTarget::FilesystemResolved(token.to_string()));
        } else {
            return Err(BootRuntimeParseError::UnexpectedOperand {
                command: COMMAND,
                action: state.action.map_or("unknown", DkmsAction::name),
                operand: token.to_string(),
            });
        }
        index += 1;
    }

    validate_state(&state)?;
    let action =
        state
            .information
            .or(state.action)
            .ok_or(BootRuntimeParseError::MissingAction {
                command: COMMAND,
                expected: "a DKMS action",
            })?;
    Ok(DkmsInvocation {
        contract: DKMS_CONTRACT,
        action,
        options: state.options,
        target: state.target,
    })
}

fn validate_state(state: &ParseState) -> Result<(), BootRuntimeParseError> {
    const COMMAND: &str = "dkms";
    if state.binaries_only && state.source_only {
        return Err(BootRuntimeParseError::ConflictingArguments {
            command: COMMAND,
            detail: "--binaries-only and --source-only are mutually exclusive",
        });
    }
    if state.has_all && state.kernel_count > 0 {
        return Err(BootRuntimeParseError::ConflictingArguments {
            command: COMMAND,
            detail: "--all cannot be combined with a kernel version",
        });
    }
    if state.has_all && state.architecture_count > 0 {
        return Err(BootRuntimeParseError::ConflictingArguments {
            command: COMMAND,
            detail: "--all cannot be combined with an architecture",
        });
    }
    if state.architecture_count > 1 && state.architecture_count != state.kernel_count {
        return Err(BootRuntimeParseError::ConflictingArguments {
            command: COMMAND,
            detail: "multiple architectures require a 1:1 kernel/architecture count",
        });
    }
    if state.information.is_some() {
        return Ok(());
    }
    let action = state.action.ok_or(BootRuntimeParseError::MissingAction {
        command: COMMAND,
        expected: "add, remove, build, unbuild, install, uninstall, match, autoinstall, mktarball, ldtarball, status, generate_mok, or a kernel hook action",
    })?;
    if state.has_all
        && matches!(
            action,
            DkmsAction::Add | DkmsAction::Build | DkmsAction::Install
        )
    {
        return Err(BootRuntimeParseError::ConflictingArguments {
            command: COMMAND,
            detail: "add, build, and install do not support --all",
        });
    }
    if matches!(
        action,
        DkmsAction::Autoinstall
            | DkmsAction::Match
            | DkmsAction::KernelPreInstall
            | DkmsAction::KernelPostInstall
            | DkmsAction::KernelPreRemove
    ) && (state.has_all || state.kernel_count > 1)
    {
        return Err(BootRuntimeParseError::ConflictingArguments {
            command: COMMAND,
            detail: "this action operates on exactly one kernel",
        });
    }
    if action == DkmsAction::Match && !state.has_template_kernel {
        return Err(BootRuntimeParseError::MissingOperand {
            command: COMMAND,
            action: "match",
            expected: "--templatekernel",
        });
    }
    if matches!(
        action,
        DkmsAction::Autoinstall
            | DkmsAction::GenerateMok
            | DkmsAction::KernelPreInstall
            | DkmsAction::KernelPostInstall
            | DkmsAction::KernelPreRemove
    ) && let Some(DkmsTarget::FilesystemResolved(operand)) = &state.target
    {
        return Err(BootRuntimeParseError::UnexpectedOperand {
            command: COMMAND,
            action: action.name(),
            operand: operand.clone(),
        });
    }
    let has_target = state.target.is_some()
        || (state.has_module && state.has_module_version)
        || (action == DkmsAction::LoadTarball && state.has_archive);
    if matches!(
        action,
        DkmsAction::Add
            | DkmsAction::Remove
            | DkmsAction::Build
            | DkmsAction::Unbuild
            | DkmsAction::Install
            | DkmsAction::Uninstall
            | DkmsAction::MakeTarball
            | DkmsAction::LoadTarball
    ) && !has_target
    {
        return Err(BootRuntimeParseError::MissingOperand {
            command: COMMAND,
            action: action.name(),
            expected: "a module/version or source/archive path",
        });
    }
    Ok(())
}

fn split_long(token: &str) -> (&str, Option<&str>) {
    if token.starts_with("--") {
        token
            .split_once('=')
            .map_or((token, None), |(name, value)| (name, Some(value)))
    } else {
        (token, None)
    }
}

fn takes_value(option: &str) -> bool {
    matches!(
        option,
        "--module"
            | "--kernelver"
            | "--templatekernel"
            | "--dkmstree"
            | "--sourcetree"
            | "--installtree"
            | "--config"
            | "--archive"
            | "--arch"
            | "--kernelsourcedir"
            | "--directive"
    )
}

fn option_value(
    argv: &[&str],
    index: usize,
    option: &str,
    joined: Option<&str>,
) -> Result<(String, usize), BootRuntimeParseError> {
    if let Some(value) = joined {
        return Ok((value.to_string(), 0));
    }
    argv.get(index + 1)
        .map(|value| ((*value).to_string(), 1))
        .ok_or_else(|| BootRuntimeParseError::MissingOptionValue {
            command: "dkms",
            option: option.to_string(),
            values: 1,
        })
}

fn split_pair(value: &str) -> (String, Option<String>) {
    value.split_once('/').map_or_else(
        || (value.to_string(), None),
        |(left, right)| (left.to_string(), Some(right.to_string())),
    )
}

fn nonempty(
    option: &str,
    value: String,
    expected: &'static str,
) -> Result<String, BootRuntimeParseError> {
    if value.is_empty() {
        invalid(option, value, expected)
    } else {
        Ok(value)
    }
}

fn invalid<T>(
    option: &str,
    value: String,
    expected: &'static str,
) -> Result<T, BootRuntimeParseError> {
    Err(BootRuntimeParseError::InvalidOptionValue {
        command: "dkms",
        option: option.to_string(),
        value,
        expected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_declared_options_and_module_install() {
        let parsed = parse_dkms(&[
            "install",
            "-m",
            "demo/1.2",
            "-k",
            "6.12",
            "-a",
            "x86_64",
            "-c",
            "/src/dkms.conf",
            "-q",
            "--force",
            "--force-version-override",
            "--verbose",
            "--rpm_safe_upgrade",
            "--dkmstree=/var/lib/dkms",
            "--sourcetree",
            "/usr/src",
            "--installtree=/lib/modules",
            "--symlink-modules",
            "--config=/kernel/.config",
            "--kernelsourcedir=/kernel",
            "--directive=BUILD_EXCLUSIVE_KERNEL=^6",
            "--no-depmod",
            "--modprobe-on-install",
            "--debug",
            "-j",
            "0",
        ])
        .unwrap();
        assert_eq!(parsed.action, DkmsAction::Install);
        assert_eq!(parsed.options.len(), 20);
    }

    #[test]
    fn covers_every_action_and_status_information_branches() {
        for action in [
            "add",
            "remove",
            "build",
            "unbuild",
            "install",
            "uninstall",
            "mktarball",
        ] {
            assert!(parse_dkms(&[action, "demo/1"]).is_ok(), "{action}");
        }
        assert!(parse_dkms(&["ldtarball", "--archive=/tmp/a.tar"]).is_ok());
        assert!(parse_dkms(&["match", "--templatekernel=6.11", "-k", "6.12"]).is_ok());
        assert!(parse_dkms(&["autoinstall"]).is_ok());
        assert!(parse_dkms(&["status"]).is_ok());
        assert!(parse_dkms(&["generate_mok"]).is_ok());
        for action in ["kernel_preinst", "kernel_postinst", "kernel_prerm"] {
            assert!(parse_dkms(&[action, "-k", "6.12"]).is_ok(), "{action}");
        }
        assert_eq!(parse_dkms(&["--help"]).unwrap().action, DkmsAction::Help);
        assert_eq!(
            parse_dkms(&["--version"]).unwrap().action,
            DkmsAction::Version
        );
    }

    #[test]
    fn covers_source_only_tarball_and_deprecated_flags() {
        let parsed = parse_dkms(&[
            "mktarball",
            "demo/1",
            "--source-only",
            "--archive=/tmp/demo.tar",
            "--no-initrd",
            "--no-clean-kernel",
            "--no-prepare-kernel",
        ])
        .unwrap();
        assert_eq!(parsed.options.len(), 5);

        assert!(parse_dkms(&["mktarball", "demo/1", "--binaries-only"]).is_ok());
    }

    #[test]
    fn rejects_prefixes_clusters_unknowns_and_contract_conflicts() {
        assert!(parse_dkms(&["status", "--module-prefix=demo"]).is_err());
        assert!(parse_dkms(&["status", "--force=yes"]).is_err());
        assert!(parse_dkms(&["status", "-qV"]).is_err());
        assert!(parse_dkms(&["status", "--"]).is_err());
        assert!(parse_dkms(&["build", "demo/1", "--all"]).is_err());
        assert!(parse_dkms(&["remove", "demo/1", "--all", "-k", "6.12"]).is_err());
        assert!(parse_dkms(&["mktarball", "demo/1", "--source-only", "--binaries-only"]).is_err());
        assert!(parse_dkms(&["match", "-k", "6.12"]).is_err());
        assert!(parse_dkms(&["build", "-m", "demo/"]).is_err());
        assert!(parse_dkms(&["autoinstall", "unexpected"]).is_err());
        assert!(parse_dkms(&["status", "one", "two"]).is_err());
    }

    #[test]
    fn keeps_filesystem_dependent_positional_semantics_explicit() {
        let invocation = parse_dkms(&["add", "/tmp/source-tree"]).unwrap();
        assert_eq!(
            invocation.target,
            Some(DkmsTarget::FilesystemResolved(
                "/tmp/source-tree".to_string()
            ))
        );
    }
}
