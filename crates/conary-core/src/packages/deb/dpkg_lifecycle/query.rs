// conary-core/src/packages/deb/dpkg_lifecycle/query.rs

use super::common::{
    DPKG_QUERY, DpkgLifecycleGrammarError, long_value, option_value, push_long_value,
    render_with_operands, require_arity,
};

/// Source-supported dpkg-query options. Repeated value options use dpkg's
/// last-one-wins semantics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DpkgQueryOptions {
    pub admindir: Option<Vec<u8>>,
    pub root: Option<Vec<u8>>,
    pub load_available: bool,
    pub no_pager: bool,
    pub show_format: Option<Vec<u8>>,
}

impl DpkgQueryOptions {
    fn argv(&self) -> Vec<Vec<u8>> {
        let mut argv = Vec::new();
        if let Some(admindir) = &self.admindir {
            push_long_value(&mut argv, b"--admindir", admindir);
        }
        if let Some(root) = &self.root {
            push_long_value(&mut argv, b"--root", root);
        }
        if self.load_available {
            argv.push(b"--load-avail".to_vec());
        }
        if self.no_pager {
            argv.push(b"--no-pager".to_vec());
        }
        if let Some(format) = &self.show_format {
            push_long_value(&mut argv, b"--showformat", format);
        }
        argv
    }
}

/// Every command in dpkg-query 1.23.7, with its exact operand shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpkgQueryAction {
    List {
        patterns: Vec<Vec<u8>>,
    },
    Show {
        patterns: Vec<Vec<u8>>,
    },
    Status {
        packages: Vec<Vec<u8>>,
    },
    ListFiles {
        packages: Vec<Vec<u8>>,
    },
    ControlList {
        package: Vec<u8>,
    },
    ControlShow {
        package: Vec<u8>,
        file: Vec<u8>,
    },
    ControlPath {
        package: Vec<u8>,
        file: Option<Vec<u8>>,
    },
    Search {
        patterns: Vec<Vec<u8>>,
    },
    PrintAvailable {
        packages: Vec<Vec<u8>>,
    },
    Help,
    Version,
}

impl DpkgQueryAction {
    pub const DOCUMENTED_NAMES: [&'static [u8]; 11] = [
        b"--list",
        b"--show",
        b"--status",
        b"--listfiles",
        b"--control-list",
        b"--control-show",
        b"--control-path",
        b"--search",
        b"--print-avail",
        b"--help",
        b"--version",
    ];

    pub const fn name(&self) -> &'static str {
        match self {
            Self::List { .. } => "list",
            Self::Show { .. } => "show",
            Self::Status { .. } => "status",
            Self::ListFiles { .. } => "listfiles",
            Self::ControlList { .. } => "control-list",
            Self::ControlShow { .. } => "control-show",
            Self::ControlPath { .. } => "control-path",
            Self::Search { .. } => "search",
            Self::PrintAvailable { .. } => "print-avail",
            Self::Help => "help",
            Self::Version => "version",
        }
    }

    pub const fn option(&self) -> &'static [u8] {
        match self {
            Self::List { .. } => b"--list",
            Self::Show { .. } => b"--show",
            Self::Status { .. } => b"--status",
            Self::ListFiles { .. } => b"--listfiles",
            Self::ControlList { .. } => b"--control-list",
            Self::ControlShow { .. } => b"--control-show",
            Self::ControlPath { .. } => b"--control-path",
            Self::Search { .. } => b"--search",
            Self::PrintAvailable { .. } => b"--print-avail",
            Self::Help => b"--help",
            Self::Version => b"--version",
        }
    }

    pub fn operands(&self) -> Vec<Vec<u8>> {
        match self {
            Self::List { patterns } | Self::Show { patterns } | Self::Search { patterns } => {
                patterns.clone()
            }
            Self::Status { packages }
            | Self::ListFiles { packages }
            | Self::PrintAvailable { packages } => packages.clone(),
            Self::ControlList { package } => vec![package.clone()],
            Self::ControlShow { package, file } => vec![package.clone(), file.clone()],
            Self::ControlPath { package, file } => {
                let mut operands = vec![package.clone()];
                operands.extend(file.iter().cloned());
                operands
            }
            Self::Help | Self::Version => Vec::new(),
        }
    }

    pub(super) fn from_name_and_operands(
        name: &[u8],
        operands: Vec<Vec<u8>>,
    ) -> Result<Self, DpkgLifecycleGrammarError> {
        let (action, minimum, maximum, missing) = match name {
            b"list" => ("list", 0, None, "package pattern"),
            b"show" => ("show", 0, None, "package pattern"),
            b"status" => ("status", 0, None, "package name"),
            b"listfiles" => ("listfiles", 1, None, "package name"),
            b"control-list" => ("control-list", 1, Some(1), "package name"),
            b"control-show" => ("control-show", 2, Some(2), "package and control file"),
            b"control-path" => ("control-path", 1, Some(2), "package name"),
            b"search" => ("search", 1, None, "filename pattern"),
            b"print-avail" => ("print-avail", 0, None, "package name"),
            b"help" => ("help", 0, Some(0), "argument"),
            b"version" => ("version", 0, Some(0), "argument"),
            _ => {
                return Err(DpkgLifecycleGrammarError::UnknownAction {
                    tool: DPKG_QUERY,
                    action: name.to_vec(),
                });
            }
        };
        require_arity(DPKG_QUERY, action, &operands, minimum, maximum, missing)?;
        Ok(match name {
            b"list" => Self::List { patterns: operands },
            b"show" => Self::Show { patterns: operands },
            b"status" => Self::Status { packages: operands },
            b"listfiles" => Self::ListFiles { packages: operands },
            b"control-list" => Self::ControlList {
                package: operands[0].clone(),
            },
            b"control-show" => Self::ControlShow {
                package: operands[0].clone(),
                file: operands[1].clone(),
            },
            b"control-path" => Self::ControlPath {
                package: operands[0].clone(),
                file: operands.get(1).cloned(),
            },
            b"search" => Self::Search { patterns: operands },
            b"print-avail" => Self::PrintAvailable { packages: operands },
            b"help" => Self::Help,
            b"version" => Self::Version,
            _ => unreachable!("validated query action"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DpkgQueryInvocation {
    pub options: DpkgQueryOptions,
    pub action: DpkgQueryAction,
}

impl DpkgQueryInvocation {
    pub fn parse(argv: &[Vec<u8>]) -> Result<Self, DpkgLifecycleGrammarError> {
        let mut options = DpkgQueryOptions::default();
        let mut action: Option<Vec<u8>> = None;
        let mut action_spelling: Option<Vec<u8>> = None;
        let mut index = 0;
        while let Some(argument) = argv.get(index) {
            if argument == b"--" {
                index += 1;
                break;
            }
            if argument.first() != Some(&b'-') || argument == b"-" {
                break;
            }
            let parsed_action = query_action_name(argument);
            if let Some(name) = parsed_action {
                set_action(&mut action, &mut action_spelling, name, argument)?;
            } else if argument == b"--load-avail" {
                options.load_available = true;
            } else if argument == b"--no-pager" {
                options.no_pager = true;
            } else if argument == b"--admindir" {
                options.admindir = Some(option_value(
                    argv,
                    &mut index,
                    DPKG_QUERY,
                    "--admindir",
                    None,
                )?);
            } else if let Some(value) = long_value(argument, b"--admindir") {
                options.admindir = Some(option_value(
                    argv,
                    &mut index,
                    DPKG_QUERY,
                    "--admindir",
                    Some(value),
                )?);
            } else if argument == b"--root" {
                options.root = Some(option_value(argv, &mut index, DPKG_QUERY, "--root", None)?);
            } else if let Some(value) = long_value(argument, b"--root") {
                options.root = Some(option_value(
                    argv,
                    &mut index,
                    DPKG_QUERY,
                    "--root",
                    Some(value),
                )?);
            } else if argument == b"--showformat" || argument == b"-f" {
                options.show_format = Some(option_value(
                    argv,
                    &mut index,
                    DPKG_QUERY,
                    "--showformat",
                    None,
                )?);
            } else if let Some(value) = long_value(argument, b"--showformat") {
                options.show_format = Some(option_value(
                    argv,
                    &mut index,
                    DPKG_QUERY,
                    "--showformat",
                    Some(value),
                )?);
            } else if let Some(value) = argument
                .strip_prefix(b"-f")
                .filter(|value| !value.is_empty())
            {
                let value = value.strip_prefix(b"=").unwrap_or(value);
                options.show_format = Some(option_value(
                    argv,
                    &mut index,
                    DPKG_QUERY,
                    "-f",
                    Some(value),
                )?);
            } else {
                return Err(DpkgLifecycleGrammarError::UnknownOption {
                    tool: DPKG_QUERY,
                    option: argument.clone(),
                });
            }
            index += 1;
        }
        let action = action.ok_or(DpkgLifecycleGrammarError::MissingAction { tool: DPKG_QUERY })?;
        let action = DpkgQueryAction::from_name_and_operands(&action, argv[index..].to_vec())?;
        Ok(Self { options, action })
    }

    pub fn argv(&self) -> Vec<Vec<u8>> {
        render_with_operands(
            self.options.argv(),
            self.action.option(),
            &self.action.operands(),
        )
    }
}

fn query_action_name(argument: &[u8]) -> Option<&'static [u8]> {
    Some(match argument {
        b"-l" | b"--list" => b"list",
        b"-W" | b"--show" => b"show",
        b"-s" | b"--status" => b"status",
        b"-L" | b"--listfiles" => b"listfiles",
        b"--control-list" => b"control-list",
        b"--control-show" => b"control-show",
        b"-c" | b"--control-path" => b"control-path",
        b"-S" | b"--search" => b"search",
        b"-p" | b"--print-avail" => b"print-avail",
        b"-?" | b"--help" => b"help",
        b"--version" => b"version",
        _ => return None,
    })
}

fn set_action(
    action: &mut Option<Vec<u8>>,
    spelling: &mut Option<Vec<u8>>,
    name: &[u8],
    argument: &[u8],
) -> Result<(), DpkgLifecycleGrammarError> {
    if let Some(first) = spelling {
        return Err(DpkgLifecycleGrammarError::ConflictingActions {
            tool: DPKG_QUERY,
            first: first.clone(),
            second: argument.to_vec(),
        });
    }
    *action = Some(name.to_vec());
    *spelling = Some(argument.to_vec());
    Ok(())
}

/// dpkg-query's documented process result classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpkgQueryExitStatus {
    Success,
    NoMatch,
    Error,
}

impl DpkgQueryExitStatus {
    pub fn from_exit_code(status: u8) -> Result<Self, DpkgLifecycleGrammarError> {
        match status {
            0 => Ok(Self::Success),
            1 => Ok(Self::NoMatch),
            2 => Ok(Self::Error),
            status => Err(DpkgLifecycleGrammarError::UnsupportedExitStatus {
                tool: DPKG_QUERY,
                status,
            }),
        }
    }

    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::NoMatch => 1,
            Self::Error => 2,
        }
    }
}
