// conary-core/src/packages/deb/lifecycle_helpers/update_rc_d.rs

use super::HelperGrammarError;
use super::common::validate_word;

const HELPER: &str = "update-rc.d";

/// Runlevels accepted by the current and retained start/stop source branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysvRunlevel {
    Boot,
    Zero,
    One,
    Two,
    Three,
    Four,
    Five,
    Six,
}

impl SysvRunlevel {
    pub const ALL: [Self; 8] = [
        Self::Boot,
        Self::Zero,
        Self::One,
        Self::Two,
        Self::Three,
        Self::Four,
        Self::Five,
        Self::Six,
    ];

    pub const TOGGLE: [Self; 5] = [Self::Boot, Self::Two, Self::Three, Self::Four, Self::Five];

    fn parse(value: &str) -> Result<Self, HelperGrammarError> {
        match value {
            "S" => Ok(Self::Boot),
            "0" => Ok(Self::Zero),
            "1" => Ok(Self::One),
            "2" => Ok(Self::Two),
            "3" => Ok(Self::Three),
            "4" => Ok(Self::Four),
            "5" => Ok(Self::Five),
            "6" => Ok(Self::Six),
            value => Err(HelperGrammarError::InvalidRunlevel {
                helper: HELPER,
                value: value.to_string(),
            }),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Boot => "S",
            Self::Zero => "0",
            Self::One => "1",
            Self::Two => "2",
            Self::Three => "3",
            Self::Four => "4",
            Self::Five => "5",
            Self::Six => "6",
        }
    }

    const fn is_toggle(self) -> bool {
        matches!(
            self,
            Self::Boot | Self::Two | Self::Three | Self::Four | Self::Five
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacySequenceKind {
    Start,
    Stop,
}

impl LegacySequenceKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "start" => Some(Self::Start),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
        }
    }
}

/// One strict clause from the still-present legacy start/stop source branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacySequenceClause {
    pub kind: LegacySequenceKind,
    pub sequence: u8,
    pub runlevels: Vec<SysvRunlevel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateRcDOperation {
    Remove,
    Defaults,
    DefaultsDisabled,
    Toggle {
        enable: bool,
        runlevels: Vec<SysvRunlevel>,
    },
    LegacyDefaults {
        action: LegacySequenceKind,
        clauses: Vec<LegacySequenceClause>,
    },
}

impl UpdateRcDOperation {
    fn action(&self) -> &'static str {
        match self {
            Self::Remove => "remove",
            Self::Defaults => "defaults",
            Self::DefaultsDisabled => "defaults-disabled",
            Self::Toggle { enable: true, .. } => "enable",
            Self::Toggle { enable: false, .. } => "disable",
            Self::LegacyDefaults {
                action: LegacySequenceKind::Start,
                ..
            } => "start",
            Self::LegacyDefaults {
                action: LegacySequenceKind::Stop,
                ..
            } => "stop",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateRcDInvocation {
    Help,
    Apply {
        force: bool,
        service: String,
        operation: UpdateRcDOperation,
    },
}

impl UpdateRcDInvocation {
    pub fn parse(argv: &[String]) -> Result<Self, HelperGrammarError> {
        let mut force = false;
        let mut index = 0;
        while let Some(argument) = argv.get(index)
            && argument.starts_with('-')
        {
            match argument.as_str() {
                "-f" => force = true,
                "-h" | "--help" => return Ok(Self::Help),
                option => {
                    return Err(HelperGrammarError::UnknownOption {
                        helper: HELPER,
                        option: option.to_string(),
                    });
                }
            }
            index += 1;
        }

        let service = argv
            .get(index)
            .ok_or(HelperGrammarError::MissingOperand {
                helper: HELPER,
                operand: "initscript basename",
            })?
            .clone();
        validate_word(HELPER, "initscript basename", &service)?;
        index += 1;
        let action = argv
            .get(index)
            .ok_or(HelperGrammarError::MissingOperand {
                helper: HELPER,
                operand: "action",
            })?
            .as_str();
        index += 1;
        let trailing = &argv[index..];

        let operation = match action {
            "remove" => {
                reject_trailing(trailing)?;
                UpdateRcDOperation::Remove
            }
            "defaults" => {
                reject_trailing(trailing)?;
                UpdateRcDOperation::Defaults
            }
            "defaults-disabled" => {
                reject_trailing(trailing)?;
                UpdateRcDOperation::DefaultsDisabled
            }
            "enable" | "disable" => {
                let runlevels = trailing
                    .iter()
                    .map(|value| SysvRunlevel::parse(value))
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(invalid) = runlevels.iter().find(|runlevel| !runlevel.is_toggle()) {
                    return Err(HelperGrammarError::InvalidRunlevel {
                        helper: HELPER,
                        value: invalid.as_str().to_string(),
                    });
                }
                UpdateRcDOperation::Toggle {
                    enable: action == "enable",
                    runlevels,
                }
            }
            "start" | "stop" => {
                let action = LegacySequenceKind::parse(action)
                    .expect("the start/stop match arm supplies a known legacy action");
                UpdateRcDOperation::LegacyDefaults {
                    action,
                    clauses: parse_legacy_sequence(action, trailing)?,
                }
            }
            unknown => {
                return Err(HelperGrammarError::UnknownAction {
                    helper: HELPER,
                    action: unknown.to_string(),
                });
            }
        };

        Ok(Self::Apply {
            force,
            service,
            operation,
        })
    }

    pub fn argv(&self) -> Vec<String> {
        let Self::Apply {
            force,
            service,
            operation,
        } = self
        else {
            return vec!["--help".to_string()];
        };

        let mut argv = Vec::new();
        if *force {
            argv.push("-f".to_string());
        }
        argv.push(service.clone());
        argv.push(operation.action().to_string());
        match operation {
            UpdateRcDOperation::Toggle { runlevels, .. } => {
                argv.extend(
                    runlevels
                        .iter()
                        .map(|runlevel| runlevel.as_str().to_string()),
                );
            }
            UpdateRcDOperation::LegacyDefaults { clauses, .. } => {
                for (index, clause) in clauses.iter().enumerate() {
                    if index > 0 {
                        argv.push(clause.kind.as_str().to_string());
                    }
                    argv.push(format!("{:02}", clause.sequence));
                    argv.extend(
                        clause
                            .runlevels
                            .iter()
                            .map(|runlevel| runlevel.as_str().to_string()),
                    );
                    argv.push(".".to_string());
                }
            }
            UpdateRcDOperation::Remove
            | UpdateRcDOperation::Defaults
            | UpdateRcDOperation::DefaultsDisabled => {}
        }
        argv
    }
}

fn reject_trailing(trailing: &[String]) -> Result<(), HelperGrammarError> {
    if let Some(operand) = trailing.first() {
        return Err(HelperGrammarError::UnexpectedOperand {
            helper: HELPER,
            operand: operand.clone(),
        });
    }
    Ok(())
}

fn parse_legacy_sequence(
    initial_kind: LegacySequenceKind,
    arguments: &[String],
) -> Result<Vec<LegacySequenceClause>, HelperGrammarError> {
    if arguments.is_empty() {
        return Ok(Vec::new());
    }

    let mut index = 0;
    let mut kind = initial_kind;
    if LegacySequenceKind::parse(&arguments[0]) == Some(initial_kind) {
        index += 1;
    }
    let mut clauses = Vec::new();

    while index < arguments.len() {
        let sequence =
            arguments
                .get(index)
                .ok_or_else(|| HelperGrammarError::InvalidLegacySequence {
                    helper: HELPER,
                    reason: "missing two-digit sequence number".to_string(),
                })?;
        if sequence.len() != 2 || !sequence.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(HelperGrammarError::InvalidLegacySequence {
                helper: HELPER,
                reason: format!("sequence number {sequence:?} is not exactly two digits"),
            });
        }
        let sequence = sequence
            .parse::<u8>()
            .expect("two ASCII digits always fit in u8");
        index += 1;

        let mut runlevels = Vec::new();
        while let Some(argument) = arguments.get(index)
            && argument != "."
        {
            if LegacySequenceKind::parse(argument).is_some() {
                return Err(HelperGrammarError::InvalidLegacySequence {
                    helper: HELPER,
                    reason: format!("clause before {argument:?} is missing its '.' terminator"),
                });
            }
            runlevels.push(SysvRunlevel::parse(argument)?);
            index += 1;
        }
        if runlevels.is_empty() {
            return Err(HelperGrammarError::InvalidLegacySequence {
                helper: HELPER,
                reason: "a clause requires at least one runlevel".to_string(),
            });
        }
        if arguments.get(index).map(String::as_str) != Some(".") {
            return Err(HelperGrammarError::InvalidLegacySequence {
                helper: HELPER,
                reason: "a clause must end with '.'".to_string(),
            });
        }
        index += 1;
        clauses.push(LegacySequenceClause {
            kind,
            sequence,
            runlevels,
        });

        if index < arguments.len() {
            kind = LegacySequenceKind::parse(&arguments[index]).ok_or_else(|| {
                HelperGrammarError::InvalidLegacySequence {
                    helper: HELPER,
                    reason: format!(
                        "expected start or stop before the next clause, got {:?}",
                        arguments[index]
                    ),
                }
            })?;
            index += 1;
        }
    }

    Ok(clauses)
}
