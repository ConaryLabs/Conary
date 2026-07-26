// crates/conary-core/src/boot_runtime/argv.rs

use super::BootRuntimeParseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OptionArity {
    None,
    One,
    Two,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OptionSpec {
    pub id: &'static str,
    pub short: Option<char>,
    pub long: Option<&'static str>,
    pub arity: OptionArity,
}

impl OptionSpec {
    pub const fn flag(id: &'static str, short: Option<char>, long: &'static str) -> Self {
        Self {
            id,
            short,
            long: Some(long),
            arity: OptionArity::None,
        }
    }

    pub const fn short_flag(id: &'static str, short: char) -> Self {
        Self {
            id,
            short: Some(short),
            long: None,
            arity: OptionArity::None,
        }
    }

    pub const fn value(id: &'static str, short: Option<char>, long: &'static str) -> Self {
        Self {
            id,
            short,
            long: Some(long),
            arity: OptionArity::One,
        }
    }

    pub const fn short_value(id: &'static str, short: char) -> Self {
        Self {
            id,
            short: Some(short),
            long: None,
            arity: OptionArity::One,
        }
    }

    pub const fn pair(id: &'static str, short: Option<char>, long: &'static str) -> Self {
        Self {
            id,
            short,
            long: Some(long),
            arity: OptionArity::Two,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ParsedOption {
    pub id: &'static str,
    pub spelling: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScannedArgs {
    pub options: Vec<ParsedOption>,
    pub operands: Vec<String>,
}

/// Scan the exact GNU-style forms used by kmod, dracut, mkinitcpio and systemd.
///
/// Long-option prefix abbreviations are intentionally rejected. Although some
/// upstream parser implementations accept them as an implementation detail,
/// they are not declared spellings and cannot be durable authority.
pub(super) fn scan_gnu(
    command: &'static str,
    argv: &[&str],
    specs: &[OptionSpec],
) -> Result<ScannedArgs, BootRuntimeParseError> {
    let mut options = Vec::new();
    let mut operands = Vec::new();
    let mut index = 0;
    let mut options_enabled = true;

    while index < argv.len() {
        let token = argv[index];
        if options_enabled && token == "--" {
            options_enabled = false;
            index += 1;
            continue;
        }
        if !options_enabled || token == "-" || !token.starts_with('-') {
            operands.push(token.to_string());
            index += 1;
            continue;
        }

        if let Some(long) = token.strip_prefix("--") {
            let (name, joined) = long
                .split_once('=')
                .map_or((long, None), |(name, value)| (name, Some(value)));
            let spec = specs
                .iter()
                .find(|spec| spec.long == Some(name))
                .ok_or_else(|| BootRuntimeParseError::UnknownOption {
                    command,
                    option: token.to_string(),
                })?;
            let spelling = format!("--{name}");
            let (values, consumed) =
                collect_values(command, &spelling, spec.arity, joined, argv, index + 1)?;
            options.push(ParsedOption {
                id: spec.id,
                spelling,
                values,
            });
            index += 1 + consumed;
            continue;
        }

        let short = &token[1..];
        for (offset, name) in short.char_indices() {
            let spec = specs
                .iter()
                .find(|spec| spec.short == Some(name))
                .ok_or_else(|| BootRuntimeParseError::UnknownOption {
                    command,
                    option: format!("-{name}"),
                })?;
            let spelling = format!("-{name}");
            let rest_start = offset + name.len_utf8();
            let joined = (spec.arity != OptionArity::None && rest_start < short.len())
                .then_some(&short[rest_start..]);
            let (values, consumed) =
                collect_values(command, &spelling, spec.arity, joined, argv, index + 1)?;
            options.push(ParsedOption {
                id: spec.id,
                spelling,
                values,
            });
            if spec.arity != OptionArity::None {
                index += consumed;
                break;
            }
        }
        index += 1;
    }

    Ok(ScannedArgs { options, operands })
}

fn collect_values(
    command: &'static str,
    spelling: &str,
    arity: OptionArity,
    joined: Option<&str>,
    argv: &[&str],
    next: usize,
) -> Result<(Vec<String>, usize), BootRuntimeParseError> {
    let required = match arity {
        OptionArity::None => {
            if joined.is_some() {
                return Err(BootRuntimeParseError::UnexpectedOptionValue {
                    command,
                    option: spelling.to_string(),
                });
            }
            return Ok((Vec::new(), 0));
        }
        OptionArity::One => 1,
        OptionArity::Two => 2,
    };

    let mut values = Vec::with_capacity(required);
    if let Some(value) = joined {
        values.push(value.to_string());
    }
    let missing = required - values.len();
    if argv.len().saturating_sub(next) < missing {
        return Err(BootRuntimeParseError::MissingOptionValue {
            command,
            option: spelling.to_string(),
            values: required,
        });
    }
    values.extend(
        argv[next..next + missing]
            .iter()
            .map(|value| (*value).to_string()),
    );
    Ok((values, missing))
}

pub(super) fn require_nonempty(
    command: &'static str,
    option: &str,
    value: String,
    expected: &'static str,
) -> Result<String, BootRuntimeParseError> {
    if value.is_empty() {
        Err(BootRuntimeParseError::InvalidOptionValue {
            command,
            option: option.to_string(),
            value,
            expected,
        })
    } else {
        Ok(value)
    }
}

pub(super) fn parse_boolean(
    command: &'static str,
    option: &str,
    value: String,
) -> Result<bool, BootRuntimeParseError> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "yes" | "y" | "true" | "t" | "on" => Ok(true),
        "0" | "no" | "n" | "false" | "f" | "off" => Ok(false),
        _ => Err(BootRuntimeParseError::InvalidOptionValue {
            command,
            option: option.to_string(),
            value,
            expected: "a systemd boolean (yes/no, true/false, on/off, 1/0)",
        }),
    }
}

pub(super) fn only_operand_count(
    command: &'static str,
    action: &'static str,
    operands: &[String],
    min: usize,
    max: usize,
    expected: &'static str,
) -> Result<(), BootRuntimeParseError> {
    if operands.len() < min {
        return Err(BootRuntimeParseError::MissingOperand {
            command,
            action,
            expected,
        });
    }
    if operands.len() > max {
        return Err(BootRuntimeParseError::UnexpectedOperand {
            command,
            action,
            operand: operands[max].clone(),
        });
    }
    Ok(())
}
