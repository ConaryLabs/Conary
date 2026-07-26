// conary-core/src/packages/deb/debconf/protocol.rs

//! Byte-preserving debconf command and response grammar.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebconfPriority {
    Low,
    Medium,
    High,
    Critical,
}

impl DebconfPriority {
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Low => b"low",
            Self::Medium => b"medium",
            Self::High => b"high",
            Self::Critical => b"critical",
        }
    }

    fn parse(raw: &[u8]) -> Option<Self> {
        match raw {
            b"low" => Some(Self::Low),
            b"medium" => Some(Self::Medium),
            b"high" => Some(Self::High),
            b"critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebconfProtocolVersion {
    pub major: u16,
    pub minor: Option<u16>,
}

impl DebconfProtocolVersion {
    pub const CURRENT: Self = Self {
        major: 2,
        minor: Some(1),
    };

    fn parse(raw: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(raw).ok()?;
        let mut parts = text.split('.');
        let major = parse_decimal(parts.next()?)?;
        let minor = match parts.next() {
            Some(raw) => Some(parse_decimal(raw)?),
            None => None,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(Self { major, minor })
    }

    pub fn wire_bytes(&self) -> Vec<u8> {
        match self.minor {
            Some(minor) => format!("{}.{}", self.major, minor).into_bytes(),
            None => self.major.to_string().into_bytes(),
        }
    }
}

fn parse_decimal(raw: &str) -> Option<u16> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DebconfFlagValue {
    True,
    False,
}

impl DebconfFlagValue {
    pub fn as_bool(self) -> bool {
        matches!(self, Self::True)
    }

    pub fn as_bytes(self) -> &'static [u8] {
        if self.as_bool() { b"true" } else { b"false" }
    }

    fn parse(raw: &[u8]) -> Option<Self> {
        match raw {
            b"true" => Some(Self::True),
            b"false" => Some(Self::False),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum DebconfProgressCommand {
    Start {
        minimum: i64,
        maximum: i64,
        question: Vec<u8>,
    },
    Set {
        value: i64,
    },
    Step {
        increment: i64,
    },
    Info {
        question: Vec<u8>,
    },
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum DebconfCommand {
    Version {
        version: Option<DebconfProtocolVersion>,
    },
    Capb {
        capabilities: Vec<Vec<u8>>,
    },
    Title {
        title: Vec<u8>,
    },
    SetTitle {
        question: Vec<u8>,
    },
    Input {
        priority: DebconfPriority,
        question: Vec<u8>,
    },
    Go,
    Clear,
    BeginBlock,
    EndBlock,
    Stop,
    Get {
        question: Vec<u8>,
    },
    Set {
        question: Vec<u8>,
        value: Vec<u8>,
    },
    Reset {
        question: Vec<u8>,
    },
    Subst {
        question: Vec<u8>,
        key: Vec<u8>,
        value: Vec<u8>,
    },
    FGet {
        question: Vec<u8>,
        flag: Vec<u8>,
    },
    FSet {
        question: Vec<u8>,
        flag: Vec<u8>,
        value: DebconfFlagValue,
    },
    MetaGet {
        question: Vec<u8>,
        field: Vec<u8>,
    },
    Register {
        template: Vec<u8>,
        question: Vec<u8>,
    },
    Unregister {
        question: Vec<u8>,
    },
    Purge,
    Info {
        question: Option<Vec<u8>>,
    },
    Data {
        template: Vec<u8>,
        item: Vec<u8>,
        value: Vec<u8>,
    },
    Progress(DebconfProgressCommand),
    XLoadTemplateFile {
        path: Vec<u8>,
        owner: Option<Vec<u8>>,
    },
    Visible {
        priority: DebconfPriority,
        question: Vec<u8>,
    },
    Exist {
        question: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebconfCommandParseError {
    pub message: Vec<u8>,
}

impl DebconfCommandParseError {
    fn incorrect_arguments() -> Self {
        Self {
            message: b"Incorrect number of arguments".to_vec(),
        }
    }

    pub fn response(&self) -> DebconfResponse {
        DebconfResponse::new(DebconfResultCode::SyntaxError, self.message.clone())
    }
}

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebconfResultCode {
    Success = 0,
    EscapedData = 1,
    InvalidParameters = 10,
    SyntaxError = 20,
    CommandSpecific = 30,
    InternalError = 100,
}

impl DebconfResultCode {
    pub fn as_u16(self) -> u16 {
        self as u16
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebconfResponse {
    pub code: DebconfResultCode,
    /// Bytes following the numeric status and separating whitespace.
    pub ret: Vec<u8>,
}

impl DebconfResponse {
    pub fn new(code: DebconfResultCode, ret: impl Into<Vec<u8>>) -> Self {
        let mut ret = ret.into();
        if let Some(newline) = ret.iter().position(|byte| *byte == b'\n') {
            ret.truncate(newline);
        }
        Self { code, ret }
    }

    pub fn success(ret: impl Into<Vec<u8>>) -> Self {
        Self::new(DebconfResultCode::Success, ret)
    }

    pub fn wire_bytes(&self) -> Vec<u8> {
        let mut output = self.code.as_u16().to_string().into_bytes();
        if !self.ret.is_empty() {
            output.push(b' ');
            output.extend_from_slice(&self.ret);
        }
        output.push(b'\n');
        output
    }

    /// Return the status and `RET` bytes observed by the standard debconf
    /// clients. Status 1 is converted to success after unescaping `RET`.
    pub fn caller_result(&self) -> DebconfCallerResult {
        if self.code == DebconfResultCode::EscapedData {
            DebconfCallerResult {
                status: DebconfResultCode::Success,
                ret: unescape_return(&self.ret),
            }
        } else {
            DebconfCallerResult {
                status: self.code,
                ret: self.ret.clone(),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebconfCallerResult {
    pub status: DebconfResultCode,
    pub ret: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebconfExecution {
    Reply(DebconfResponse),
    /// `STOP` has no protocol reply; standard clients deliberately do not
    /// attempt to read one.
    Stopped,
}

pub fn parse_command(
    raw_line: &[u8],
    escape_capability: bool,
) -> Result<DebconfCommand, DebconfCommandParseError> {
    let line = strip_record_terminator(raw_line)?;
    let words = if escape_capability {
        unescape_split(line)
    } else {
        whitespace_split(line)
    };
    let Some((raw_command, arguments)) = words.split_first() else {
        return Err(DebconfCommandParseError {
            message: quoted_line(b"Bad line \"", line, b"\" received from confmodule."),
        });
    };
    parse_words(raw_command, arguments, Some(line))
}

/// Parse an already tokenized native command without serializing its arguments
/// through the line protocol. Bytes inside each argument, including repeated
/// whitespace, newlines, and empty trailing values, remain authoritative.
pub fn parse_argv(
    raw_command: &[u8],
    arguments: &[Vec<u8>],
) -> Result<DebconfCommand, DebconfCommandParseError> {
    if raw_command.is_empty() || raw_command.iter().any(u8::is_ascii_whitespace) {
        return Err(DebconfCommandParseError {
            message: b"Command name is empty or contains whitespace".to_vec(),
        });
    }
    parse_words(raw_command, arguments, None)
}

fn parse_words(
    raw_command: &[u8],
    arguments: &[Vec<u8>],
    source_line: Option<&[u8]>,
) -> Result<DebconfCommand, DebconfCommandParseError> {
    let mut command = raw_command.to_vec();
    command.make_ascii_lowercase();
    match command.as_slice() {
        b"version" => {
            arity_between(arguments, 0, 1)?;
            let version = arguments
                .first()
                .map(|raw| {
                    DebconfProtocolVersion::parse(raw).ok_or_else(|| DebconfCommandParseError {
                        message: quoted_line(b"Invalid protocol version \"", raw, b"\""),
                    })
                })
                .transpose()?;
            Ok(DebconfCommand::Version { version })
        }
        b"capb" => Ok(DebconfCommand::Capb {
            capabilities: arguments.to_vec(),
        }),
        b"title" => Ok(DebconfCommand::Title {
            title: join_words(arguments),
        }),
        b"settitle" => {
            arity(arguments, 1)?;
            Ok(DebconfCommand::SetTitle {
                question: arguments[0].clone(),
            })
        }
        b"input" => {
            arity(arguments, 2)?;
            let priority =
                DebconfPriority::parse(&arguments[0]).ok_or_else(|| DebconfCommandParseError {
                    message: quoted_line(b"\"", &arguments[0], b"\" is not a valid priority"),
                })?;
            Ok(DebconfCommand::Input {
                priority,
                question: arguments[1].clone(),
            })
        }
        b"go" => {
            arity(arguments, 0)?;
            Ok(DebconfCommand::Go)
        }
        b"clear" => {
            arity(arguments, 0)?;
            Ok(DebconfCommand::Clear)
        }
        b"beginblock" => {
            arity(arguments, 0)?;
            Ok(DebconfCommand::BeginBlock)
        }
        b"endblock" => {
            arity(arguments, 0)?;
            Ok(DebconfCommand::EndBlock)
        }
        b"stop" => {
            arity(arguments, 0)?;
            Ok(DebconfCommand::Stop)
        }
        b"get" => one_question(arguments, |question| DebconfCommand::Get { question }),
        b"set" => {
            arity_at_least(arguments, 1)?;
            Ok(DebconfCommand::Set {
                question: arguments[0].clone(),
                value: join_words(&arguments[1..]),
            })
        }
        b"reset" => one_question(arguments, |question| DebconfCommand::Reset { question }),
        b"subst" => {
            arity_at_least(arguments, 2)?;
            Ok(DebconfCommand::Subst {
                question: arguments[0].clone(),
                key: arguments[1].clone(),
                value: join_words(&arguments[2..]),
            })
        }
        b"fget" => {
            arity(arguments, 2)?;
            Ok(DebconfCommand::FGet {
                question: arguments[0].clone(),
                flag: arguments[1].clone(),
            })
        }
        b"fset" => {
            arity(arguments, 3)?;
            let value =
                DebconfFlagValue::parse(&arguments[2]).ok_or_else(|| DebconfCommandParseError {
                    message: b"Flag value must be exactly \"true\" or \"false\"".to_vec(),
                })?;
            Ok(DebconfCommand::FSet {
                question: arguments[0].clone(),
                flag: arguments[1].clone(),
                value,
            })
        }
        b"metaget" => {
            arity(arguments, 2)?;
            Ok(DebconfCommand::MetaGet {
                question: arguments[0].clone(),
                field: arguments[1].clone(),
            })
        }
        b"register" => {
            arity(arguments, 2)?;
            Ok(DebconfCommand::Register {
                template: arguments[0].clone(),
                question: arguments[1].clone(),
            })
        }
        b"unregister" => one_question(arguments, |question| DebconfCommand::Unregister {
            question,
        }),
        b"purge" => {
            arity(arguments, 0)?;
            Ok(DebconfCommand::Purge)
        }
        b"info" => {
            arity_between(arguments, 0, 1)?;
            Ok(DebconfCommand::Info {
                question: arguments.first().cloned(),
            })
        }
        b"data" => {
            arity_at_least(arguments, 3)?;
            Ok(DebconfCommand::Data {
                template: arguments[0].clone(),
                item: arguments[1].clone(),
                value: data_unescape(&join_words(&arguments[2..])),
            })
        }
        b"progress" => parse_progress(arguments).map(DebconfCommand::Progress),
        b"x_loadtemplatefile" => {
            arity_between(arguments, 1, 2)?;
            Ok(DebconfCommand::XLoadTemplateFile {
                path: arguments[0].clone(),
                owner: arguments.get(1).cloned(),
            })
        }
        b"visible" => {
            arity(arguments, 2)?;
            let priority =
                DebconfPriority::parse(&arguments[0]).ok_or_else(|| DebconfCommandParseError {
                    message: quoted_line(b"\"", &arguments[0], b"\" is not a valid priority"),
                })?;
            Ok(DebconfCommand::Visible {
                priority,
                question: arguments[1].clone(),
            })
        }
        b"exist" => one_question(arguments, |question| DebconfCommand::Exist { question }),
        _ => {
            let mut message = b"Unsupported command \"".to_vec();
            message.extend_from_slice(&command);
            if let Some(line) = source_line {
                message.extend_from_slice(b"\" (full line was \"");
                message.extend_from_slice(line);
                message.extend_from_slice(b"\") received from confmodule.");
            } else {
                message.extend_from_slice(b"\" received from confmodule.");
            }
            Err(DebconfCommandParseError { message })
        }
    }
}

fn parse_progress(
    arguments: &[Vec<u8>],
) -> Result<DebconfProgressCommand, DebconfCommandParseError> {
    let Some((operation, arguments)) = arguments.split_first() else {
        return Err(DebconfCommandParseError::incorrect_arguments());
    };
    let mut operation = operation.clone();
    operation.make_ascii_lowercase();
    match operation.as_slice() {
        b"start" => {
            arity(arguments, 3)?;
            Ok(DebconfProgressCommand::Start {
                minimum: integer(&arguments[0])?,
                maximum: integer(&arguments[1])?,
                question: arguments[2].clone(),
            })
        }
        b"set" => {
            arity(arguments, 1)?;
            Ok(DebconfProgressCommand::Set {
                value: integer(&arguments[0])?,
            })
        }
        b"step" => {
            arity(arguments, 1)?;
            Ok(DebconfProgressCommand::Step {
                increment: integer(&arguments[0])?,
            })
        }
        b"info" => {
            arity(arguments, 1)?;
            Ok(DebconfProgressCommand::Info {
                question: arguments[0].clone(),
            })
        }
        b"stop" => {
            arity(arguments, 0)?;
            Ok(DebconfProgressCommand::Stop)
        }
        _ => Err(DebconfCommandParseError {
            message: b"Unknown subcommand".to_vec(),
        }),
    }
}

fn integer(raw: &[u8]) -> Result<i64, DebconfCommandParseError> {
    let text = std::str::from_utf8(raw).map_err(|_| DebconfCommandParseError {
        message: b"Progress value is not a valid integer".to_vec(),
    })?;
    if text.is_empty()
        || !text
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_digit() || (index == 0 && byte == b'-'))
        || text == "-"
    {
        return Err(DebconfCommandParseError {
            message: b"Progress value is not a valid integer".to_vec(),
        });
    }
    text.parse().map_err(|_| DebconfCommandParseError {
        message: b"Progress value is outside the supported integer range".to_vec(),
    })
}

fn one_question(
    arguments: &[Vec<u8>],
    make: impl FnOnce(Vec<u8>) -> DebconfCommand,
) -> Result<DebconfCommand, DebconfCommandParseError> {
    arity(arguments, 1)?;
    Ok(make(arguments[0].clone()))
}

fn arity(arguments: &[Vec<u8>], count: usize) -> Result<(), DebconfCommandParseError> {
    if arguments.len() == count {
        Ok(())
    } else {
        Err(DebconfCommandParseError::incorrect_arguments())
    }
}

fn arity_at_least(arguments: &[Vec<u8>], minimum: usize) -> Result<(), DebconfCommandParseError> {
    if arguments.len() >= minimum {
        Ok(())
    } else {
        Err(DebconfCommandParseError::incorrect_arguments())
    }
}

fn arity_between(
    arguments: &[Vec<u8>],
    minimum: usize,
    maximum: usize,
) -> Result<(), DebconfCommandParseError> {
    if (minimum..=maximum).contains(&arguments.len()) {
        Ok(())
    } else {
        Err(DebconfCommandParseError::incorrect_arguments())
    }
}

fn strip_record_terminator(raw: &[u8]) -> Result<&[u8], DebconfCommandParseError> {
    let line = raw.strip_suffix(b"\n").unwrap_or(raw);
    if line.contains(&b'\n') {
        return Err(DebconfCommandParseError {
            message: b"Command contains an unescaped newline".to_vec(),
        });
    }
    Ok(line)
}

fn whitespace_split(line: &[u8]) -> Vec<Vec<u8>> {
    line.split(is_protocol_whitespace)
        .filter(|word| !word.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

/// Matches `Debconf::ConfModule::unescape_split`, including its significant
/// empty command when an escape-capable line begins with whitespace.
fn unescape_split(line: &[u8]) -> Vec<Vec<u8>> {
    if line.is_empty() {
        return Vec::new();
    }
    let mut words = Vec::new();
    let mut word = Vec::new();
    let mut index = 0;
    while index < line.len() {
        match line[index] {
            b'\\' if index + 1 < line.len() => {
                let escaped = line[index + 1];
                word.push(if escaped == b'n' { b'\n' } else { escaped });
                index += 2;
            }
            byte if is_protocol_whitespace(&byte) => {
                words.push(std::mem::take(&mut word));
                while index < line.len() && is_protocol_whitespace(&line[index]) {
                    index += 1;
                }
            }
            byte => {
                word.push(byte);
                index += 1;
            }
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn join_words(words: &[Vec<u8>]) -> Vec<u8> {
    let size = words.iter().map(Vec::len).sum::<usize>() + words.len().saturating_sub(1);
    let mut joined = Vec::with_capacity(size);
    for (index, word) in words.iter().enumerate() {
        if index != 0 {
            joined.push(b' ');
        }
        joined.extend_from_slice(word);
    }
    joined
}

fn data_unescape(raw: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'\\' && index + 1 < raw.len() {
            match raw[index + 1] {
                b'n' => {
                    output.push(b'\n');
                    index += 2;
                    continue;
                }
                b'"' | b'\\' => {
                    output.push(raw[index + 1]);
                    index += 2;
                    continue;
                }
                _ => {}
            }
        }
        output.push(raw[index]);
        index += 1;
    }
    output
}

pub(crate) fn escape_return(raw: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(raw.len());
    for byte in raw {
        match byte {
            b'\\' => output.extend_from_slice(b"\\\\"),
            b'\n' => output.extend_from_slice(b"\\n"),
            byte => output.push(*byte),
        }
    }
    output
}

fn unescape_return(raw: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'\\' && index + 1 < raw.len() {
            output.push(if raw[index + 1] == b'n' {
                b'\n'
            } else {
                raw[index + 1]
            });
            index += 2;
        } else {
            output.push(raw[index]);
            index += 1;
        }
    }
    output
}

fn quoted_line(prefix: &[u8], value: &[u8], suffix: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(prefix.len() + value.len() + suffix.len());
    message.extend_from_slice(prefix);
    message.extend_from_slice(value);
    message.extend_from_slice(suffix);
    message
}

fn is_protocol_whitespace(byte: &u8) -> bool {
    byte.is_ascii_whitespace()
}
