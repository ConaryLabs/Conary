// conary-core/src/packages/deb/dpkg_lifecycle/common.rs

pub(super) const MAINTSCRIPT_HELPER: &str = "dpkg-maintscript-helper";
pub(super) const DPKG_QUERY: &str = "dpkg-query";
pub(super) const DPKG: &str = "dpkg";
pub(super) const UPDATE_ALTERNATIVES: &str = "update-alternatives";

/// Exact failures from the pinned lifecycle tool grammars.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DpkgLifecycleGrammarError {
    #[error("unknown dpkg lifecycle tool {0:?}")]
    UnknownTool(Vec<u8>),
    #[error("{tool}: no action was specified")]
    MissingAction { tool: &'static str },
    #[error("{tool}: unknown action {action:?}")]
    UnknownAction { tool: &'static str, action: Vec<u8> },
    #[error("{tool}: unknown option {option:?}")]
    UnknownOption { tool: &'static str, option: Vec<u8> },
    #[error("{tool}: conflicting actions {first:?} and {second:?}")]
    ConflictingActions {
        tool: &'static str,
        first: Vec<u8>,
        second: Vec<u8>,
    },
    #[error("{tool} {action}: missing required {operand}")]
    MissingOperand {
        tool: &'static str,
        action: &'static str,
        operand: &'static str,
    },
    #[error("{tool} {action}: unexpected operand {operand:?}")]
    UnexpectedOperand {
        tool: &'static str,
        action: &'static str,
        operand: Vec<u8>,
    },
    #[error("{tool} {action}: invalid {operand} {value:?}: {reason}")]
    InvalidOperand {
        tool: &'static str,
        action: &'static str,
        operand: &'static str,
        value: Vec<u8>,
        reason: &'static str,
    },
    #[error("{tool}: option {option} is not valid with action {action}")]
    InvalidOptionForAction {
        tool: &'static str,
        option: &'static str,
        action: &'static str,
    },
    #[error("{tool}: {action} requires a '--' maintainer-argument delimiter")]
    MissingDelimiter {
        tool: &'static str,
        action: &'static str,
    },
    #[error("{tool}: multiple '--' delimiters are not permitted")]
    MultipleDelimiters { tool: &'static str },
    #[error("{tool}: private internal command {command:?} is not a public lifecycle contract")]
    PrivateInternalCommand {
        tool: &'static str,
        command: Vec<u8>,
    },
    #[error("{tool}: unsupported exit status {status}")]
    UnsupportedExitStatus { tool: &'static str, status: u8 },
}

pub(super) fn private_internal_command(command: &[u8]) -> DpkgLifecycleGrammarError {
    DpkgLifecycleGrammarError::PrivateInternalCommand {
        tool: MAINTSCRIPT_HELPER,
        command: command.to_vec(),
    }
}

pub(super) fn required<'a>(
    tool: &'static str,
    action: &'static str,
    operand: &'static str,
    value: Option<&'a Vec<u8>>,
) -> Result<&'a [u8], DpkgLifecycleGrammarError> {
    value
        .map(Vec::as_slice)
        .ok_or(DpkgLifecycleGrammarError::MissingOperand {
            tool,
            action,
            operand,
        })
}

pub(super) fn require_no_nul(
    tool: &'static str,
    action: &'static str,
    operand: &'static str,
    value: &[u8],
) -> Result<(), DpkgLifecycleGrammarError> {
    if value.contains(&0) {
        return Err(DpkgLifecycleGrammarError::InvalidOperand {
            tool,
            action,
            operand,
            value: value.to_vec(),
            reason: "must not contain NUL",
        });
    }
    Ok(())
}

pub(super) fn require_nonempty(
    tool: &'static str,
    action: &'static str,
    operand: &'static str,
    value: &[u8],
) -> Result<(), DpkgLifecycleGrammarError> {
    require_no_nul(tool, action, operand, value)?;
    if value.is_empty() {
        return Err(DpkgLifecycleGrammarError::InvalidOperand {
            tool,
            action,
            operand,
            value: value.to_vec(),
            reason: "must not be empty",
        });
    }
    Ok(())
}

pub(super) fn require_absolute(
    tool: &'static str,
    action: &'static str,
    operand: &'static str,
    value: &[u8],
) -> Result<(), DpkgLifecycleGrammarError> {
    require_nonempty(tool, action, operand, value)?;
    if value.first() != Some(&b'/') {
        return Err(DpkgLifecycleGrammarError::InvalidOperand {
            tool,
            action,
            operand,
            value: value.to_vec(),
            reason: "must be an absolute path",
        });
    }
    Ok(())
}

pub(super) fn require_arity(
    tool: &'static str,
    action: &'static str,
    operands: &[Vec<u8>],
    minimum: usize,
    maximum: Option<usize>,
    first_missing: &'static str,
) -> Result<(), DpkgLifecycleGrammarError> {
    if operands.len() < minimum {
        return Err(DpkgLifecycleGrammarError::MissingOperand {
            tool,
            action,
            operand: first_missing,
        });
    }
    if maximum.is_some_and(|maximum| operands.len() > maximum) {
        return Err(DpkgLifecycleGrammarError::UnexpectedOperand {
            tool,
            action,
            operand: operands[maximum.expect("checked maximum")].clone(),
        });
    }
    for operand in operands {
        require_no_nul(tool, action, "argument", operand)?;
    }
    Ok(())
}

pub(super) fn long_value<'a>(argument: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    argument
        .strip_prefix(name)
        .and_then(|suffix| suffix.strip_prefix(b"="))
}

pub(super) fn option_value(
    argv: &[Vec<u8>],
    index: &mut usize,
    tool: &'static str,
    option: &'static str,
    attached: Option<&[u8]>,
) -> Result<Vec<u8>, DpkgLifecycleGrammarError> {
    let value = if let Some(attached) = attached {
        attached.to_vec()
    } else {
        *index += 1;
        argv.get(*index)
            .cloned()
            .ok_or(DpkgLifecycleGrammarError::MissingOperand {
                tool,
                action: option,
                operand: "option value",
            })?
    };
    require_no_nul(tool, option, "option value", &value)?;
    Ok(value)
}

pub(super) fn push_long_value(argv: &mut Vec<Vec<u8>>, option: &[u8], value: &[u8]) {
    let mut argument = Vec::with_capacity(option.len() + value.len() + 1);
    argument.extend_from_slice(option);
    argument.push(b'=');
    argument.extend_from_slice(value);
    argv.push(argument);
}

pub(super) fn render_with_operands(
    mut prefix: Vec<Vec<u8>>,
    action: &[u8],
    operands: &[Vec<u8>],
) -> Vec<Vec<u8>> {
    prefix.push(action.to_vec());
    if !operands.is_empty() {
        prefix.push(b"--".to_vec());
        prefix.extend(operands.iter().cloned());
    }
    prefix
}
