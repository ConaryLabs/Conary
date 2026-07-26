// crates/conary-core/src/boot_runtime/kernel/modprobe.rs

use crate::boot_runtime::argv::{OptionSpec, require_nonempty, scan_gnu};
use crate::boot_runtime::{BootRuntimeParseError, InvocationDisposition, UpstreamContract};

pub const MODPROBE_CONTRACT: UpstreamContract = UpstreamContract {
    project: "kmod",
    version: "34",
    revision: "84fa569b2eb3230284168d59863330bafb07bc52",
    source_url: "https://github.com/kmod-project/kmod",
};

const COMMAND: &str = "modprobe";
const OPTIONS: &[OptionSpec] = &[
    OptionSpec::flag("all", Some('a'), "all"),
    OptionSpec::flag("remove", Some('r'), "remove"),
    OptionSpec::flag("remove-holders", None, "remove-holders"),
    OptionSpec::flag("remove-holders", None, "remove-dependencies"),
    OptionSpec::value("wait", Some('w'), "wait"),
    OptionSpec::flag("resolve-alias", None, "resolve-alias"),
    OptionSpec::flag("resolve-alias", Some('R'), "show-alias"),
    OptionSpec::flag("first-time", None, "first-time"),
    OptionSpec::flag("ignore-commands", Some('i'), "ignore-install"),
    OptionSpec::flag("ignore-commands", None, "ignore-remove"),
    OptionSpec::flag("use-blacklist", Some('b'), "use-blacklist"),
    OptionSpec::flag("force", Some('f'), "force"),
    OptionSpec::flag("force-modversion", None, "force-modversion"),
    OptionSpec::flag("force-vermagic", None, "force-vermagic"),
    OptionSpec::flag("show-depends", Some('D'), "show-depends"),
    OptionSpec::flag("show-config", None, "showconfig"),
    OptionSpec::flag("show-config", Some('c'), "show-config"),
    OptionSpec::flag("show-modversions", None, "show-modversions"),
    OptionSpec::flag("show-modversions", None, "dump-modversions"),
    OptionSpec::flag("show-exports", None, "show-exports"),
    OptionSpec::flag("dry-run", Some('n'), "dry-run"),
    OptionSpec::flag("dry-run", None, "show"),
    OptionSpec::value("config", Some('C'), "config"),
    OptionSpec::value("dirname", Some('d'), "dirname"),
    OptionSpec::value("set-version", Some('S'), "set-version"),
    OptionSpec::flag("syslog", Some('s'), "syslog"),
    OptionSpec::flag("quiet", Some('q'), "quiet"),
    OptionSpec::flag("verbose", Some('v'), "verbose"),
    OptionSpec::flag("version", Some('V'), "version"),
    OptionSpec::flag("help", Some('h'), "help"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModprobeOption {
    All,
    Remove,
    RemoveHolders,
    WaitMilliseconds(u64),
    ResolveAlias,
    FirstTime,
    IgnoreInstallRemoveCommands,
    UseBlacklist,
    Force,
    ForceModuleVersion,
    ForceVersionMagic,
    ShowDependencies,
    ShowConfig,
    ShowModuleVersions,
    ShowExports,
    DryRun,
    Config(String),
    DirectoryRoot(String),
    KernelVersion(String),
    Syslog,
    Quiet,
    Verbose,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModprobeOperation {
    Insert {
        module: String,
        parameters: Vec<String>,
    },
    InsertAll {
        modules: Vec<String>,
    },
    Remove {
        modules: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModprobeAction {
    Help,
    Version,
    ShowConfig {
        ignored_operands: Vec<String>,
    },
    ShowModuleVersions {
        module: String,
        ignored_operands: Vec<String>,
    },
    ShowExports {
        module: String,
        ignored_operands: Vec<String>,
    },
    Query {
        operation: ModprobeOperation,
    },
    Mutate {
        operation: ModprobeOperation,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModprobeInvocation {
    pub contract: UpstreamContract,
    pub action: ModprobeAction,
    pub options: Vec<ModprobeOption>,
}

impl ModprobeInvocation {
    pub fn disposition(&self) -> InvocationDisposition {
        match self.action {
            ModprobeAction::Help | ModprobeAction::Version => InvocationDisposition::Information,
            ModprobeAction::ShowConfig { .. }
            | ModprobeAction::ShowModuleVersions { .. }
            | ModprobeAction::ShowExports { .. }
            | ModprobeAction::Query { .. } => InvocationDisposition::Status,
            ModprobeAction::Mutate { .. } => InvocationDisposition::Mutation,
        }
    }
}

pub(super) fn parse_modprobe(argv: &[&str]) -> Result<ModprobeInvocation, BootRuntimeParseError> {
    let scanned = scan_gnu(COMMAND, argv, OPTIONS)?;
    let mut options = Vec::new();
    let mut information = None;
    let mut remove = false;
    let mut all = false;
    let mut query = false;
    let mut resolve_alias = false;
    let mut show_config = false;
    let mut show_modversions = false;
    let mut show_exports = false;

    for option in scanned.options {
        let value = || option.values[0].clone();
        match option.id {
            "all" => {
                all = true;
                options.push(ModprobeOption::All);
            }
            "remove" => {
                remove = true;
                options.push(ModprobeOption::Remove);
            }
            "remove-holders" => {
                remove = true;
                options.push(ModprobeOption::RemoveHolders);
            }
            "wait" => {
                let raw = value();
                let milliseconds = parse_unsigned_long(&raw).ok_or_else(|| {
                    BootRuntimeParseError::InvalidOptionValue {
                        command: COMMAND,
                        option: option.spelling.clone(),
                        value: raw,
                        expected: "a base-0 unsigned millisecond value",
                    }
                })?;
                options.push(ModprobeOption::WaitMilliseconds(milliseconds));
            }
            "resolve-alias" => {
                resolve_alias = true;
                options.push(ModprobeOption::ResolveAlias);
            }
            "first-time" => options.push(ModprobeOption::FirstTime),
            "ignore-commands" => {
                options.push(ModprobeOption::IgnoreInstallRemoveCommands);
            }
            "use-blacklist" => options.push(ModprobeOption::UseBlacklist),
            "force" => options.push(ModprobeOption::Force),
            "force-modversion" => options.push(ModprobeOption::ForceModuleVersion),
            "force-vermagic" => options.push(ModprobeOption::ForceVersionMagic),
            "show-depends" => {
                query = true;
                options.push(ModprobeOption::ShowDependencies);
            }
            "show-config" => {
                show_config = true;
                options.push(ModprobeOption::ShowConfig);
            }
            "show-modversions" => {
                show_modversions = true;
                options.push(ModprobeOption::ShowModuleVersions);
            }
            "show-exports" => {
                show_exports = true;
                options.push(ModprobeOption::ShowExports);
            }
            "dry-run" => {
                query = true;
                options.push(ModprobeOption::DryRun);
            }
            "config" => options.push(ModprobeOption::Config(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a configuration path",
            )?)),
            "dirname" => options.push(ModprobeOption::DirectoryRoot(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a filesystem root",
            )?)),
            "set-version" => options.push(ModprobeOption::KernelVersion(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a kernel version",
            )?)),
            "syslog" => options.push(ModprobeOption::Syslog),
            "quiet" => options.push(ModprobeOption::Quiet),
            "verbose" => options.push(ModprobeOption::Verbose),
            "version" if information.is_none() => information = Some(ModprobeAction::Version),
            "help" if information.is_none() => information = Some(ModprobeAction::Help),
            "version" | "help" => {}
            _ => unreachable!("closed modprobe option table"),
        }
    }

    let action = if let Some(action) = information {
        action
    } else if show_config {
        ModprobeAction::ShowConfig {
            ignored_operands: scanned.operands,
        }
    } else if show_modversions {
        let (module, ignored_operands) = first_module(scanned.operands, "show-modversions")?;
        ModprobeAction::ShowModuleVersions {
            module,
            ignored_operands,
        }
    } else if show_exports {
        let (module, ignored_operands) = first_module(scanned.operands, "show-exports")?;
        ModprobeAction::ShowExports {
            module,
            ignored_operands,
        }
    } else {
        let operation = parse_operation(scanned.operands, remove, all)?;
        if query || (resolve_alias && !remove) {
            ModprobeAction::Query { operation }
        } else {
            ModprobeAction::Mutate { operation }
        }
    };

    Ok(ModprobeInvocation {
        contract: MODPROBE_CONTRACT,
        action,
        options,
    })
}

fn first_module(
    mut operands: Vec<String>,
    action: &'static str,
) -> Result<(String, Vec<String>), BootRuntimeParseError> {
    if operands.is_empty() {
        return Err(BootRuntimeParseError::MissingOperand {
            command: COMMAND,
            action,
            expected: "a module path",
        });
    }
    let module = operands.remove(0);
    Ok((module, operands))
}

fn parse_operation(
    mut operands: Vec<String>,
    remove: bool,
    all: bool,
) -> Result<ModprobeOperation, BootRuntimeParseError> {
    if operands.is_empty() {
        return Err(BootRuntimeParseError::MissingOperand {
            command: COMMAND,
            action: if remove { "remove" } else { "insert" },
            expected: "at least one module name or alias",
        });
    }
    if remove {
        return Ok(ModprobeOperation::Remove { modules: operands });
    }
    if all {
        return Ok(ModprobeOperation::InsertAll { modules: operands });
    }
    let module = operands.remove(0);
    Ok(ModprobeOperation::Insert {
        module,
        parameters: operands,
    })
}

fn parse_unsigned_long(value: &str) -> Option<u64> {
    let value = value.strip_prefix('+').unwrap_or(value);
    if value.is_empty() || value.starts_with('-') {
        return None;
    }
    let (digits, radix) = if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        (hex, 16)
    } else if value.len() > 1 && value.starts_with('0') {
        (&value[1..], 8)
    } else {
        (value, 10)
    };
    if digits.is_empty() {
        return None;
    }
    u64::from_str_radix(digits, radix).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_every_declared_option_and_deprecated_alias() {
        let parsed = parse_modprobe(&[
            "-a",
            "-r",
            "--remove-dependencies",
            "--remove-holders",
            "-w0x10",
            "--resolve-alias",
            "-R",
            "--first-time",
            "-i",
            "--ignore-remove",
            "-b",
            "-f",
            "--force-modversion",
            "--force-vermagic",
            "-D",
            "--showconfig",
            "-c",
            "--show-modversions",
            "--dump-modversions",
            "--show-exports",
            "-n",
            "--show",
            "-C/etc/modprobe.d",
            "-d",
            "/root",
            "-S6.12",
            "-sqv",
            "demo",
        ])
        .unwrap();
        assert_eq!(parsed.options.len(), 28);
        assert!(matches!(parsed.action, ModprobeAction::ShowConfig { .. }));
    }

    #[test]
    fn distinguishes_query_and_mutation_precedence() {
        assert!(matches!(
            parse_modprobe(&["demo", "answer=1"]).unwrap().action,
            ModprobeAction::Mutate {
                operation: ModprobeOperation::Insert { .. }
            }
        ));
        assert!(matches!(
            parse_modprobe(&["-n", "demo"]).unwrap().action,
            ModprobeAction::Query { .. }
        ));
        assert!(matches!(
            parse_modprobe(&["-R", "pci:demo"]).unwrap().action,
            ModprobeAction::Query { .. }
        ));
        assert!(matches!(
            parse_modprobe(&["-r", "-R", "demo"]).unwrap().action,
            ModprobeAction::Mutate {
                operation: ModprobeOperation::Remove { .. }
            }
        ));
    }

    #[test]
    fn models_information_show_and_operand_contracts() {
        assert_eq!(
            parse_modprobe(&["--help", "ignored"]).unwrap().action,
            ModprobeAction::Help
        );
        assert_eq!(
            parse_modprobe(&["-V"]).unwrap().action,
            ModprobeAction::Version
        );
        assert!(matches!(
            parse_modprobe(&["--show-modversions", "demo.ko", "ignored"])
                .unwrap()
                .action,
            ModprobeAction::ShowModuleVersions {
                ref ignored_operands,
                ..
            } if ignored_operands == &["ignored"]
        ));
        assert!(parse_modprobe(&["--show-exports"]).is_err());
        assert!(parse_modprobe(&[]).is_err());
    }

    #[test]
    fn rejects_undeclared_or_malformed_forms() {
        assert!(parse_modprobe(&["--show-conf", "demo"]).is_err());
        assert!(parse_modprobe(&["--wait=09", "demo"]).is_err());
        assert!(parse_modprobe(&["--wait=-1", "demo"]).is_err());
        assert!(parse_modprobe(&["-w", "demo"]).is_err());
        assert!(parse_modprobe(&["--config=", "demo"]).is_err());
    }
}
