// conary-core/src/packages/deb/lifecycle_helpers/service.rs

use super::HelperGrammarError;
use super::common::{InitScriptAction, validate_argument};

const HELPER: &str = "service";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceAction {
    Init(InitScriptAction),
    FullRestart,
}

impl ServiceAction {
    fn parse(value: &str) -> Result<Self, HelperGrammarError> {
        if value == "--full-restart" {
            Ok(Self::FullRestart)
        } else {
            InitScriptAction::parse(HELPER, value, true).map(Self::Init)
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Init(action) => action.as_str(),
            Self::FullRestart => "--full-restart",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceInvocation {
    Help,
    Version,
    StatusAll,
    Run {
        service: String,
        action: ServiceAction,
        extra_parameters: Vec<String>,
    },
}

impl ServiceInvocation {
    pub fn parse(argv: &[String]) -> Result<Self, HelperGrammarError> {
        if argv.is_empty() {
            return Err(HelperGrammarError::MissingOperand {
                helper: HELPER,
                operand: "service name or informational option",
            });
        }

        // The shell implementation handles these in every loop iteration and
        // exits immediately, before any service action can run.
        if argv
            .iter()
            .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
        {
            return Ok(Self::Help);
        }
        if argv
            .iter()
            .any(|argument| matches!(argument.as_str(), "--version" | "-V"))
        {
            return Ok(Self::Version);
        }
        if argv == ["--status-all"] {
            return Ok(Self::StatusAll);
        }
        if argv
            .first()
            .is_some_and(|argument| argument.starts_with('-'))
        {
            return Err(HelperGrammarError::UnknownOption {
                helper: HELPER,
                option: argv[0].clone(),
            });
        }

        let service = argv[0].clone();
        validate_argument(HELPER, "service name", &service)?;
        if service.is_empty() {
            return Err(HelperGrammarError::InvalidOperand {
                helper: HELPER,
                operand: "service name",
                value: service,
                reason: "must not be empty",
            });
        }

        let action_text = argv.get(1).ok_or(HelperGrammarError::MissingOperand {
            helper: HELPER,
            operand: "command",
        })?;
        let action = ServiceAction::parse(action_text)?;
        let extra_parameters = argv[2..].to_vec();
        if action == ServiceAction::FullRestart && !extra_parameters.is_empty() {
            return Err(HelperGrammarError::UnexpectedOperand {
                helper: HELPER,
                operand: extra_parameters[0].clone(),
            });
        }
        for parameter in &extra_parameters {
            validate_argument(HELPER, "init script parameter", parameter)?;
        }

        Ok(Self::Run {
            service,
            action,
            extra_parameters,
        })
    }

    pub fn argv(&self) -> Vec<String> {
        match self {
            Self::Help => vec!["--help".to_string()],
            Self::Version => vec!["--version".to_string()],
            Self::StatusAll => vec!["--status-all".to_string()],
            Self::Run {
                service,
                action,
                extra_parameters,
            } => {
                let mut argv = vec![service.clone(), action.as_str().to_string()];
                argv.extend(extra_parameters.iter().cloned());
                argv
            }
        }
    }
}

/// The three markers emitted by `service --status-all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatus {
    Running,
    Stopped,
    Unknown,
}

impl ServiceStatus {
    pub const ALL: [Self; 3] = [Self::Running, Self::Stopped, Self::Unknown];

    pub const fn marker(self) -> &'static str {
        match self {
            Self::Running => "+",
            Self::Stopped => "-",
            Self::Unknown => "?",
        }
    }

    pub fn from_marker(marker: &str) -> Result<Self, HelperGrammarError> {
        match marker {
            "+" => Ok(Self::Running),
            "-" => Ok(Self::Stopped),
            "?" => Ok(Self::Unknown),
            marker => Err(HelperGrammarError::InvalidOperand {
                helper: HELPER,
                operand: "status marker",
                value: marker.to_string(),
                reason: "expected +, -, or ?",
            }),
        }
    }
}
