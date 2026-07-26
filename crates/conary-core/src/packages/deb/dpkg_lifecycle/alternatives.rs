// conary-core/src/packages/deb/dpkg_lifecycle/alternatives.rs

use super::common::{
    DpkgLifecycleGrammarError, UPDATE_ALTERNATIVES, require_absolute, require_no_nul,
    require_nonempty, required,
};
use std::collections::BTreeSet;

/// The three verbosity states and the default output state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AlternativesOutput {
    Quiet,
    #[default]
    Normal,
    Verbose,
    Debug,
}

/// One source-ordered option. Ordering is semantically significant because
/// `--root` resets several directories established by earlier options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlternativesOption {
    AlternativesDirectory(Vec<u8>),
    AdministrativeDirectory(Vec<u8>),
    InstallationDirectory(Vec<u8>),
    Root(Vec<u8>),
    Log(Vec<u8>),
    Force,
    SkipAutomatic,
    Output(AlternativesOutput),
}

impl AlternativesOption {
    fn append_argv(&self, argv: &mut Vec<Vec<u8>>) {
        match self {
            Self::AlternativesDirectory(value) => {
                argv.extend([b"--altdir".to_vec(), value.clone()]);
            }
            Self::AdministrativeDirectory(value) => {
                argv.extend([b"--admindir".to_vec(), value.clone()]);
            }
            Self::InstallationDirectory(value) => {
                argv.extend([b"--instdir".to_vec(), value.clone()]);
            }
            Self::Root(value) => argv.extend([b"--root".to_vec(), value.clone()]),
            Self::Log(value) => argv.extend([b"--log".to_vec(), value.clone()]),
            Self::Force => argv.push(b"--force".to_vec()),
            Self::SkipAutomatic => argv.push(b"--skip-auto".to_vec()),
            Self::Output(AlternativesOutput::Quiet) => argv.push(b"--quiet".to_vec()),
            Self::Output(AlternativesOutput::Verbose) => argv.push(b"--verbose".to_vec()),
            Self::Output(AlternativesOutput::Debug) => argv.push(b"--debug".to_vec()),
            Self::Output(AlternativesOutput::Normal) => {}
        }
    }
}

/// Source-ordered option list for update-alternatives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlternativesOptions {
    pub occurrences: Vec<AlternativesOption>,
}

impl AlternativesOptions {
    pub fn output(&self) -> AlternativesOutput {
        self.occurrences
            .iter()
            .filter_map(|option| match option {
                AlternativesOption::Output(output) => Some(*output),
                _ => None,
            })
            .next_back()
            .unwrap_or_default()
    }

    pub fn force(&self) -> bool {
        self.occurrences
            .iter()
            .any(|option| matches!(option, AlternativesOption::Force))
    }

    pub fn skip_automatic(&self) -> bool {
        self.occurrences
            .iter()
            .any(|option| matches!(option, AlternativesOption::SkipAutomatic))
    }

    fn argv(&self) -> Vec<Vec<u8>> {
        let mut argv = Vec::new();
        for option in &self.occurrences {
            option.append_argv(&mut argv);
        }
        argv
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlternativesSlave {
    pub link: Vec<u8>,
    pub name: Vec<u8>,
    pub path: Vec<u8>,
}

/// Every update-alternatives command in dpkg 1.23.7.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlternativesAction {
    Install {
        link: Vec<u8>,
        name: Vec<u8>,
        path: Vec<u8>,
        priority: i32,
        slaves: Vec<AlternativesSlave>,
    },
    Set {
        name: Vec<u8>,
        path: Vec<u8>,
    },
    Remove {
        name: Vec<u8>,
        path: Vec<u8>,
    },
    RemoveAll {
        name: Vec<u8>,
    },
    All,
    Automatic {
        name: Vec<u8>,
    },
    Display {
        name: Vec<u8>,
    },
    GetSelections,
    SetSelections,
    Query {
        name: Vec<u8>,
    },
    List {
        name: Vec<u8>,
    },
    Configure {
        name: Vec<u8>,
    },
    Help,
    Version,
}

impl AlternativesAction {
    pub const DOCUMENTED_OPTIONS: [&'static [u8]; 14] = [
        b"--install",
        b"--set",
        b"--remove",
        b"--remove-all",
        b"--all",
        b"--auto",
        b"--display",
        b"--get-selections",
        b"--set-selections",
        b"--query",
        b"--list",
        b"--config",
        b"--help",
        b"--version",
    ];

    pub const fn effect(&self) -> AlternativesEffect {
        match self {
            Self::Display { .. }
            | Self::GetSelections
            | Self::Query { .. }
            | Self::List { .. }
            | Self::Help
            | Self::Version => AlternativesEffect::ReadOnly,
            Self::All | Self::Configure { .. } => AlternativesEffect::InteractiveMutation,
            Self::Install { .. }
            | Self::Set { .. }
            | Self::Remove { .. }
            | Self::RemoveAll { .. }
            | Self::Automatic { .. }
            | Self::SetSelections => AlternativesEffect::Mutation,
        }
    }

    pub const fn reads_selection_stdin(&self) -> bool {
        matches!(self, Self::SetSelections)
    }

    fn name(&self) -> &'static [u8] {
        match self {
            Self::Install { .. } => b"--install",
            Self::Set { .. } => b"--set",
            Self::Remove { .. } => b"--remove",
            Self::RemoveAll { .. } => b"--remove-all",
            Self::All => b"--all",
            Self::Automatic { .. } => b"--auto",
            Self::Display { .. } => b"--display",
            Self::GetSelections => b"--get-selections",
            Self::SetSelections => b"--set-selections",
            Self::Query { .. } => b"--query",
            Self::List { .. } => b"--list",
            Self::Configure { .. } => b"--config",
            Self::Help => b"--help",
            Self::Version => b"--version",
        }
    }

    fn append_argv(&self, argv: &mut Vec<Vec<u8>>) {
        argv.push(self.name().to_vec());
        match self {
            Self::Install {
                link,
                name,
                path,
                priority,
                slaves,
            } => {
                argv.extend([
                    link.clone(),
                    name.clone(),
                    path.clone(),
                    priority.to_string().into_bytes(),
                ]);
                for slave in slaves {
                    argv.extend([
                        b"--slave".to_vec(),
                        slave.link.clone(),
                        slave.name.clone(),
                        slave.path.clone(),
                    ]);
                }
            }
            Self::Set { name, path } | Self::Remove { name, path } => {
                argv.extend([name.clone(), path.clone()]);
            }
            Self::RemoveAll { name }
            | Self::Automatic { name }
            | Self::Display { name }
            | Self::Query { name }
            | Self::List { name }
            | Self::Configure { name } => argv.push(name.clone()),
            Self::All | Self::GetSelections | Self::SetSelections | Self::Help | Self::Version => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlternativesEffect {
    ReadOnly,
    Mutation,
    InteractiveMutation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlternativesInvocation {
    pub options: AlternativesOptions,
    pub action: AlternativesAction,
}

impl AlternativesInvocation {
    pub fn parse(argv: &[Vec<u8>]) -> Result<Self, DpkgLifecycleGrammarError> {
        let mut options = AlternativesOptions::default();
        let mut action: Option<AlternativesAction> = None;
        let mut index = 0;
        while let Some(argument) = argv.get(index) {
            match argument.as_slice() {
                b"--quiet" => options
                    .occurrences
                    .push(AlternativesOption::Output(AlternativesOutput::Quiet)),
                b"--verbose" => options
                    .occurrences
                    .push(AlternativesOption::Output(AlternativesOutput::Verbose)),
                b"--debug" => options
                    .occurrences
                    .push(AlternativesOption::Output(AlternativesOutput::Debug)),
                b"--force" => options.occurrences.push(AlternativesOption::Force),
                b"--skip-auto" => options.occurrences.push(AlternativesOption::SkipAutomatic),
                b"--altdir" | b"--admindir" | b"--instdir" | b"--root" | b"--log" => {
                    let value = next_value(argv, &mut index, argument)?;
                    let option = match argument.as_slice() {
                        b"--altdir" => AlternativesOption::AlternativesDirectory(value),
                        b"--admindir" => AlternativesOption::AdministrativeDirectory(value),
                        b"--instdir" => AlternativesOption::InstallationDirectory(value),
                        b"--root" => AlternativesOption::Root(value),
                        b"--log" => AlternativesOption::Log(value),
                        _ => unreachable!("matched directory option"),
                    };
                    options.occurrences.push(option);
                }
                b"--install" => {
                    let values = next_values(argv, &mut index, argument, 4)?;
                    let priority = parse_priority(&values[3])?;
                    validate_name("install", &values[1])?;
                    require_absolute(UPDATE_ALTERNATIVES, "install", "master link", &values[0])?;
                    require_absolute(
                        UPDATE_ALTERNATIVES,
                        "install",
                        "alternative path",
                        &values[2],
                    )?;
                    if values[0] == values[2] {
                        return invalid(
                            "install",
                            "master link",
                            &values[0],
                            "must differ from the alternative path",
                        );
                    }
                    set_action(
                        &mut action,
                        AlternativesAction::Install {
                            link: values[0].clone(),
                            name: values[1].clone(),
                            path: values[2].clone(),
                            priority,
                            slaves: Vec::new(),
                        },
                    )?;
                }
                b"--slave" => {
                    let values = next_values(argv, &mut index, argument, 3)?;
                    add_slave(&mut action, &values)?;
                }
                b"--set" | b"--remove" => {
                    let values = next_values(argv, &mut index, argument, 2)?;
                    let action_name = if argument == b"--set" {
                        "set"
                    } else {
                        "remove"
                    };
                    validate_name(action_name, &values[0])?;
                    require_absolute(
                        UPDATE_ALTERNATIVES,
                        action_name,
                        "alternative path",
                        &values[1],
                    )?;
                    let parsed = if argument == b"--set" {
                        AlternativesAction::Set {
                            name: values[0].clone(),
                            path: values[1].clone(),
                        }
                    } else {
                        AlternativesAction::Remove {
                            name: values[0].clone(),
                            path: values[1].clone(),
                        }
                    };
                    set_action(&mut action, parsed)?;
                }
                b"--remove-all" | b"--auto" | b"--display" | b"--query" | b"--list"
                | b"--config" => {
                    let value = next_value(argv, &mut index, argument)?;
                    validate_name(action_label(argument), &value)?;
                    let parsed = match argument.as_slice() {
                        b"--remove-all" => AlternativesAction::RemoveAll { name: value },
                        b"--auto" => AlternativesAction::Automatic { name: value },
                        b"--display" => AlternativesAction::Display { name: value },
                        b"--query" => AlternativesAction::Query { name: value },
                        b"--list" => AlternativesAction::List { name: value },
                        b"--config" => AlternativesAction::Configure { name: value },
                        _ => unreachable!("matched one-name action"),
                    };
                    set_action(&mut action, parsed)?;
                }
                b"--all" => set_action(&mut action, AlternativesAction::All)?,
                b"--get-selections" => {
                    set_action(&mut action, AlternativesAction::GetSelections)?;
                }
                b"--set-selections" => {
                    set_action(&mut action, AlternativesAction::SetSelections)?;
                }
                b"--help" => set_action(&mut action, AlternativesAction::Help)?,
                b"--version" => set_action(&mut action, AlternativesAction::Version)?,
                _ if !argument.starts_with(b"--") => {
                    return Err(DpkgLifecycleGrammarError::UnknownOption {
                        tool: UPDATE_ALTERNATIVES,
                        option: argument.clone(),
                    });
                }
                _ => {
                    return Err(DpkgLifecycleGrammarError::UnknownOption {
                        tool: UPDATE_ALTERNATIVES,
                        option: argument.clone(),
                    });
                }
            }
            index += 1;
        }
        let action = action.ok_or(DpkgLifecycleGrammarError::MissingAction {
            tool: UPDATE_ALTERNATIVES,
        })?;
        Ok(Self { options, action })
    }

    pub fn argv(&self) -> Vec<Vec<u8>> {
        let mut argv = self.options.argv();
        self.action.append_argv(&mut argv);
        argv
    }
}

fn set_action(
    slot: &mut Option<AlternativesAction>,
    action: AlternativesAction,
) -> Result<(), DpkgLifecycleGrammarError> {
    if let Some(first) = slot {
        return Err(DpkgLifecycleGrammarError::ConflictingActions {
            tool: UPDATE_ALTERNATIVES,
            first: first.name().to_vec(),
            second: action.name().to_vec(),
        });
    }
    *slot = Some(action);
    Ok(())
}

fn next_value(
    argv: &[Vec<u8>],
    index: &mut usize,
    option: &[u8],
) -> Result<Vec<u8>, DpkgLifecycleGrammarError> {
    *index += 1;
    let value = required(
        UPDATE_ALTERNATIVES,
        action_label(option),
        "argument",
        argv.get(*index),
    )?
    .to_vec();
    require_no_nul(
        UPDATE_ALTERNATIVES,
        action_label(option),
        "argument",
        &value,
    )?;
    Ok(value)
}

fn next_values(
    argv: &[Vec<u8>],
    index: &mut usize,
    option: &[u8],
    count: usize,
) -> Result<Vec<Vec<u8>>, DpkgLifecycleGrammarError> {
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(next_value(argv, index, option)?);
    }
    Ok(values)
}

fn action_label(option: &[u8]) -> &'static str {
    match option {
        b"--install" => "install",
        b"--slave" => "slave",
        b"--set" => "set",
        b"--remove" => "remove",
        b"--remove-all" => "remove-all",
        b"--auto" => "auto",
        b"--display" => "display",
        b"--query" => "query",
        b"--list" => "list",
        b"--config" => "config",
        b"--altdir" => "altdir",
        b"--admindir" => "admindir",
        b"--instdir" => "instdir",
        b"--root" => "root",
        b"--log" => "log",
        _ => "option",
    }
}

fn parse_priority(value: &[u8]) -> Result<i32, DpkgLifecycleGrammarError> {
    let text =
        std::str::from_utf8(value).map_err(|_| DpkgLifecycleGrammarError::InvalidOperand {
            tool: UPDATE_ALTERNATIVES,
            action: "install",
            operand: "priority",
            value: value.to_vec(),
            reason: "must be a decimal integer",
        })?;
    text.parse()
        .map_err(|_| DpkgLifecycleGrammarError::InvalidOperand {
            tool: UPDATE_ALTERNATIVES,
            action: "install",
            operand: "priority",
            value: value.to_vec(),
            reason: "must be a signed 32-bit decimal integer",
        })
}

fn validate_name(action: &'static str, value: &[u8]) -> Result<(), DpkgLifecycleGrammarError> {
    require_nonempty(UPDATE_ALTERNATIVES, action, "alternative name", value)?;
    if value.iter().any(|byte| matches!(byte, b'/' | b' ' | b'\t')) {
        return invalid(
            action,
            "alternative name",
            value,
            "must not contain '/', space, or tab",
        );
    }
    Ok(())
}

fn invalid<T>(
    action: &'static str,
    operand: &'static str,
    value: &[u8],
    reason: &'static str,
) -> Result<T, DpkgLifecycleGrammarError> {
    Err(DpkgLifecycleGrammarError::InvalidOperand {
        tool: UPDATE_ALTERNATIVES,
        action,
        operand,
        value: value.to_vec(),
        reason,
    })
}

fn add_slave(
    action: &mut Option<AlternativesAction>,
    values: &[Vec<u8>],
) -> Result<(), DpkgLifecycleGrammarError> {
    let Some(AlternativesAction::Install {
        link, name, slaves, ..
    }) = action
    else {
        return Err(DpkgLifecycleGrammarError::InvalidOptionForAction {
            tool: UPDATE_ALTERNATIVES,
            option: "--slave",
            action: "non-install action",
        });
    };
    require_absolute(UPDATE_ALTERNATIVES, "install", "slave link", &values[0])?;
    validate_name("install", &values[1])?;
    require_absolute(UPDATE_ALTERNATIVES, "install", "slave path", &values[2])?;
    if values[0] == values[2] {
        return invalid(
            "install",
            "slave link",
            &values[0],
            "must differ from the slave path",
        );
    }
    if &values[0] == link {
        return invalid(
            "install",
            "slave link",
            &values[0],
            "must differ from the master link",
        );
    }
    if &values[1] == name {
        return invalid(
            "install",
            "slave name",
            &values[1],
            "must differ from the master name",
        );
    }
    let names = slaves
        .iter()
        .map(|slave| slave.name.as_slice())
        .collect::<BTreeSet<_>>();
    let links = slaves
        .iter()
        .map(|slave| slave.link.as_slice())
        .collect::<BTreeSet<_>>();
    if names.contains(values[1].as_slice()) {
        return invalid(
            "install",
            "slave name",
            &values[1],
            "must not be duplicated",
        );
    }
    if links.contains(values[0].as_slice()) {
        return invalid(
            "install",
            "slave link",
            &values[0],
            "must not be duplicated",
        );
    }
    slaves.push(AlternativesSlave {
        link: values[0].clone(),
        name: values[1].clone(),
        path: values[2].clone(),
    });
    Ok(())
}

/// Mode field in update-alternatives' `--get-selections` text contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlternativeSelectionMode {
    Automatic,
    Manual,
}

impl AlternativeSelectionMode {
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Automatic => b"auto",
            Self::Manual => b"manual",
        }
    }
}

/// One current `--get-selections`/`--set-selections` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlternativeSelection {
    pub name: Vec<u8>,
    pub mode: AlternativeSelectionMode,
    pub choice: Vec<u8>,
}

impl AlternativeSelection {
    /// Parse one newline-terminated record. dpkg's fixed input buffer permits
    /// at most 1022 data bytes plus the newline.
    pub fn parse_line(line: &[u8]) -> Result<Self, DpkgLifecycleGrammarError> {
        if line.len() > 1023 || !line.ends_with(b"\n") {
            return invalid(
                "set-selections",
                "selection line",
                line,
                "must end in newline and fit the 1023-byte input record",
            );
        }
        let content = &line[..line.len() - 1];
        if content.contains(&b'\n') || content.contains(&0) {
            return invalid(
                "set-selections",
                "selection line",
                line,
                "must contain exactly one NUL-free newline-terminated record",
            );
        }
        let first = content
            .iter()
            .position(|byte| matches!(byte, b' ' | b'\t'))
            .ok_or_else(|| DpkgLifecycleGrammarError::InvalidOperand {
                tool: UPDATE_ALTERNATIVES,
                action: "set-selections",
                operand: "selection line",
                value: line.to_vec(),
                reason: "must contain name, mode, and choice",
            })?;
        let name = &content[..first];
        let mode_start = skip_blanks(content, first);
        let mode_end = content[mode_start..]
            .iter()
            .position(|byte| matches!(byte, b' ' | b'\t'))
            .map(|offset| mode_start + offset)
            .ok_or_else(|| DpkgLifecycleGrammarError::InvalidOperand {
                tool: UPDATE_ALTERNATIVES,
                action: "set-selections",
                operand: "selection line",
                value: line.to_vec(),
                reason: "must contain a choice after the mode",
            })?;
        let choice_start = skip_blanks(content, mode_end);
        if choice_start >= content.len() {
            return invalid("set-selections", "choice", &[], "must not be empty");
        }
        validate_name("set-selections", name)?;
        let mode = match &content[mode_start..mode_end] {
            b"auto" => AlternativeSelectionMode::Automatic,
            b"manual" => AlternativeSelectionMode::Manual,
            value => {
                return invalid(
                    "set-selections",
                    "mode",
                    value,
                    "must be 'auto' or 'manual'",
                );
            }
        };
        let choice = content[choice_start..].to_vec();
        require_no_nul(UPDATE_ALTERNATIVES, "set-selections", "choice", &choice)?;
        Ok(Self {
            name: name.to_vec(),
            mode,
            choice,
        })
    }

    pub fn line(&self) -> Result<Vec<u8>, DpkgLifecycleGrammarError> {
        validate_name("set-selections", &self.name)?;
        require_nonempty(
            UPDATE_ALTERNATIVES,
            "set-selections",
            "choice",
            &self.choice,
        )?;
        if self.choice.contains(&b'\n') {
            return invalid(
                "set-selections",
                "choice",
                &self.choice,
                "must not contain newline",
            );
        }
        let mut line = Vec::new();
        line.extend_from_slice(&self.name);
        line.push(b' ');
        line.extend_from_slice(self.mode.as_bytes());
        line.push(b' ');
        line.extend_from_slice(&self.choice);
        line.push(b'\n');
        if line.len() > 1023 {
            return invalid(
                "set-selections",
                "selection line",
                &line,
                "exceeds the 1023-byte input record",
            );
        }
        Ok(line)
    }
}

fn skip_blanks(value: &[u8], mut index: usize) -> usize {
    while value
        .get(index)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        index += 1;
    }
    index
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlternativesExitStatus {
    Success,
    Error,
}

impl AlternativesExitStatus {
    pub fn from_exit_code(status: u8) -> Result<Self, DpkgLifecycleGrammarError> {
        match status {
            0 => Ok(Self::Success),
            2 => Ok(Self::Error),
            status => Err(DpkgLifecycleGrammarError::UnsupportedExitStatus {
                tool: UPDATE_ALTERNATIVES,
                status,
            }),
        }
    }

    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Error => 2,
        }
    }
}
