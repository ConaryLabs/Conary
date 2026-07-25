// conary-core/src/packages/deb/lifecycle_helpers/deb_systemd_invoke.rs

use super::HelperGrammarError;
use super::common::{UnitInstance, required, validate_argument};

const HELPER: &str = "deb-systemd-invoke";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DebSystemdInvokeOptions {
    pub instance: UnitInstance,
    pub no_dbus: bool,
}

/// The two exact action shapes documented by `deb-systemd-invoke`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebSystemdInvokeAction {
    Start,
    Stop,
    Restart,
    DaemonReload,
    DaemonReexec,
}

impl DebSystemdInvokeAction {
    pub const ALL: [Self; 5] = [
        Self::Start,
        Self::Stop,
        Self::Restart,
        Self::DaemonReload,
        Self::DaemonReexec,
    ];

    fn parse(value: &str) -> Result<Self, HelperGrammarError> {
        match value {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "restart" => Ok(Self::Restart),
            "daemon-reload" => Ok(Self::DaemonReload),
            "daemon-reexec" => Ok(Self::DaemonReexec),
            unknown => Err(HelperGrammarError::UnknownAction {
                helper: HELPER,
                action: unknown.to_string(),
            }),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::DaemonReload => "daemon-reload",
            Self::DaemonReexec => "daemon-reexec",
        }
    }

    pub const fn takes_units(self) -> bool {
        matches!(self, Self::Start | Self::Stop | Self::Restart)
    }
}

/// Exact policy-rc.d branches implemented by deb-systemd-invoke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebSystemdInvokePolicyStatus {
    Run,
    Deny,
    IgnoredUnsupported(i32),
}

impl DebSystemdInvokePolicyStatus {
    pub const fn from_exit_code(status: i32) -> Self {
        match status {
            0 | 104 => Self::Run,
            101 => Self::Deny,
            status => Self::IgnoredUnsupported(status),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebSystemdInvokeInvocation {
    pub options: DebSystemdInvokeOptions,
    pub action: DebSystemdInvokeAction,
    pub units: Vec<String>,
}

impl DebSystemdInvokeInvocation {
    pub fn parse(argv: &[String]) -> Result<Self, HelperGrammarError> {
        let mut options = DebSystemdInvokeOptions::default();
        let mut positional = Vec::new();
        let mut options_ended = false;

        for argument in argv {
            if options_ended {
                positional.push(argument.clone());
                continue;
            }
            match argument.as_str() {
                "--" => options_ended = true,
                "--user" => options.instance = UnitInstance::User,
                "--system" => options.instance = UnitInstance::System,
                "--no-dbus" => options.no_dbus = true,
                option if option.starts_with('-') => {
                    return Err(HelperGrammarError::UnknownOption {
                        helper: HELPER,
                        option: option.to_string(),
                    });
                }
                _ => positional.push(argument.clone()),
            }
        }

        let action =
            DebSystemdInvokeAction::parse(required(HELPER, "action", positional.first())?)?;
        let units = positional.into_iter().skip(1).collect::<Vec<_>>();
        if action.takes_units() && units.is_empty() {
            return Err(HelperGrammarError::MissingOperand {
                helper: HELPER,
                operand: "unit file",
            });
        }
        if !action.takes_units() && !units.is_empty() {
            return Err(HelperGrammarError::UnexpectedOperand {
                helper: HELPER,
                operand: units[0].clone(),
            });
        }
        if action.takes_units() && options.no_dbus {
            return Err(HelperGrammarError::InvalidOptionForAction {
                helper: HELPER,
                option: "--no-dbus",
                action: action.as_str(),
            });
        }
        for unit in &units {
            validate_argument(HELPER, "unit file", unit)?;
            if unit.is_empty() {
                return Err(HelperGrammarError::InvalidOperand {
                    helper: HELPER,
                    operand: "unit file",
                    value: unit.clone(),
                    reason: "must not be empty",
                });
            }
        }

        Ok(Self {
            options,
            action,
            units,
        })
    }

    pub fn argv(&self) -> Vec<String> {
        let mut argv = Vec::new();
        if self.options.instance == UnitInstance::User {
            argv.push("--user".to_string());
        }
        if self.options.no_dbus {
            argv.push("--no-dbus".to_string());
        }
        if self.units.iter().any(|unit| unit.starts_with('-')) {
            argv.push("--".to_string());
        }
        argv.push(self.action.as_str().to_string());
        argv.extend(self.units.iter().cloned());
        argv
    }
}
