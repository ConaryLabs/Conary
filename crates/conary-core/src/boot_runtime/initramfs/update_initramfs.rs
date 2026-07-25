// crates/conary-core/src/boot_runtime/initramfs/update_initramfs.rs

use crate::boot_runtime::argv::{OptionSpec, require_nonempty, scan_gnu};
use crate::boot_runtime::{BootRuntimeParseError, InvocationDisposition, UpstreamContract};

pub const UPDATE_INITRAMFS_CONTRACT: UpstreamContract = UpstreamContract {
    project: "initramfs-tools",
    version: "0.151",
    revision: "b6795cfe2545b18b30526deb7b609f74339bd6e0",
    source_url: "https://salsa.debian.org/kernel-team/initramfs-tools",
};

const OPTIONS: &[OptionSpec] = &[
    OptionSpec::short_value("kernel", 'k'),
    OptionSpec::short_flag("create", 'c'),
    OptionSpec::short_flag("update", 'u'),
    OptionSpec::short_flag("delete", 'd'),
    OptionSpec::short_flag("verbose", 'v'),
    OptionSpec::short_flag("takeover", 't'),
    OptionSpec::short_value("bootdir", 'b'),
    OptionSpec::short_value("timestamp", 's'),
    OptionSpec::flag("help", Some('h'), "help"),
    OptionSpec::short_flag("help", '?'),
    OptionSpec::flag("version", None, "version"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelSelection {
    All,
    Version(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateInitramfsOption {
    Kernel(KernelSelection),
    Verbose,
    TakeoverCompatibility,
    BootDirectory(String),
    Timestamp(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateInitramfsAction {
    Help,
    Version,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInitramfsInvocation {
    pub contract: UpstreamContract,
    pub action: UpdateInitramfsAction,
    pub options: Vec<UpdateInitramfsOption>,
}

impl UpdateInitramfsInvocation {
    pub fn disposition(&self) -> InvocationDisposition {
        match self.action {
            UpdateInitramfsAction::Help | UpdateInitramfsAction::Version => {
                InvocationDisposition::Information
            }
            UpdateInitramfsAction::Create
            | UpdateInitramfsAction::Update
            | UpdateInitramfsAction::Delete => InvocationDisposition::Mutation,
        }
    }
}

pub(super) fn parse_update_initramfs(
    argv: &[&str],
) -> Result<UpdateInitramfsInvocation, BootRuntimeParseError> {
    const COMMAND: &str = "update-initramfs";
    let scanned = scan_gnu(COMMAND, argv, OPTIONS)?;
    if let Some(operand) = scanned.operands.first() {
        return Err(BootRuntimeParseError::UnexpectedOperand {
            command: COMMAND,
            action: "update-initramfs",
            operand: operand.clone(),
        });
    }

    let mut options = Vec::new();
    let mut action = None;
    let mut information = None;
    for option in scanned.options {
        let value = || option.values[0].clone();
        match option.id {
            "kernel" => {
                let value = require_nonempty(
                    COMMAND,
                    &option.spelling,
                    value(),
                    "all or a kernel version",
                )?;
                options.push(UpdateInitramfsOption::Kernel(if value == "all" {
                    KernelSelection::All
                } else {
                    KernelSelection::Version(value)
                }));
            }
            "create" => action = Some(UpdateInitramfsAction::Create),
            "update" => action = Some(UpdateInitramfsAction::Update),
            "delete" => action = Some(UpdateInitramfsAction::Delete),
            "verbose" => options.push(UpdateInitramfsOption::Verbose),
            "takeover" => options.push(UpdateInitramfsOption::TakeoverCompatibility),
            "bootdir" => options.push(UpdateInitramfsOption::BootDirectory(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a boot directory",
            )?)),
            "timestamp" => {
                let raw = value();
                let timestamp =
                    raw.parse::<u64>()
                        .map_err(|_| BootRuntimeParseError::InvalidOptionValue {
                            command: COMMAND,
                            option: option.spelling,
                            value: raw,
                            expected: "an unsigned Unix timestamp",
                        })?;
                options.push(UpdateInitramfsOption::Timestamp(timestamp));
            }
            "help" => information = Some(UpdateInitramfsAction::Help),
            "version" => information = Some(UpdateInitramfsAction::Version),
            _ => unreachable!("closed update-initramfs option table"),
        }
    }

    let action = information
        .or(action)
        .ok_or(BootRuntimeParseError::MissingAction {
            command: COMMAND,
            expected: "-c/--create, -u/--update, or -d/--delete",
        })?;
    Ok(UpdateInitramfsInvocation {
        contract: UPDATE_INITRAMFS_CONTRACT,
        action,
        options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_every_option_and_last_mutation_mode_wins() {
        let invocation =
            parse_update_initramfs(&["-cuvtd", "-k", "all", "-b/boot", "-s", "1234"]).unwrap();
        assert_eq!(invocation.action, UpdateInitramfsAction::Delete);
        assert_eq!(invocation.options.len(), 5);
    }

    #[test]
    fn covers_help_aliases_and_version() {
        for help in [["-h"].as_slice(), ["-?"].as_slice(), ["--help"].as_slice()] {
            assert_eq!(
                parse_update_initramfs(help).unwrap().action,
                UpdateInitramfsAction::Help
            );
        }
        assert_eq!(
            parse_update_initramfs(&["--version"]).unwrap().action,
            UpdateInitramfsAction::Version
        );
    }

    #[test]
    fn accepts_terminator_but_rejects_every_positional() {
        assert!(parse_update_initramfs(&["-u", "--"]).is_ok());
        assert!(parse_update_initramfs(&["-u", "--", "extra"]).is_err());
    }

    #[test]
    fn rejects_missing_mode_unknowns_and_malformed_values() {
        assert!(parse_update_initramfs(&[]).is_err());
        assert!(parse_update_initramfs(&["--future"]).is_err());
        assert!(parse_update_initramfs(&["-u", "-s", "-1"]).is_err());
        assert!(parse_update_initramfs(&["-u", "-k", ""]).is_err());
    }
}
