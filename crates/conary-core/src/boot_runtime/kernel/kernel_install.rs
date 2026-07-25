// crates/conary-core/src/boot_runtime/kernel/kernel_install.rs

use crate::boot_runtime::argv::{
    OptionSpec, only_operand_count, parse_boolean, require_nonempty, scan_gnu,
};
use crate::boot_runtime::{BootRuntimeParseError, InvocationDisposition, UpstreamContract};

pub const KERNEL_INSTALL_CONTRACT: UpstreamContract = UpstreamContract {
    project: "systemd",
    version: "262~devel",
    revision: "a8e93919c3fd4ff46ce7f1c7a0e73ea0a0a7eaeb",
    source_url: "https://github.com/systemd/systemd",
};

const OPTIONS: &[OptionSpec] = &[
    OptionSpec::flag("help", Some('h'), "help"),
    OptionSpec::flag("version", None, "version"),
    OptionSpec::flag("no-legend", None, "no-legend"),
    OptionSpec::flag("verbose", Some('v'), "verbose"),
    OptionSpec::value("esp-path", None, "esp-path"),
    OptionSpec::value("boot-path", None, "boot-path"),
    OptionSpec::value("make-entry-directory", None, "make-entry-directory"),
    OptionSpec::value("entry-token", None, "entry-token"),
    OptionSpec::value("entry-type", None, "entry-type"),
    OptionSpec::flag("no-pager", None, "no-pager"),
    OptionSpec::value("json", None, "json"),
    OptionSpec::value("root", None, "root"),
    OptionSpec::value("image", None, "image"),
    OptionSpec::value("image-policy", None, "image-policy"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemdTriState {
    Auto,
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootEntryToken {
    Auto,
    MachineId,
    OsId,
    OsImageId,
    Literal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootEntryType {
    All,
    Type1,
    Type2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonMode {
    Pretty,
    Short,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelInstallOption {
    NoLegend,
    Verbose,
    EspPath(String),
    BootPath(String),
    MakeEntryDirectory(SystemdTriState),
    EntryToken(BootEntryToken),
    EntryType(BootEntryType),
    NoPager,
    Json(JsonMode),
    Root(String),
    Image(String),
    ImagePolicy(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelInstallAction {
    Help,
    Version,
    JsonHelp,
    Add {
        kernel_version: Option<String>,
        kernel_image: Option<String>,
        initrds: Vec<String>,
    },
    AddAll,
    Remove {
        kernel_version: String,
        ignored_residual: Vec<String>,
    },
    Inspect {
        kernel_version: Option<String>,
        kernel_image: Option<String>,
        initrds: Vec<String>,
    },
    List,
    InstallKernelCompatibility {
        kernel_version: String,
        kernel_image: String,
        ignored_residual: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelInstallInvocation {
    pub contract: UpstreamContract,
    pub action: KernelInstallAction,
    pub options: Vec<KernelInstallOption>,
}

impl KernelInstallInvocation {
    pub fn disposition(&self) -> InvocationDisposition {
        match self.action {
            KernelInstallAction::Help
            | KernelInstallAction::Version
            | KernelInstallAction::JsonHelp => InvocationDisposition::Information,
            KernelInstallAction::Inspect { .. } | KernelInstallAction::List => {
                InvocationDisposition::Status
            }
            KernelInstallAction::Add { .. }
            | KernelInstallAction::AddAll
            | KernelInstallAction::Remove { .. }
            | KernelInstallAction::InstallKernelCompatibility { .. } => {
                InvocationDisposition::Mutation
            }
        }
    }
}

pub(super) fn parse_kernel_install(
    argv: &[&str],
) -> Result<KernelInstallInvocation, BootRuntimeParseError> {
    parse_kernel_command("kernel-install", argv, false)
}

pub(super) fn parse_installkernel(
    argv: &[&str],
) -> Result<KernelInstallInvocation, BootRuntimeParseError> {
    parse_kernel_command("installkernel", argv, true)
}

fn parse_kernel_command(
    command: &'static str,
    argv: &[&str],
    installkernel_compatibility: bool,
) -> Result<KernelInstallInvocation, BootRuntimeParseError> {
    let scanned = scan_gnu(command, argv, OPTIONS)?;
    let mut options = Vec::new();
    let mut information = None;
    let mut has_root = false;
    let mut has_image = false;

    for option in scanned.options {
        let value = || option.values[0].clone();
        match option.id {
            "help" => information = Some(KernelInstallAction::Help),
            "version" => information = Some(KernelInstallAction::Version),
            "no-legend" => options.push(KernelInstallOption::NoLegend),
            "verbose" => options.push(KernelInstallOption::Verbose),
            "esp-path" => options.push(KernelInstallOption::EspPath(value())),
            "boot-path" => options.push(KernelInstallOption::BootPath(value())),
            "make-entry-directory" => {
                let raw = value();
                let state = if raw == "auto" {
                    SystemdTriState::Auto
                } else {
                    SystemdTriState::Boolean(parse_boolean(command, &option.spelling, raw)?)
                };
                options.push(KernelInstallOption::MakeEntryDirectory(state));
            }
            "entry-token" => options.push(KernelInstallOption::EntryToken(parse_entry_token(
                command,
                &option.spelling,
                value(),
            )?)),
            "entry-type" => options.push(KernelInstallOption::EntryType(match value().as_str() {
                "" | "all" => BootEntryType::All,
                "type1" => BootEntryType::Type1,
                "type2" => BootEntryType::Type2,
                invalid => {
                    return Err(BootRuntimeParseError::InvalidOptionValue {
                        command,
                        option: option.spelling,
                        value: invalid.to_string(),
                        expected: "type1, type2, or all",
                    });
                }
            })),
            "no-pager" => options.push(KernelInstallOption::NoPager),
            "json" => match value().as_str() {
                "pretty" => options.push(KernelInstallOption::Json(JsonMode::Pretty)),
                "short" => options.push(KernelInstallOption::Json(JsonMode::Short)),
                "off" => options.push(KernelInstallOption::Json(JsonMode::Off)),
                "help" => information = Some(KernelInstallAction::JsonHelp),
                invalid => {
                    return Err(BootRuntimeParseError::InvalidOptionValue {
                        command,
                        option: option.spelling,
                        value: invalid.to_string(),
                        expected: "pretty, short, off, or help",
                    });
                }
            },
            "root" => {
                let path = value();
                has_root = !path.is_empty();
                options.push(KernelInstallOption::Root(path));
            }
            "image" => {
                let path = value();
                has_image = !path.is_empty();
                options.push(KernelInstallOption::Image(path));
            }
            "image-policy" => options.push(KernelInstallOption::ImagePolicy(require_nonempty(
                command,
                &option.spelling,
                value(),
                "a systemd image policy",
            )?)),
            _ => unreachable!("closed kernel-install option table"),
        }
    }

    if has_root && has_image {
        return Err(BootRuntimeParseError::ConflictingArguments {
            command,
            detail: "--root and --image are mutually exclusive",
        });
    }

    let action = if let Some(action) = information {
        action
    } else if installkernel_compatibility {
        parse_installkernel_action(&scanned.operands)?
    } else {
        parse_action(&scanned.operands)?
    };

    if (has_root || has_image)
        && matches!(
            action,
            KernelInstallAction::Add { .. }
                | KernelInstallAction::AddAll
                | KernelInstallAction::Remove { .. }
                | KernelInstallAction::InstallKernelCompatibility { .. }
        )
    {
        return Err(BootRuntimeParseError::ConflictingArguments {
            command,
            detail: "kernel mutation commands do not support --root or --image",
        });
    }

    Ok(KernelInstallInvocation {
        contract: KERNEL_INSTALL_CONTRACT,
        action,
        options,
    })
}

fn parse_installkernel_action(
    operands: &[String],
) -> Result<KernelInstallAction, BootRuntimeParseError> {
    const COMMAND: &str = "installkernel";
    only_operand_count(
        COMMAND,
        "install",
        operands,
        2,
        usize::MAX,
        "VERSION and VMLINUZ",
    )?;
    if !filename_is_valid(&operands[0]) {
        return Err(BootRuntimeParseError::InvalidOptionValue {
            command: COMMAND,
            option: "kernel-version".to_string(),
            value: operands[0].clone(),
            expected: "a non-empty filename-safe kernel version",
        });
    }
    Ok(KernelInstallAction::InstallKernelCompatibility {
        kernel_version: operands[0].clone(),
        kernel_image: operands[1].clone(),
        // systemd passes only VERSION and VMLINUZ to verb_add and ignores
        // every kernel-build compatibility residual.
        ignored_residual: operands[2..].to_vec(),
    })
}

fn parse_action(operands: &[String]) -> Result<KernelInstallAction, BootRuntimeParseError> {
    const COMMAND: &str = "kernel-install";
    if operands.is_empty() {
        return Ok(KernelInstallAction::Inspect {
            kernel_version: None,
            kernel_image: None,
            initrds: Vec::new(),
        });
    }

    match operands[0].as_str() {
        "help" => Ok(KernelInstallAction::Help),
        "add" => parse_add_like(&operands[1..], true),
        "inspect" => parse_add_like(&operands[1..], false),
        "add-all" => {
            only_operand_count(COMMAND, "add-all", &operands[1..], 0, 0, "no operands")?;
            Ok(KernelInstallAction::AddAll)
        }
        "remove" => {
            only_operand_count(
                COMMAND,
                "remove",
                &operands[1..],
                1,
                usize::MAX,
                "a kernel version",
            )?;
            if !filename_is_valid(&operands[1]) {
                return Err(BootRuntimeParseError::InvalidOptionValue {
                    command: COMMAND,
                    option: "kernel-version".to_string(),
                    value: operands[1].clone(),
                    expected: "a non-empty filename-safe kernel version",
                });
            }
            Ok(KernelInstallAction::Remove {
                kernel_version: operands[1].clone(),
                // systemd 262 explicitly logs and ignores these residuals.
                ignored_residual: operands[2..].to_vec(),
            })
        }
        "list" => {
            only_operand_count(COMMAND, "list", &operands[1..], 0, 0, "no operands")?;
            Ok(KernelInstallAction::List)
        }
        action => Err(BootRuntimeParseError::UnknownAction {
            command: COMMAND,
            action: action.to_string(),
        }),
    }
}

fn parse_add_like(
    operands: &[String],
    add: bool,
) -> Result<KernelInstallAction, BootRuntimeParseError> {
    let value = |value: &String| {
        if value.is_empty() || value == "-" {
            None
        } else {
            Some(value.clone())
        }
    };
    let (kernel_version, kernel_image, initrds) = match operands {
        [] => (None, None, Vec::new()),
        [image] => (None, value(image), Vec::new()),
        [version, image, initrds @ ..] => (value(version), value(image), initrds.to_vec()),
    };
    if let Some(version) = &kernel_version
        && !filename_is_valid(version)
    {
        return Err(BootRuntimeParseError::InvalidOptionValue {
            command: "kernel-install",
            option: "kernel-version".to_string(),
            value: version.clone(),
            expected: "a non-empty filename-safe kernel version",
        });
    }
    if add {
        Ok(KernelInstallAction::Add {
            kernel_version,
            kernel_image,
            initrds,
        })
    } else {
        Ok(KernelInstallAction::Inspect {
            kernel_version,
            kernel_image,
            initrds,
        })
    }
}

fn parse_entry_token(
    command: &'static str,
    option: &str,
    value: String,
) -> Result<BootEntryToken, BootRuntimeParseError> {
    match value.as_str() {
        "auto" => Ok(BootEntryToken::Auto),
        "machine-id" => Ok(BootEntryToken::MachineId),
        "os-id" => Ok(BootEntryToken::OsId),
        "os-image-id" => Ok(BootEntryToken::OsImageId),
        _ => {
            if let Some(literal) = value.strip_prefix("literal:")
                && filename_is_valid(literal)
                && literal.chars().all(|character| !character.is_control())
            {
                return Ok(BootEntryToken::Literal(literal.to_string()));
            }
            Err(BootRuntimeParseError::InvalidOptionValue {
                command,
                option: option.to_string(),
                value,
                expected: "auto, machine-id, os-id, os-image-id, or literal:FILENAME",
            })
        }
    }
}

fn filename_is_valid(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains('/') && value.len() <= 255
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_all_options_and_add_positional_forms() {
        let parsed = parse_kernel_install(&[
            "--no-legend",
            "-v",
            "--esp-path",
            "/efi",
            "--boot-path=/boot",
            "--make-entry-directory=auto",
            "--entry-token=literal:demo",
            "--entry-type=type2",
            "--no-pager",
            "--json=pretty",
            "add",
            "6.12",
            "/vmlinuz",
            "/initrd",
        ])
        .unwrap();
        assert_eq!(parsed.options.len(), 9);
        assert!(matches!(
            parsed.action,
            KernelInstallAction::Add {
                kernel_version: Some(ref version),
                kernel_image: Some(ref image),
                ref initrds,
            } if version == "6.12" && image == "/vmlinuz" && initrds == &["/initrd"]
        ));

        assert!(matches!(
            parse_kernel_install(&["add", "/vmlinuz"]).unwrap().action,
            KernelInstallAction::Add {
                kernel_version: None,
                kernel_image: Some(_),
                ..
            }
        ));
        assert!(matches!(
            parse_kernel_install(&["add", "-", "-", "/initrd"])
                .unwrap()
                .action,
            KernelInstallAction::Add {
                kernel_version: None,
                kernel_image: None,
                ..
            }
        ));
    }

    #[test]
    fn covers_information_and_status_branches() {
        assert!(matches!(
            parse_kernel_install(&[]).unwrap().action,
            KernelInstallAction::Inspect { .. }
        ));
        assert!(matches!(
            parse_kernel_install(&["list"]).unwrap().action,
            KernelInstallAction::List
        ));
        assert!(matches!(
            parse_kernel_install(&["--help"]).unwrap().action,
            KernelInstallAction::Help
        ));
        assert!(matches!(
            parse_kernel_install(&["--version"]).unwrap().action,
            KernelInstallAction::Version
        ));
        assert!(matches!(
            parse_kernel_install(&["--json=help"]).unwrap().action,
            KernelInstallAction::JsonHelp
        ));
    }

    #[test]
    fn preserves_source_declared_remove_residuals() {
        assert!(matches!(
            parse_kernel_install(&["remove", "6.12", "ignored"])
                .unwrap()
                .action,
            KernelInstallAction::Remove {
                ref ignored_residual,
                ..
            } if ignored_residual == &["ignored"]
        ));
    }

    #[test]
    fn installkernel_is_a_distinct_source_declared_compatibility_grammar() {
        let parsed = parse_installkernel(&[
            "-v",
            "6.12.1",
            "/boot/vmlinuz-6.12.1",
            "/boot/System.map-6.12.1",
            "/boot",
            "ignored-by-systemd",
        ])
        .unwrap();
        assert!(matches!(
            parsed.action,
            KernelInstallAction::InstallKernelCompatibility {
                ref kernel_version,
                ref kernel_image,
                ref ignored_residual,
            } if kernel_version == "6.12.1"
                && kernel_image == "/boot/vmlinuz-6.12.1"
                && ignored_residual
                    == &[
                        "/boot/System.map-6.12.1",
                        "/boot",
                        "ignored-by-systemd",
                    ]
        ));
        assert!(matches!(
            parse_kernel_install(&["6.12.1", "/boot/vmlinuz-6.12.1"]),
            Err(BootRuntimeParseError::UnknownAction { .. })
        ));
        assert!(parse_installkernel(&["6.12.1"]).is_err());
        assert!(
            parse_installkernel(&["--root=/target", "6.12.1", "/boot/vmlinuz-6.12.1"]).is_err()
        );
    }

    #[test]
    fn rejects_unknown_values_conflicts_and_arity_errors() {
        assert!(parse_kernel_install(&["--entry-type=type3"]).is_err());
        assert!(parse_kernel_install(&["--json=yaml"]).is_err());
        assert!(parse_kernel_install(&["--entry-token=literal:a/b"]).is_err());
        assert!(parse_kernel_install(&["--make-entry-directory="]).is_err());
        assert!(parse_kernel_install(&["--root=/root", "--image=/image", "inspect"]).is_err());
        assert!(parse_kernel_install(&["--root=", "--image=", "inspect"]).is_ok());
        assert!(parse_kernel_install(&["--root=/root", "add"]).is_err());
        assert!(parse_kernel_install(&["add", "bad/version", "/vmlinuz"]).is_err());
        assert!(parse_kernel_install(&["list", "extra"]).is_err());
        assert!(parse_kernel_install(&["unknown"]).is_err());
    }
}
