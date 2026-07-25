// crates/conary-core/src/boot_runtime/initramfs/mkinitcpio.rs

use crate::boot_runtime::argv::{OptionSpec, require_nonempty, scan_gnu};
use crate::boot_runtime::{BootRuntimeParseError, InvocationDisposition, UpstreamContract};

pub const MKINITCPIO_CONTRACT: UpstreamContract = UpstreamContract {
    project: "mkinitcpio",
    version: "unreleased master (2026-05-27)",
    revision: "562a817d7f81179c98f62699ec02b93783db1bc5",
    source_url: "https://gitlab.archlinux.org/archlinux/mkinitcpio/mkinitcpio",
};

const OPTIONS: &[OptionSpec] = &[
    OptionSpec::value("addhooks", Some('A'), "add"),
    OptionSpec::value("addhooks", None, "addhooks"),
    OptionSpec::value("include", Some('I'), "include"),
    OptionSpec::value("config", Some('c'), "config"),
    OptionSpec::value("generate", Some('g'), "generate"),
    OptionSpec::value("hookdir", Some('D'), "hookdir"),
    OptionSpec::value("hookhelp", Some('H'), "hookhelp"),
    OptionSpec::flag("help", Some('h'), "help"),
    OptionSpec::value("kernel", Some('k'), "kernel"),
    OptionSpec::flag("listhooks", Some('L'), "listhooks"),
    OptionSpec::flag("automods", Some('M'), "automods"),
    OptionSpec::value("moduleroot", Some('r'), "moduleroot"),
    OptionSpec::flag("nocolor", Some('n'), "nocolor"),
    OptionSpec::flag("nopost", None, "nopost"),
    OptionSpec::flag("allpresets", Some('P'), "allpresets"),
    OptionSpec::value("preset", Some('p'), "preset"),
    OptionSpec::flag("remove", Some('R'), "remove"),
    OptionSpec::value("skiphooks", Some('S'), "skiphooks"),
    OptionSpec::flag("save", Some('s'), "save"),
    OptionSpec::value("generatedir", Some('d'), "generatedir"),
    OptionSpec::value("builddir", Some('t'), "builddir"),
    OptionSpec::flag("version", Some('V'), "version"),
    OptionSpec::flag("verbose", Some('v'), "verbose"),
    OptionSpec::value("compress", Some('z'), "compress"),
    OptionSpec::value("uki", Some('U'), "uki"),
    OptionSpec::value("uki", None, "uefi"),
    OptionSpec::value("microcode", None, "microcode"),
    OptionSpec::value("splash", None, "splash"),
    OptionSpec::value("kernelimage", None, "kernelimage"),
    OptionSpec::value("uefistub", None, "uefistub"),
    OptionSpec::value("cmdline", None, "cmdline"),
    OptionSpec::value("osrelease", None, "osrelease"),
    OptionSpec::flag("no-cmdline", None, "no-cmdline"),
    OptionSpec::value("ukiconfig", None, "ukiconfig"),
    OptionSpec::flag("no-ukify", None, "no-ukify"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MkinitcpioOption {
    AddHooks(Vec<String>),
    Include(Vec<String>),
    Config(String),
    Generate(String),
    HookDirectory(String),
    Kernel(String),
    ModuleRoot(String),
    NoColor,
    NoPostHooks,
    AllPresets,
    Preset(String),
    Remove,
    SkipHooks(Vec<String>),
    SaveBuildDirectory,
    GenerateDirectory(String),
    BuildDirectory(String),
    Verbose,
    Compressor(String),
    UnifiedKernelImage(String),
    DeprecatedUefiAlias(String),
    DeprecatedMicrocode(String),
    Splash(String),
    KernelImage(String),
    UefiStub(String),
    CommandLine(String),
    OsRelease(String),
    NoCommandLine,
    UkiConfig(String),
    NoUkify,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MkinitcpioAction {
    Help,
    Version,
    HookHelp(String),
    ListHooks,
    AutoModules,
    RemovePresets,
    ProcessPresets,
    Build,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkinitcpioInvocation {
    pub contract: UpstreamContract,
    pub action: MkinitcpioAction,
    pub options: Vec<MkinitcpioOption>,
    /// `mkinitcpio` explicitly retains `OPTREST` for preset worker calls.
    pub preset_passthrough: Vec<String>,
}

impl MkinitcpioInvocation {
    pub fn disposition(&self) -> InvocationDisposition {
        match self.action {
            MkinitcpioAction::Help
            | MkinitcpioAction::Version
            | MkinitcpioAction::HookHelp(_)
            | MkinitcpioAction::ListHooks => InvocationDisposition::Information,
            MkinitcpioAction::AutoModules => InvocationDisposition::Status,
            MkinitcpioAction::RemovePresets
            | MkinitcpioAction::ProcessPresets
            | MkinitcpioAction::Build => InvocationDisposition::Mutation,
        }
    }
}

pub(super) fn parse_mkinitcpio(
    argv: &[&str],
) -> Result<MkinitcpioInvocation, BootRuntimeParseError> {
    const COMMAND: &str = "mkinitcpio";
    let scanned = scan_gnu(COMMAND, argv, OPTIONS)?;
    let mut options = Vec::new();
    let mut information = None;
    let mut has_presets = false;
    let mut remove = false;

    for option in scanned.options {
        let value = || option.values[0].clone();
        match option.id {
            "addhooks" => options.push(MkinitcpioOption::AddHooks(comma_values(
                &option.spelling,
                value(),
                "a comma-separated hook list",
            )?)),
            "include" => options.push(MkinitcpioOption::Include(comma_values(
                &option.spelling,
                value(),
                "a comma-separated path list",
            )?)),
            "config" => options.push(MkinitcpioOption::Config(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a configuration path",
            )?)),
            "generate" => options.push(MkinitcpioOption::Generate(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "an output path",
            )?)),
            "hookdir" => options.push(MkinitcpioOption::HookDirectory(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a hook directory",
            )?)),
            "hookhelp" => {
                let hook = require_nonempty(COMMAND, &option.spelling, value(), "a hook name")?;
                information.get_or_insert(MkinitcpioAction::HookHelp(hook));
            }
            "help" => {
                information.get_or_insert(MkinitcpioAction::Help);
            }
            "kernel" => options.push(MkinitcpioOption::Kernel(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a kernel version or image path",
            )?)),
            "listhooks" => {
                information.get_or_insert(MkinitcpioAction::ListHooks);
            }
            "automods" => {
                information.get_or_insert(MkinitcpioAction::AutoModules);
            }
            "moduleroot" => options.push(MkinitcpioOption::ModuleRoot(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a module root directory",
            )?)),
            "nocolor" => options.push(MkinitcpioOption::NoColor),
            "nopost" => options.push(MkinitcpioOption::NoPostHooks),
            "allpresets" => {
                has_presets = true;
                options.push(MkinitcpioOption::AllPresets);
            }
            "preset" => {
                has_presets = true;
                options.push(MkinitcpioOption::Preset(require_nonempty(
                    COMMAND,
                    &option.spelling,
                    value(),
                    "a preset",
                )?));
            }
            "remove" => {
                remove = true;
                options.push(MkinitcpioOption::Remove);
            }
            "skiphooks" => options.push(MkinitcpioOption::SkipHooks(comma_values(
                &option.spelling,
                value(),
                "a comma-separated hook list",
            )?)),
            "save" => options.push(MkinitcpioOption::SaveBuildDirectory),
            "generatedir" => options.push(MkinitcpioOption::GenerateDirectory(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a generated image directory",
            )?)),
            "builddir" => options.push(MkinitcpioOption::BuildDirectory(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a temporary build directory",
            )?)),
            "version" => {
                information.get_or_insert(MkinitcpioAction::Version);
            }
            "verbose" => options.push(MkinitcpioOption::Verbose),
            "compress" => options.push(MkinitcpioOption::Compressor(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a compressor program",
            )?)),
            "uki" => {
                let path = require_nonempty(
                    COMMAND,
                    &option.spelling,
                    value(),
                    "a unified kernel image path",
                )?;
                if option.spelling == "--uefi" {
                    options.push(MkinitcpioOption::DeprecatedUefiAlias(path));
                } else {
                    options.push(MkinitcpioOption::UnifiedKernelImage(path));
                }
            }
            "microcode" => {
                has_presets = true;
                options.push(MkinitcpioOption::DeprecatedMicrocode(value()));
            }
            "splash" => options.push(MkinitcpioOption::Splash(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a splash image path",
            )?)),
            "kernelimage" => options.push(MkinitcpioOption::KernelImage(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a kernel image path",
            )?)),
            "uefistub" => options.push(MkinitcpioOption::UefiStub(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a UEFI stub path",
            )?)),
            "cmdline" => options.push(MkinitcpioOption::CommandLine(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a command line file or directory",
            )?)),
            "osrelease" => options.push(MkinitcpioOption::OsRelease(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "an os-release path",
            )?)),
            "no-cmdline" => options.push(MkinitcpioOption::NoCommandLine),
            "ukiconfig" => options.push(MkinitcpioOption::UkiConfig(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a UKI configuration path",
            )?)),
            "no-ukify" => options.push(MkinitcpioOption::NoUkify),
            _ => unreachable!("closed mkinitcpio option table"),
        }
    }

    if remove && !has_presets {
        return Err(BootRuntimeParseError::ConflictingArguments {
            command: COMMAND,
            detail: "--remove requires --preset or --allpresets",
        });
    }
    if !has_presets && !scanned.operands.is_empty() {
        return Err(BootRuntimeParseError::UnexpectedOperand {
            command: COMMAND,
            action: "build",
            operand: scanned.operands[0].clone(),
        });
    }

    let action = information.unwrap_or({
        if remove {
            MkinitcpioAction::RemovePresets
        } else if has_presets {
            MkinitcpioAction::ProcessPresets
        } else {
            MkinitcpioAction::Build
        }
    });
    Ok(MkinitcpioInvocation {
        contract: MKINITCPIO_CONTRACT,
        action,
        options,
        preset_passthrough: scanned.operands,
    })
}

fn comma_values(
    option: &str,
    value: String,
    expected: &'static str,
) -> Result<Vec<String>, BootRuntimeParseError> {
    let values: Vec<String> = value.split(',').map(str::to_string).collect();
    if values.iter().any(String::is_empty) {
        Err(BootRuntimeParseError::InvalidOptionValue {
            command: "mkinitcpio",
            option: option.to_string(),
            value,
            expected,
        })
    } else {
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_every_build_option_and_declared_alias() {
        let parsed = parse_mkinitcpio(&[
            "-Abase,udev",
            "--addhooks=filesystems",
            "-I/etc/one,/etc/two",
            "-c/etc/mkinitcpio.conf",
            "-g/boot/initramfs.img",
            "-D/hooks",
            "-k6.12",
            "-r/",
            "-n",
            "--nopost",
            "-Sfsck",
            "-s",
            "-d/generated",
            "-t/tmp",
            "-v",
            "-z",
            "zstd",
            "-U/boot/linux.efi",
            "--uefi=/boot/deprecated.efi",
            "--splash=/splash.bmp",
            "--kernelimage=/vmlinuz",
            "--uefistub=/stub.efi",
            "--cmdline=/etc/kernel/cmdline",
            "--osrelease=/etc/os-release",
            "--no-cmdline",
            "--ukiconfig=/etc/kernel/uki.conf",
            "--no-ukify",
        ])
        .unwrap();
        assert_eq!(parsed.action, MkinitcpioAction::Build);
        assert_eq!(parsed.options.len(), 26);
    }

    #[test]
    fn covers_information_and_status_actions() {
        assert_eq!(
            parse_mkinitcpio(&["--help"]).unwrap().action,
            MkinitcpioAction::Help
        );
        assert_eq!(
            parse_mkinitcpio(&["-V"]).unwrap().action,
            MkinitcpioAction::Version
        );
        assert!(matches!(
            parse_mkinitcpio(&["-H", "base"]).unwrap().action,
            MkinitcpioAction::HookHelp(ref hook) if hook == "base"
        ));
        assert_eq!(
            parse_mkinitcpio(&["-L"]).unwrap().action,
            MkinitcpioAction::ListHooks
        );
        assert_eq!(
            parse_mkinitcpio(&["-M"]).unwrap().action,
            MkinitcpioAction::AutoModules
        );
    }

    #[test]
    fn covers_preset_remove_deprecated_microcode_and_passthrough() {
        let parsed = parse_mkinitcpio(&["-p", "linux", "-R", "--", "--worker-option"]).unwrap();
        assert_eq!(parsed.action, MkinitcpioAction::RemovePresets);
        assert_eq!(parsed.preset_passthrough, ["--worker-option"]);

        let parsed = parse_mkinitcpio(&["--microcode=deprecated"]).unwrap();
        assert_eq!(parsed.action, MkinitcpioAction::ProcessPresets);
        assert_eq!(parsed.options.len(), 1);

        assert_eq!(
            parse_mkinitcpio(&["-P"]).unwrap().action,
            MkinitcpioAction::ProcessPresets
        );
    }

    #[test]
    fn rejects_unknown_missing_and_malformed_forms() {
        assert!(parse_mkinitcpio(&["--remove"]).is_err());
        assert!(parse_mkinitcpio(&["operand"]).is_err());
        assert!(parse_mkinitcpio(&["--future"]).is_err());
        assert!(parse_mkinitcpio(&["--addhooks=a,,b"]).is_err());
        assert!(parse_mkinitcpio(&["--uefi"]).is_err());
        assert!(parse_mkinitcpio(&["--microcode"]).is_err());
    }
}
