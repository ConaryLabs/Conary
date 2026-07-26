// conary-core/src/packages/deb/lifecycle_helpers/invoke_rc_d.rs

use super::HelperGrammarError;
use super::common::{InitScriptAction, validate_argument, validate_word};

const HELPER: &str = "invoke-rc.d";

/// Every option in the pinned invoke-rc.d help/manpage contract.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InvokeRcDOptions {
    pub quiet: bool,
    pub force: bool,
    pub try_anyway: bool,
    pub disclose_deny: bool,
    pub query: bool,
    pub no_fallback: bool,
    pub skip_systemd_native: bool,
}

impl InvokeRcDOptions {
    pub const fn effectively_discloses_denial(self) -> bool {
        self.disclose_deny || self.query
    }

    pub const fn effectively_disables_fallback(self) -> bool {
        self.no_fallback || self.query
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvokeRcDInvocation {
    Help,
    Run {
        options: InvokeRcDOptions,
        service: String,
        action: InitScriptAction,
        extra_parameters: Vec<String>,
    },
}

impl InvokeRcDInvocation {
    pub fn parse(argv: &[String]) -> Result<Self, HelperGrammarError> {
        let mut options = InvokeRcDOptions::default();
        let mut service = None;
        let mut action = None;
        let mut index = 0;

        while index < argv.len() && action.is_none() {
            let argument = &argv[index];
            match argument.as_str() {
                "--help" => return Ok(Self::Help),
                "--quiet" => options.quiet = true,
                "--force" => {
                    options.force = true;
                    options.try_anyway = true;
                }
                "--try-anyway" => options.try_anyway = true,
                "--disclose-deny" => options.disclose_deny = true,
                "--query" => options.query = true,
                "--no-fallback" => options.no_fallback = true,
                "--skip-systemd-native" => options.skip_systemd_native = true,
                option if option.starts_with("--") => {
                    return Err(HelperGrammarError::UnknownOption {
                        helper: HELPER,
                        option: option.to_string(),
                    });
                }
                value if service.is_none() => {
                    validate_word(HELPER, "initscript ID", value)?;
                    service = Some(value.to_string());
                }
                value => {
                    action = Some(InitScriptAction::parse(HELPER, value, true)?);
                }
            }
            index += 1;
        }

        let service = service.ok_or(HelperGrammarError::MissingOperand {
            helper: HELPER,
            operand: "initscript ID",
        })?;
        let action = action.ok_or(HelperGrammarError::MissingOperand {
            helper: HELPER,
            operand: "action",
        })?;
        let extra_parameters = argv[index..].to_vec();
        for parameter in &extra_parameters {
            validate_argument(HELPER, "init script parameter", parameter)?;
        }

        Ok(Self::Run {
            options,
            service,
            action,
            extra_parameters,
        })
    }

    pub fn argv(&self) -> Vec<String> {
        let Self::Run {
            options,
            service,
            action,
            extra_parameters,
        } = self
        else {
            return vec!["--help".to_string()];
        };

        let mut argv = Vec::new();
        if options.quiet {
            argv.push("--quiet".to_string());
        }
        if options.force {
            argv.push("--force".to_string());
        } else if options.try_anyway {
            argv.push("--try-anyway".to_string());
        }
        if options.disclose_deny {
            argv.push("--disclose-deny".to_string());
        }
        if options.query {
            argv.push("--query".to_string());
        }
        if options.no_fallback {
            argv.push("--no-fallback".to_string());
        }
        if options.skip_systemd_native {
            argv.push("--skip-systemd-native".to_string());
        }
        argv.push(service.clone());
        argv.push(action.as_str().to_string());
        argv.extend(extra_parameters.iter().cloned());
        argv
    }
}

/// Status code space documented by invoke-rc.d.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvokeRcDStatus {
    Success,
    InitScriptStatus(u8),
    UnknownScript,
    Denied,
    SubsystemError,
    SyntaxError,
    AllowedQuery,
    UncertainQuery,
    FallbackRequested,
}

impl InvokeRcDStatus {
    pub fn from_exit_code(status: i32) -> Result<Self, HelperGrammarError> {
        match status {
            0 => Ok(Self::Success),
            1..=99 => Ok(Self::InitScriptStatus(status as u8)),
            100 => Ok(Self::UnknownScript),
            101 => Ok(Self::Denied),
            102 => Ok(Self::SubsystemError),
            103 => Ok(Self::SyntaxError),
            104 => Ok(Self::AllowedQuery),
            105 => Ok(Self::UncertainQuery),
            106 => Ok(Self::FallbackRequested),
            status => Err(HelperGrammarError::UnsupportedStatus {
                helper: HELPER,
                status,
            }),
        }
    }

    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::InitScriptStatus(status) => status as i32,
            Self::UnknownScript => 100,
            Self::Denied => 101,
            Self::SubsystemError => 102,
            Self::SyntaxError => 103,
            Self::AllowedQuery => 104,
            Self::UncertainQuery => 105,
            Self::FallbackRequested => 106,
        }
    }
}

/// Raw policy-rc.d result branches normalized exactly as invoke-rc.d does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvokeRcDPolicyStatus {
    Allow,
    Deny,
    Undefined,
    Fallback(Vec<InitScriptAction>),
    InvokeStatus(InvokeRcDStatus),
    Unsupported(i32),
}

impl InvokeRcDPolicyStatus {
    pub fn from_helper_result(status: i32, stdout: &str) -> Result<Self, HelperGrammarError> {
        match status {
            0 | 104 => Ok(Self::Allow),
            1 | 105 => Ok(Self::Undefined),
            101 => Ok(Self::Deny),
            106 => {
                let actions = stdout
                    .split_ascii_whitespace()
                    .map(|action| InitScriptAction::parse(HELPER, action, true))
                    .collect::<Result<Vec<_>, _>>()?;
                if actions.is_empty() {
                    return Err(HelperGrammarError::InvalidOperand {
                        helper: HELPER,
                        operand: "policy fallback action",
                        value: stdout.to_string(),
                        reason: "status 106 requires at least one fallback action",
                    });
                }
                Ok(Self::Fallback(actions))
            }
            100 | 102 | 103 => Ok(Self::InvokeStatus(InvokeRcDStatus::from_exit_code(status)?)),
            status => Ok(Self::Unsupported(status)),
        }
    }
}
