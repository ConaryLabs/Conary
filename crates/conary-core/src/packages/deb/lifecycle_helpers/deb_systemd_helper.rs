// conary-core/src/packages/deb/lifecycle_helpers/deb_systemd_helper.rs

use super::common::{UnitInstance, required, validate_argument};
use super::{HelperGrammarError, common::validate_word};

const HELPER: &str = "deb-systemd-helper";

/// Options parsed by init-system-helpers' Getopt::Long contract.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DebSystemdHelperOptions {
    pub quiet: bool,
    pub instance: UnitInstance,
}

/// Every documented `deb-systemd-helper` action in 1.69~deb13u1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebSystemdHelperAction {
    Enable,
    Disable,
    Purge,
    Mask,
    Unmask,
    IsEnabled,
    WasEnabled,
    DebianInstalled,
    UpdateState,
    Reenable,
}

impl DebSystemdHelperAction {
    pub const ALL: [Self; 10] = [
        Self::Enable,
        Self::Disable,
        Self::Purge,
        Self::Mask,
        Self::Unmask,
        Self::IsEnabled,
        Self::WasEnabled,
        Self::DebianInstalled,
        Self::UpdateState,
        Self::Reenable,
    ];

    fn parse(value: &str) -> Result<Self, HelperGrammarError> {
        match value {
            "enable" => Ok(Self::Enable),
            "disable" => Ok(Self::Disable),
            "purge" => Ok(Self::Purge),
            "mask" => Ok(Self::Mask),
            "unmask" => Ok(Self::Unmask),
            "is-enabled" => Ok(Self::IsEnabled),
            "was-enabled" => Ok(Self::WasEnabled),
            "debian-installed" => Ok(Self::DebianInstalled),
            "update-state" => Ok(Self::UpdateState),
            "reenable" => Ok(Self::Reenable),
            unknown => Err(HelperGrammarError::UnknownAction {
                helper: HELPER,
                action: unknown.to_string(),
            }),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Purge => "purge",
            Self::Mask => "mask",
            Self::Unmask => "unmask",
            Self::IsEnabled => "is-enabled",
            Self::WasEnabled => "was-enabled",
            Self::DebianInstalled => "debian-installed",
            Self::UpdateState => "update-state",
            Self::Reenable => "reenable",
        }
    }

    pub const fn is_query(self) -> bool {
        matches!(
            self,
            Self::IsEnabled | Self::WasEnabled | Self::DebianInstalled
        )
    }
}

/// The common 0/1 result contract of the three query actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebSystemdHelperQueryStatus {
    Satisfied,
    Unsatisfied,
}

impl DebSystemdHelperQueryStatus {
    pub fn from_exit_code(status: i32) -> Result<Self, HelperGrammarError> {
        match status {
            0 => Ok(Self::Satisfied),
            1 => Ok(Self::Unsatisfied),
            status => Err(HelperGrammarError::UnsupportedStatus {
                helper: HELPER,
                status,
            }),
        }
    }

    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Satisfied => 0,
            Self::Unsatisfied => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebSystemdHelperInvocation {
    pub options: DebSystemdHelperOptions,
    pub action: DebSystemdHelperAction,
    pub units: Vec<String>,
}

impl DebSystemdHelperInvocation {
    pub fn parse(argv: &[String]) -> Result<Self, HelperGrammarError> {
        let mut options = DebSystemdHelperOptions::default();
        let mut positional = Vec::new();
        let mut options_ended = false;

        for argument in argv {
            if options_ended {
                positional.push(argument.clone());
                continue;
            }
            match argument.as_str() {
                "--" => options_ended = true,
                "--quiet" => options.quiet = true,
                "--user" => options.instance = UnitInstance::User,
                "--system" => options.instance = UnitInstance::System,
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
            DebSystemdHelperAction::parse(required(HELPER, "action", positional.first())?)?;
        let units = positional.into_iter().skip(1).collect::<Vec<_>>();
        if units.is_empty() {
            return Err(HelperGrammarError::MissingOperand {
                helper: HELPER,
                operand: "unit file",
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
        if self.options.quiet {
            argv.push("--quiet".to_string());
        }
        if self.options.instance == UnitInstance::User {
            argv.push("--user".to_string());
        }
        if self.units.iter().any(|unit| unit.starts_with('-')) {
            argv.push("--".to_string());
        }
        argv.push(self.action.as_str().to_string());
        argv.extend(self.units.iter().cloned());
        argv
    }

    pub fn validate(&self) -> Result<(), HelperGrammarError> {
        validate_word(HELPER, "action", self.action.as_str())?;
        Self::parse(&self.argv()).map(|_| ())
    }
}
