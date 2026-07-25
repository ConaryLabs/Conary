// conary-core/src/packages/deb/lifecycle_helpers/common.rs

use super::HelperGrammarError;

/// systemd manager instance selected by the Debian helper options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UnitInstance {
    #[default]
    System,
    User,
}

/// Standard Debian init-script actions plus the explicitly permitted custom
/// action branch used by `invoke-rc.d` and `service`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitScriptAction {
    Start,
    Stop,
    ForceStop,
    Restart,
    TryRestart,
    Reload,
    ForceReload,
    Status,
    Custom(String),
}

impl InitScriptAction {
    pub const STANDARD: [Self; 8] = [
        Self::Start,
        Self::Stop,
        Self::ForceStop,
        Self::Restart,
        Self::TryRestart,
        Self::Reload,
        Self::ForceReload,
        Self::Status,
    ];

    pub fn parse(
        helper: &'static str,
        value: &str,
        permit_custom: bool,
    ) -> Result<Self, HelperGrammarError> {
        let action = match value {
            "start" => Self::Start,
            "stop" => Self::Stop,
            "force-stop" => Self::ForceStop,
            "restart" => Self::Restart,
            "try-restart" => Self::TryRestart,
            "reload" => Self::Reload,
            "force-reload" => Self::ForceReload,
            "status" => Self::Status,
            custom if permit_custom => {
                validate_word(helper, "action", custom)?;
                if custom.starts_with("--") {
                    return Err(HelperGrammarError::UnknownOption {
                        helper,
                        option: custom.to_string(),
                    });
                }
                Self::Custom(custom.to_string())
            }
            unknown => {
                return Err(HelperGrammarError::UnknownAction {
                    helper,
                    action: unknown.to_string(),
                });
            }
        };
        Ok(action)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::ForceStop => "force-stop",
            Self::Restart => "restart",
            Self::TryRestart => "try-restart",
            Self::Reload => "reload",
            Self::ForceReload => "force-reload",
            Self::Status => "status",
            Self::Custom(action) => action,
        }
    }
}

pub(super) fn required<'a>(
    helper: &'static str,
    operand: &'static str,
    value: Option<&'a String>,
) -> Result<&'a str, HelperGrammarError> {
    value
        .map(String::as_str)
        .ok_or(HelperGrammarError::MissingOperand { helper, operand })
}

pub(super) fn validate_word(
    helper: &'static str,
    operand: &'static str,
    value: &str,
) -> Result<(), HelperGrammarError> {
    if value.is_empty() {
        return Err(HelperGrammarError::InvalidOperand {
            helper,
            operand,
            value: value.to_string(),
            reason: "must not be empty",
        });
    }
    if value.contains('\0') {
        return Err(HelperGrammarError::InvalidOperand {
            helper,
            operand,
            value: value.to_string(),
            reason: "must not contain NUL",
        });
    }
    if value
        .bytes()
        .any(|byte| matches!(byte, b' ' | b'\t' | b'\n'))
    {
        return Err(HelperGrammarError::InvalidOperand {
            helper,
            operand,
            value: value.to_string(),
            reason: "must be one shell word without IFS whitespace",
        });
    }
    Ok(())
}

pub(super) fn validate_argument(
    helper: &'static str,
    operand: &'static str,
    value: &str,
) -> Result<(), HelperGrammarError> {
    if value.contains('\0') {
        return Err(HelperGrammarError::InvalidOperand {
            helper,
            operand,
            value: value.to_string(),
            reason: "must not contain NUL",
        });
    }
    Ok(())
}
